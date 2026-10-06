//! `Cargo.lock` → the exact version of every package in the build, and the
//! edges between them.
//!
//! Read straight from the file rather than through `cargo metadata`: plain
//! `cargo metadata` resolves the whole graph (slow on a workspace like
//! Zed's, and it competes with rust-analyzer for the same work), while the
//! lockfile already holds the resolved answer.

use std::collections::HashSet;

use serde::Deserialize;

use crate::metadata::is_crates_io;

/// A reference from one locked package to another, as written in its
/// `dependencies` list: `"name"`, `"name version"`, or
/// `"name version (source)"`. Cargo only adds the version (and source) when
/// the name alone would be ambiguous.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockedDependency {
    pub name: String,
    pub version: Option<String>,
}

impl LockedDependency {
    fn parse(entry: &str) -> Self {
        let mut parts = entry.split_whitespace();
        LockedDependency {
            name: parts.next().unwrap_or_default().to_string(),
            version: parts.next().map(str::to_string),
        }
    }
}

/// One `[[package]]` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    /// Absent for workspace members and other path dependencies.
    pub source: Option<String>,
    pub dependencies: Vec<LockedDependency>,
}

impl LockedPackage {
    /// Whether this package comes from crates.io — the only kind that can be
    /// looked up for newer versions or checked against OSV.
    pub fn is_crates_io(&self) -> bool {
        self.source.as_deref().is_some_and(is_crates_io)
    }
}

/// A parsed `Cargo.lock`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lockfile {
    pub packages: Vec<LockedPackage>,
}

#[derive(Deserialize)]
struct RawLockfile {
    #[serde(default)]
    package: Vec<RawPackage>,
}

#[derive(Deserialize)]
struct RawPackage {
    name: String,
    version: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    dependencies: Vec<String>,
}

/// Parses the contents of a `Cargo.lock`.
pub fn parse_lockfile(contents: &str) -> Result<Lockfile, String> {
    let raw: RawLockfile =
        toml::from_str(contents).map_err(|error| format!("Could not read Cargo.lock: {error}"))?;
    Ok(Lockfile {
        packages: raw
            .package
            .into_iter()
            .map(|package| LockedPackage {
                name: package.name,
                version: package.version,
                source: package.source,
                dependencies: package
                    .dependencies
                    .iter()
                    .map(|entry| LockedDependency::parse(entry))
                    .collect(),
            })
            .collect(),
    })
}

impl Lockfile {
    /// The entry for exactly this name and version.
    pub fn package(&self, name: &str, version: &str) -> Option<&LockedPackage> {
        self.index_of(name, Some(version))
            .map(|index| &self.packages[index])
    }

    fn index_of(&self, name: &str, version: Option<&str>) -> Option<usize> {
        self.packages.iter().position(|package| {
            package.name == name && version.is_none_or(|version| package.version == version)
        })
    }

    /// The version of `dependency` that `crate_name` at `crate_version` is
    /// locked to. `dependency` is the *package* name (not an import alias).
    ///
    /// `None` when the crate or the edge isn't in the lockfile — a lockfile
    /// that predates a just-added dependency, typically.
    pub fn locked_version(
        &self,
        crate_name: &str,
        crate_version: &str,
        dependency: &str,
    ) -> Option<&str> {
        let package = self.package(crate_name, crate_version)?;
        let reference = package
            .dependencies
            .iter()
            .find(|reference| reference.name == dependency)?;
        let index = self.index_of(&reference.name, reference.version.as_deref())?;
        Some(&self.packages[index].version)
    }

    /// Every crates.io package reachable from `crate_name` at
    /// `crate_version`, directly or transitively, sorted by name then
    /// version. The starting crate itself is not included.
    ///
    /// The walk goes *through* path and git packages — what they pull in
    /// from crates.io is still part of the build — but only crates.io
    /// packages are returned. This is the set to check for vulnerabilities
    /// (decision 2 in `docs/Rust_Manager_Design_Note.md`).
    ///
    /// **This is an upper bound.** The lockfile records every edge Cargo
    /// might need on any platform and doesn't say why an edge exists, so
    /// the walk also follows:
    /// - dependencies that only apply to another target. On this fork
    ///   `npm_backend` reaches `tokio-io` through `tempfile → getrandom →
    ///   js-sys → futures-util`, a chain that only exists when building for
    ///   WebAssembly;
    /// - dev- and build-dependencies of sibling crates reached on the way.
    ///
    /// `cargo audit` reads the lockfile the same way and reports the same
    /// set. Narrowing it to one platform needs Cargo's full resolution
    /// (`cargo tree`), which this crate deliberately avoids.
    pub fn reachable_crates_io_packages(
        &self,
        crate_name: &str,
        crate_version: &str,
    ) -> Vec<&LockedPackage> {
        let Some(start) = self.index_of(crate_name, Some(crate_version)) else {
            return Vec::new();
        };

        let mut visited = HashSet::from([start]);
        let mut queue = vec![start];
        while let Some(index) = queue.pop() {
            for reference in &self.packages[index].dependencies {
                if let Some(next) = self.index_of(&reference.name, reference.version.as_deref())
                    && visited.insert(next)
                {
                    queue.push(next);
                }
            }
        }
        visited.remove(&start);

        let mut reachable: Vec<&LockedPackage> = visited
            .into_iter()
            .map(|index| &self.packages[index])
            .filter(|package| package.is_crates_io())
            .collect();
        reachable.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.version.cmp(&b.version))
        });
        reachable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

    /// app ──► serde ──► serde_derive
    ///  │ ├──► sibling (path) ──► log, windows-sys 0.52.0
    ///  │ ├──► windows-sys 0.59.0
    ///  │ └──► forked (git) ──► itoa
    /// orphan ──► unrelated
    fn sample() -> Lockfile {
        parse_lockfile(&format!(
            r#"
# This file is automatically @generated by Cargo.
version = 4

[[package]]
name = "app"
version = "1.0.0"
dependencies = [
 "forked",
 "serde",
 "sibling",
 "windows-sys 0.59.0",
]

[[package]]
name = "forked"
version = "0.3.0"
source = "git+https://github.com/example/forked?rev=abc123#abc123"
dependencies = [
 "itoa",
]

[[package]]
name = "itoa"
version = "1.0.11"
source = "{CRATES_IO}"
checksum = "49f1f14873335454500d59611f1cf4a4b0f786f9ac11f4312a78e4cf2566695b"

[[package]]
name = "log"
version = "0.4.22"
source = "{CRATES_IO}"

[[package]]
name = "orphan"
version = "0.1.0"
dependencies = [
 "unrelated",
]

[[package]]
name = "serde"
version = "1.0.210"
source = "{CRATES_IO}"
dependencies = [
 "serde_derive",
]

[[package]]
name = "serde_derive"
version = "1.0.210"
source = "{CRATES_IO}"

[[package]]
name = "sibling"
version = "0.1.0"
dependencies = [
 "log",
 "serde",
 "windows-sys 0.52.0",
]

[[package]]
name = "unrelated"
version = "9.9.9"
source = "{CRATES_IO}"

[[package]]
name = "windows-sys"
version = "0.52.0"
source = "{CRATES_IO}"

[[package]]
name = "windows-sys"
version = "0.59.0"
source = "{CRATES_IO}"
"#
        ))
        .unwrap()
    }

    fn names(packages: &[&LockedPackage]) -> Vec<String> {
        packages
            .iter()
            .map(|package| format!("{} {}", package.name, package.version))
            .collect()
    }

    #[test]
    fn packages_and_their_sources() {
        let lockfile = sample();
        assert_eq!(lockfile.packages.len(), 11);

        let serde = lockfile.package("serde", "1.0.210").unwrap();
        assert!(serde.is_crates_io());
        assert_eq!(serde.dependencies.len(), 1);

        // Workspace members have no source; git packages aren't crates.io.
        let app = lockfile.package("app", "1.0.0").unwrap();
        assert_eq!(app.source, None);
        assert!(!app.is_crates_io());
        assert!(!lockfile.package("forked", "0.3.0").unwrap().is_crates_io());

        assert!(lockfile.package("serde", "2.0.0").is_none());
        assert!(lockfile.package("missing", "1.0.0").is_none());
    }

    #[test]
    fn dependency_references_carry_a_version_only_when_ambiguous() {
        let lockfile = sample();
        let app = lockfile.package("app", "1.0.0").unwrap();
        assert_eq!(
            app.dependencies,
            vec![
                LockedDependency { name: "forked".into(), version: None },
                LockedDependency { name: "serde".into(), version: None },
                LockedDependency { name: "sibling".into(), version: None },
                LockedDependency { name: "windows-sys".into(), version: Some("0.59.0".into()) },
            ]
        );
        // A trailing "(source)" is dropped; the version is still read.
        assert_eq!(
            LockedDependency::parse("dep 1.2.3 (git+https://example.test/dep#abc)"),
            LockedDependency { name: "dep".into(), version: Some("1.2.3".into()) }
        );
    }

    #[test]
    fn locked_version_of_a_direct_dependency() {
        let lockfile = sample();
        assert_eq!(lockfile.locked_version("app", "1.0.0", "serde"), Some("1.0.210"));
        assert_eq!(lockfile.locked_version("app", "1.0.0", "forked"), Some("0.3.0"));
    }

    #[test]
    fn locked_version_picks_the_version_each_crate_actually_uses() {
        // Two versions of windows-sys are locked; each crate gets its own.
        let lockfile = sample();
        assert_eq!(lockfile.locked_version("app", "1.0.0", "windows-sys"), Some("0.59.0"));
        assert_eq!(lockfile.locked_version("sibling", "0.1.0", "windows-sys"), Some("0.52.0"));
    }

    #[test]
    fn locked_version_is_none_for_what_the_lockfile_does_not_have() {
        let lockfile = sample();
        // Not a dependency of this crate (only of its sibling).
        assert_eq!(lockfile.locked_version("app", "1.0.0", "log"), None);
        // Not in the lockfile at all, e.g. added since it was written.
        assert_eq!(lockfile.locked_version("app", "1.0.0", "brand-new"), None);
        assert_eq!(lockfile.locked_version("missing", "1.0.0", "serde"), None);
        assert_eq!(lockfile.locked_version("app", "9.9.9", "serde"), None);
    }

    #[test]
    fn reachable_set_is_transitive_and_crates_io_only() {
        let lockfile = sample();
        let reachable = lockfile.reachable_crates_io_packages("app", "1.0.0");
        assert_eq!(
            names(&reachable),
            vec![
                // via the git package, which is itself left out
                "itoa 1.0.11",
                // via the path sibling, which is itself left out
                "log 0.4.22",
                "serde 1.0.210",
                "serde_derive 1.0.210",
                // both versions are in the build, so both are reported
                "windows-sys 0.52.0",
                "windows-sys 0.59.0",
            ]
        );
    }

    #[test]
    fn reachable_set_excludes_what_the_crate_does_not_use() {
        let lockfile = sample();
        let from_sibling = names(&lockfile.reachable_crates_io_packages("sibling", "0.1.0"));
        assert_eq!(
            from_sibling,
            vec!["log 0.4.22", "serde 1.0.210", "serde_derive 1.0.210", "windows-sys 0.52.0"]
        );
        // `unrelated` is only reachable from `orphan`.
        assert!(!from_sibling.iter().any(|name| name.starts_with("unrelated")));
        assert_eq!(
            names(&lockfile.reachable_crates_io_packages("orphan", "0.1.0")),
            vec!["unrelated 9.9.9"]
        );
    }

    #[test]
    fn reachable_set_handles_leaves_unknown_crates_and_cycles() {
        let lockfile = sample();
        assert!(lockfile.reachable_crates_io_packages("itoa", "1.0.11").is_empty());
        assert!(lockfile.reachable_crates_io_packages("missing", "1.0.0").is_empty());

        // Dev-dependency cycles are legal in a lockfile; the walk must end.
        let cyclic = parse_lockfile(&format!(
            r#"
[[package]]
name = "a"
version = "1.0.0"
dependencies = ["b"]

[[package]]
name = "b"
version = "1.0.0"
source = "{CRATES_IO}"
dependencies = ["a", "b"]
"#
        ))
        .unwrap();
        assert_eq!(names(&cyclic.reachable_crates_io_packages("a", "1.0.0")), vec!["b 1.0.0"]);
    }

    #[test]
    fn empty_and_invalid_lockfiles() {
        assert!(parse_lockfile("").unwrap().packages.is_empty());
        assert!(parse_lockfile("version = 4\n").unwrap().packages.is_empty());
        let error = parse_lockfile("[[package]]\nname = ").unwrap_err();
        assert!(error.contains("Cargo.lock"), "{error}");
        // A package entry without a version is malformed, not skipped.
        assert!(parse_lockfile("[[package]]\nname = \"a\"\n").is_err());
    }
}
