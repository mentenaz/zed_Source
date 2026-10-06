//! The crates.io sparse index: where a crate's entry lives, and what it says.
//!
//! One small file per crate at `https://index.crates.io/<path>`, holding one
//! JSON object per published version. It is the same data Cargo itself
//! resolves from, it is served from a CDN, and each line already carries the
//! two things the manager needs beyond the version number: whether the
//! version was yanked, and the minimum Rust version it declares.
//!
//! This module only computes the path and parses the body. The request is
//! the host's job (it needs an async HTTP client, which is an app-layer
//! concern this crate has no opinion on) — same split as the other fork
//! backends. Hosts must send a descriptive `User-Agent`; responses carry an
//! `ETag` and `Cache-Control: max-age=600`, so revalidate rather than refetch.

use semver::Version;
use serde::Deserialize;

pub const INDEX_BASE_URL: &str = "https://index.crates.io";

/// The index path for a crate, following Cargo's layout: names of one, two
/// and three characters get their own directories, everything else is
/// bucketed by its first four characters. Always lowercase.
///
/// `a` → `1/a`, `io` → `2/io`, `syn` → `3/s/syn`, `serde` → `se/rd/serde`.
pub fn index_path(name: &str) -> String {
    let name = name.to_lowercase();
    match name.chars().count() {
        0 => String::new(),
        1 => format!("1/{name}"),
        2 => format!("2/{name}"),
        3 => {
            let first: String = name.chars().take(1).collect();
            format!("3/{first}/{name}")
        }
        _ => {
            let first: String = name.chars().take(2).collect();
            let second: String = name.chars().skip(2).take(2).collect();
            format!("{first}/{second}/{name}")
        }
    }
}

/// The full URL of a crate's sparse-index entry.
pub fn index_url(name: &str) -> String {
    format!("{INDEX_BASE_URL}/{}", index_path(name))
}

/// One published version of a crate, as the index describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexVersion {
    pub version: Version,
    /// Withdrawn by its author. Still downloadable for existing lockfiles,
    /// but never offered as something to install.
    pub yanked: bool,
    /// The minimum Rust version this release declares (`rust-version`), if
    /// it declares one. Many releases, older ones especially, do not.
    pub rust_version: Option<String>,
}

impl IndexVersion {
    pub fn is_prerelease(&self) -> bool {
        !self.version.pre.is_empty()
    }
}

#[derive(Deserialize)]
struct RawIndexLine {
    vers: String,
    #[serde(default)]
    yanked: bool,
    #[serde(default)]
    rust_version: Option<String>,
}

/// Parses a sparse-index response body into its versions, oldest first (the
/// order the index stores them in, re-sorted by version to be safe).
///
/// A line that isn't valid JSON, or whose version isn't semver, is skipped:
/// one bad historical entry shouldn't hide every other version. A non-empty
/// body with *no* readable line is an error — that is an error page or the
/// wrong file, not a crate with no releases.
pub fn parse_index(body: &str) -> Result<Vec<IndexVersion>, String> {
    let mut versions = Vec::new();
    let mut saw_content = false;
    for line in body.lines().map(str::trim).filter(|line| !line.is_empty()) {
        saw_content = true;
        let Ok(raw) = serde_json::from_str::<RawIndexLine>(line) else {
            continue;
        };
        let Ok(version) = Version::parse(&raw.vers) else {
            continue;
        };
        versions.push(IndexVersion {
            version,
            yanked: raw.yanked,
            rust_version: raw.rust_version.filter(|declared| !declared.trim().is_empty()),
        });
    }
    if saw_content && versions.is_empty() {
        return Err("The registry's answer was not a crate index entry".to_string());
    }
    versions.sort_by(|a, b| a.version.cmp(&b.version));
    Ok(versions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_paths_follow_cargos_layout() {
        assert_eq!(index_path("a"), "1/a");
        assert_eq!(index_path("io"), "2/io");
        assert_eq!(index_path("syn"), "3/s/syn");
        assert_eq!(index_path("toml"), "to/ml/toml");
        assert_eq!(index_path("serde"), "se/rd/serde");
        assert_eq!(index_path("serde_json"), "se/rd/serde_json");
        assert_eq!(index_path("windows-registry"), "wi/nd/windows-registry");
        assert_eq!(index_path(""), "");
    }

    #[test]
    fn index_paths_are_lowercased() {
        // Names are case-insensitive on crates.io but the files are not.
        assert_eq!(index_path("Inflector"), "in/fl/inflector");
        assert_eq!(index_path("RustFFT"), "ru/st/rustfft");
        assert_eq!(index_path("Syn"), "3/s/syn");
    }

    #[test]
    fn index_url_joins_base_and_path() {
        assert_eq!(index_url("serde"), "https://index.crates.io/se/rd/serde");
        assert_eq!(index_url("syn"), "https://index.crates.io/3/s/syn");
    }

    #[test]
    fn entries_are_read_one_per_line() {
        // Shaped like real index lines: fields this crate doesn't read
        // (deps, cksum, features, …) are present and ignored.
        let body = concat!(
            r#"{"name":"demo","vers":"1.0.0","deps":[],"cksum":"aa","features":{},"yanked":false}"#,
            "\n",
            r#"{"name":"demo","vers":"1.1.0","deps":[],"cksum":"bb","features":{},"yanked":true,"rust_version":"1.60"}"#,
            "\n",
            r#"{"name":"demo","vers":"2.0.0-rc.1","deps":[],"cksum":"cc","features":{},"yanked":false,"rust_version":"1.74.1","v":2,"pubtime":"2026-01-01T00:00:00Z"}"#,
            "\n",
        );
        let versions = parse_index(body).unwrap();
        assert_eq!(versions.len(), 3);

        assert_eq!(versions[0].version, Version::new(1, 0, 0));
        assert!(!versions[0].yanked);
        assert_eq!(versions[0].rust_version, None);
        assert!(!versions[0].is_prerelease());

        assert!(versions[1].yanked);
        assert_eq!(versions[1].rust_version.as_deref(), Some("1.60"));

        assert_eq!(versions[2].version.to_string(), "2.0.0-rc.1");
        assert!(versions[2].is_prerelease());
        assert_eq!(versions[2].rust_version.as_deref(), Some("1.74.1"));
    }

    #[test]
    fn entries_are_sorted_by_version_not_by_line() {
        let body = concat!(
            r#"{"vers":"1.10.0"}"#,
            "\n",
            r#"{"vers":"1.2.0"}"#,
            "\n",
            r#"{"vers":"1.9.0"}"#,
            "\n",
        );
        let order: Vec<String> = parse_index(body)
            .unwrap()
            .iter()
            .map(|entry| entry.version.to_string())
            .collect();
        assert_eq!(order, vec!["1.2.0", "1.9.0", "1.10.0"]);
    }

    #[test]
    fn unreadable_lines_are_skipped_not_fatal() {
        let body = concat!(
            r#"{"vers":"1.0.0"}"#,
            "\n",
            "not json at all\n",
            r#"{"vers":"not-a-version"}"#,
            "\n",
            r#"{"yanked":false}"#,
            "\n",
            "\n",
            r#"{"vers":"1.0.1","rust_version":"  "}"#,
            "\n",
        );
        let versions = parse_index(body).unwrap();
        let order: Vec<String> = versions.iter().map(|entry| entry.version.to_string()).collect();
        assert_eq!(order, vec!["1.0.0", "1.0.1"]);
        // A blank declaration is no declaration.
        assert_eq!(versions[1].rust_version, None);
    }

    #[test]
    fn empty_body_is_no_versions_but_garbage_is_an_error() {
        assert!(parse_index("").unwrap().is_empty());
        assert!(parse_index("\n\n").unwrap().is_empty());
        assert!(parse_index("<html><body>404 Not Found</body></html>").is_err());
        assert!(parse_index(r#"{"errors":[{"detail":"Not Found"}]}"#).is_err());
    }
}
