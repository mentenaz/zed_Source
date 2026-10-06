//! Given what a crate declares, what it is locked to, and what the registry
//! has published: is the dependency behind, by how much, and what would it
//! take to move it?
//!
//! Two different moves, kept apart because they are different commands with
//! different blast radius (see the design note's Updates page):
//! - **In range** — a newer version the declared requirement already
//!   allows. `cargo update <name>` reaches it and only `Cargo.lock` changes.
//! - **Out of range** — a newer version the requirement excludes. The
//!   requirement itself has to change (`cargo add <name>@<version>`), which
//!   for a workspace-inherited dependency means the root manifest.
//!
//! Which bucket a version lands in is decided by the *requirement*, the same
//! way Cargo decides, not by comparing version numbers by eye: `0.3 → 0.4`
//! is out of range for `^0.3` even though only the "minor" number moved.
//!
//! **An in-range target is an upper bound, not a promise.** It is the newest
//! version *this crate's* requirement allows. Cargo resolves the whole
//! workspace at once, so another package's requirement can hold the result
//! lower: on this fork `clap` is `^4.4` and 4.6.7 exists, but
//! `cargo update clap` settles on 4.6.1. Hosts should present the target as
//! "up to", and can run `cargo update --dry-run <name>` for the exact
//! outcome before committing to it.

use semver::{Version, VersionReq};

use crate::index::IndexVersion;

/// Which component of the version number moves, for the row's badge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum UpdateKind {
    Patch,
    Minor,
    Major,
}

impl UpdateKind {
    pub fn label(self) -> &'static str {
        match self {
            UpdateKind::Patch => "patch",
            UpdateKind::Minor => "minor",
            UpdateKind::Major => "major",
        }
    }
}

/// Which component differs between two versions. Equal versions (or ones
/// differing only in pre-release/build metadata) count as `Patch`.
pub fn classify_update(from: &Version, to: &Version) -> UpdateKind {
    if from.major != to.major {
        UpdateKind::Major
    } else if from.minor != to.minor {
        UpdateKind::Minor
    } else {
        UpdateKind::Patch
    }
}

/// How a version's declared minimum Rust version sits against the installed
/// toolchain — the three states of decision 6 in the design note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RustCompat {
    /// Declares a minimum the installed toolchain meets.
    Compatible,
    /// Declares a minimum above the installed toolchain. An error the user
    /// can act on: update the toolchain, or pick an older version.
    TooNew {
        /// The minimum the version declares, as written (`1.85`).
        needs: String,
    },
    /// Declares nothing (or something unreadable), or the installed
    /// toolchain isn't known. Shown as a neutral marker, never as an error
    /// and never as confirmed-compatible.
    NotDeclared,
}

/// Reads `1.85`, `1.74.1` or `1.99.0-nightly` as `(major, minor, patch)`,
/// with missing parts as zero.
fn rust_version_parts(version: &str) -> Option<(u64, u64, u64)> {
    let numeric = version.trim().split(['-', '+', ' ']).next()?;
    let mut parts = numeric.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = match parts.next() {
        Some(part) => part.parse().ok()?,
        None => 0,
    };
    let patch = match parts.next() {
        Some(part) => part.parse().ok()?,
        None => 0,
    };
    Some((major, minor, patch))
}

/// Compares a declared `rust-version` with the installed toolchain version.
///
/// Pass `None` for `toolchain` when it couldn't be determined; everything is
/// then `NotDeclared`, because claiming compatibility would be a guess.
pub fn rust_compat(declared: Option<&str>, toolchain: Option<&str>) -> RustCompat {
    let (Some(declared), Some(toolchain)) = (declared, toolchain) else {
        return RustCompat::NotDeclared;
    };
    let (Some(needs), Some(have)) = (rust_version_parts(declared), rust_version_parts(toolchain))
    else {
        return RustCompat::NotDeclared;
    };
    if needs <= have {
        RustCompat::Compatible
    } else {
        RustCompat::TooNew {
            needs: declared.trim().to_string(),
        }
    }
}

/// A version a dependency could move to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateTarget {
    pub version: Version,
    pub kind: UpdateKind,
    pub rust: RustCompat,
}

/// Where one dependency stands against the registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionStatus {
    /// The newest stable, non-yanked version published, whatever the
    /// requirement says. `None` when the crate has no such version.
    pub latest: Option<Version>,
    /// The locked version was withdrawn by its author. Worth a warning on
    /// the Installed page even when nothing newer is in range.
    pub locked_yanked: bool,
    /// The newest version the declared requirement allows, when that is
    /// newer than what's locked. `cargo update <name>` moves toward it, and
    /// reaches it unless another package in the workspace holds it back —
    /// see the module docs.
    pub in_range: Option<UpdateTarget>,
    /// The newest stable version the requirement does *not* allow, when
    /// that is newer than what's locked. Needs the requirement changed.
    pub out_of_range: Option<UpdateTarget>,
}

impl VersionStatus {
    /// Whether anything newer exists, in or out of range.
    pub fn is_outdated(&self) -> bool {
        self.in_range.is_some() || self.out_of_range.is_some()
    }
}

/// Works out a dependency's status.
///
/// - `requirement`: as declared (`^1.0`, `=2.2.0`, `*`).
/// - `locked`: the version in `Cargo.lock`, if known. Without it nothing can
///   be called outdated — there is no "from" — so both targets are `None`
///   and only `latest` is filled in.
/// - `versions`: the crate's index entries ([`crate::parse_index`]).
/// - `toolchain`: the installed `rustc` version, if known.
///
/// Yanked versions are never targets. Pre-releases are only targets when the
/// requirement itself opts into them, which is Cargo's own rule.
pub fn version_status(
    requirement: &str,
    locked: Option<&str>,
    versions: &[IndexVersion],
    toolchain: Option<&str>,
) -> VersionStatus {
    let available = || versions.iter().filter(|entry| !entry.yanked);
    let latest_stable = available()
        .filter(|entry| !entry.is_prerelease())
        .max_by(|a, b| a.version.cmp(&b.version));

    let mut status = VersionStatus {
        latest: latest_stable.map(|entry| entry.version.clone()),
        locked_yanked: false,
        in_range: None,
        out_of_range: None,
    };

    let Some(locked) = locked.and_then(|locked| Version::parse(locked).ok()) else {
        return status;
    };
    status.locked_yanked = versions
        .iter()
        .any(|entry| entry.version == locked && entry.yanked);

    let target = |entry: &IndexVersion| UpdateTarget {
        version: entry.version.clone(),
        kind: classify_update(&locked, &entry.version),
        rust: rust_compat(entry.rust_version.as_deref(), toolchain),
    };

    // A requirement this crate can't parse can't be reasoned about: report
    // what's newest as out of range rather than guess it's reachable.
    let requirement = VersionReq::parse(requirement).ok();

    status.in_range = requirement.as_ref().and_then(|requirement| {
        available()
            .filter(|entry| requirement.matches(&entry.version) && entry.version > locked)
            .max_by(|a, b| a.version.cmp(&b.version))
            .map(target)
    });

    status.out_of_range = latest_stable
        .filter(|entry| entry.version > locked)
        .filter(|entry| {
            requirement
                .as_ref()
                .is_none_or(|requirement| !requirement.matches(&entry.version))
        })
        .map(target);

    status
}

/// A version offered in a picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionChoice {
    pub version: Version,
    pub rust: RustCompat,
}

/// The versions to offer for install, newest first: never yanked ones, and
/// pre-releases only when asked for.
///
/// With `compatible_only`, versions that declare a minimum Rust version
/// above the toolchain are left out. Versions that declare nothing are
/// *kept* — about a third of real releases don't declare one, and hiding
/// them would empty the list (decision 6).
pub fn available_versions(
    versions: &[IndexVersion],
    toolchain: Option<&str>,
    compatible_only: bool,
    include_prereleases: bool,
) -> Vec<VersionChoice> {
    let mut choices: Vec<VersionChoice> = versions
        .iter()
        .filter(|entry| !entry.yanked)
        .filter(|entry| include_prereleases || !entry.is_prerelease())
        .map(|entry| VersionChoice {
            version: entry.version.clone(),
            rust: rust_compat(entry.rust_version.as_deref(), toolchain),
        })
        .filter(|choice| !(compatible_only && matches!(choice.rust, RustCompat::TooNew { .. })))
        .collect();
    choices.sort_by(|a, b| b.version.cmp(&a.version));
    choices
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(version: &str) -> IndexVersion {
        IndexVersion {
            version: Version::parse(version).unwrap(),
            yanked: false,
            rust_version: None,
        }
    }

    fn yanked(version: &str) -> IndexVersion {
        IndexVersion {
            yanked: true,
            ..entry(version)
        }
    }

    fn needing(version: &str, rust: &str) -> IndexVersion {
        IndexVersion {
            rust_version: Some(rust.to_string()),
            ..entry(version)
        }
    }

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    fn target_version(target: &Option<UpdateTarget>) -> Option<String> {
        target.as_ref().map(|target| target.version.to_string())
    }

    #[test]
    fn update_kind_by_changed_component() {
        assert_eq!(classify_update(&version("1.2.3"), &version("2.0.0")), UpdateKind::Major);
        assert_eq!(classify_update(&version("1.2.3"), &version("1.3.0")), UpdateKind::Minor);
        assert_eq!(classify_update(&version("1.2.3"), &version("1.2.4")), UpdateKind::Patch);
        assert_eq!(classify_update(&version("1.2.3"), &version("1.2.3")), UpdateKind::Patch);
        assert_eq!(UpdateKind::Major.label(), "major");
        assert_eq!(UpdateKind::Minor.label(), "minor");
        assert_eq!(UpdateKind::Patch.label(), "patch");
    }

    #[test]
    fn rust_compat_three_states() {
        assert_eq!(rust_compat(Some("1.85"), Some("1.98.1")), RustCompat::Compatible);
        assert_eq!(rust_compat(Some("1.98.1"), Some("1.98.1")), RustCompat::Compatible);
        assert_eq!(rust_compat(Some("1.98"), Some("1.98.1")), RustCompat::Compatible);
        assert_eq!(
            rust_compat(Some("1.99"), Some("1.98.1")),
            RustCompat::TooNew { needs: "1.99".into() }
        );
        assert_eq!(
            rust_compat(Some("1.98.2"), Some("1.98.1")),
            RustCompat::TooNew { needs: "1.98.2".into() }
        );
        assert_eq!(rust_compat(None, Some("1.98.1")), RustCompat::NotDeclared);
    }

    #[test]
    fn rust_compat_compares_numbers_not_text() {
        // As text, "1.9" sorts after "1.85".
        assert_eq!(rust_compat(Some("1.9"), Some("1.85.0")), RustCompat::Compatible);
        assert_eq!(
            rust_compat(Some("1.100"), Some("1.98.1")),
            RustCompat::TooNew { needs: "1.100".into() }
        );
    }

    #[test]
    fn rust_compat_is_never_a_guess() {
        // Unknown toolchain, or an unreadable declaration: not declared,
        // rather than a claimed pass or a spurious error.
        assert_eq!(rust_compat(Some("1.85"), None), RustCompat::NotDeclared);
        assert_eq!(rust_compat(Some("nightly"), Some("1.98.1")), RustCompat::NotDeclared);
        assert_eq!(rust_compat(Some("1.85"), Some("unknown")), RustCompat::NotDeclared);
        assert_eq!(rust_compat(Some(""), Some("1.98.1")), RustCompat::NotDeclared);
        // Nightly and beta toolchains compare by their number.
        assert_eq!(rust_compat(Some("1.99"), Some("1.99.0-nightly")), RustCompat::Compatible);
    }

    #[test]
    fn up_to_date_dependency() {
        let versions = [entry("1.0.0"), entry("1.0.1")];
        let status = version_status("^1.0", Some("1.0.1"), &versions, Some("1.98.1"));
        assert_eq!(status.latest, Some(version("1.0.1")));
        assert!(!status.is_outdated());
        assert!(!status.locked_yanked);
    }

    #[test]
    fn newer_version_within_the_requirement() {
        let versions = [entry("1.0.0"), entry("1.0.5"), entry("1.4.0")];
        let status = version_status("^1.0", Some("1.0.0"), &versions, Some("1.98.1"));

        let in_range = status.in_range.clone().unwrap();
        assert_eq!(in_range.version, version("1.4.0"));
        assert_eq!(in_range.kind, UpdateKind::Minor);
        assert_eq!(status.out_of_range, None);
        assert!(status.is_outdated());
    }

    #[test]
    fn newer_version_outside_the_requirement() {
        let versions = [entry("1.0.0"), entry("1.4.0"), entry("2.1.0")];
        let status = version_status("^1.0", Some("1.4.0"), &versions, Some("1.98.1"));

        // Nothing newer that `cargo update` could reach…
        assert_eq!(status.in_range, None);
        // …but a major release exists.
        let out_of_range = status.out_of_range.clone().unwrap();
        assert_eq!(out_of_range.version, version("2.1.0"));
        assert_eq!(out_of_range.kind, UpdateKind::Major);
        assert_eq!(status.latest, Some(version("2.1.0")));
    }

    #[test]
    fn both_moves_can_be_available_at_once() {
        let versions = [entry("1.0.0"), entry("1.4.0"), entry("2.1.0")];
        let status = version_status("^1.0", Some("1.0.0"), &versions, None);
        assert_eq!(target_version(&status.in_range).as_deref(), Some("1.4.0"));
        assert_eq!(target_version(&status.out_of_range).as_deref(), Some("2.1.0"));
    }

    #[test]
    fn zero_versions_follow_cargos_rule_not_the_minor_number() {
        // For 0.x, `^0.3` allows 0.3.* only: 0.4.0 is a breaking release
        // even though only the second number moved.
        let versions = [entry("0.3.0"), entry("0.3.9"), entry("0.4.0")];
        let status = version_status("^0.3", Some("0.3.0"), &versions, None);
        assert_eq!(target_version(&status.in_range).as_deref(), Some("0.3.9"));
        let out_of_range = status.out_of_range.clone().unwrap();
        assert_eq!(out_of_range.version, version("0.4.0"));
        assert_eq!(out_of_range.kind, UpdateKind::Minor);
    }

    #[test]
    fn exact_and_wildcard_requirements() {
        let versions = [entry("2.2.0"), entry("2.3.0"), entry("3.0.0")];

        // `=2.2.0` allows nothing else: every newer version is out of range.
        let pinned = version_status("=2.2.0", Some("2.2.0"), &versions, None);
        assert_eq!(pinned.in_range, None);
        assert_eq!(target_version(&pinned.out_of_range).as_deref(), Some("3.0.0"));

        // `*` allows everything: the newest is always in range.
        let any = version_status("*", Some("2.2.0"), &versions, None);
        assert_eq!(target_version(&any.in_range).as_deref(), Some("3.0.0"));
        assert_eq!(any.out_of_range, None);
    }

    #[test]
    fn yanked_versions_are_never_targets() {
        let versions = [entry("1.0.0"), entry("1.1.0"), yanked("1.2.0"), yanked("2.0.0")];
        let status = version_status("^1.0", Some("1.0.0"), &versions, None);
        assert_eq!(target_version(&status.in_range).as_deref(), Some("1.1.0"));
        assert_eq!(status.out_of_range, None);
        assert_eq!(status.latest, Some(version("1.1.0")));
    }

    #[test]
    fn a_yanked_locked_version_is_flagged() {
        let versions = [yanked("1.0.0"), entry("1.0.1")];
        let status = version_status("^1.0", Some("1.0.0"), &versions, None);
        assert!(status.locked_yanked);
        assert_eq!(target_version(&status.in_range).as_deref(), Some("1.0.1"));

        // Flagged even when there is nothing to move to.
        let stuck = version_status("^1.0", Some("1.0.0"), &[yanked("1.0.0")], None);
        assert!(stuck.locked_yanked);
        assert!(!stuck.is_outdated());
        assert_eq!(stuck.latest, None);
    }

    #[test]
    fn prereleases_are_ignored_unless_the_requirement_opts_in() {
        let versions = [entry("1.0.0"), entry("2.0.0-rc.1"), entry("2.0.0-rc.2")];

        let stable = version_status("^1.0", Some("1.0.0"), &versions, None);
        assert!(!stable.is_outdated());
        assert_eq!(stable.latest, Some(version("1.0.0")));

        // Already on a pre-release of 2.0.0: the next one is in range.
        let on_prerelease = version_status("^2.0.0-rc.1", Some("2.0.0-rc.1"), &versions, None);
        assert_eq!(target_version(&on_prerelease.in_range).as_deref(), Some("2.0.0-rc.2"));
        assert_eq!(on_prerelease.out_of_range, None);
    }

    #[test]
    fn targets_report_their_rust_requirement() {
        let versions = [
            entry("1.0.0"),
            needing("1.1.0", "1.70"),
            needing("1.2.0", "1.99"),
            entry("2.0.0"),
        ];
        let status = version_status("^1.0", Some("1.0.0"), &versions, Some("1.98.1"));

        // The newest in range needs a newer toolchain: reported, not hidden.
        let in_range = status.in_range.unwrap();
        assert_eq!(in_range.version, version("1.2.0"));
        assert_eq!(in_range.rust, RustCompat::TooNew { needs: "1.99".into() });

        // The major release declares nothing.
        assert_eq!(status.out_of_range.unwrap().rust, RustCompat::NotDeclared);
    }

    #[test]
    fn nothing_is_outdated_without_a_locked_version() {
        let versions = [entry("1.0.0"), entry("2.0.0")];
        for locked in [None, Some("not-a-version")] {
            let status = version_status("^1.0", locked, &versions, None);
            assert_eq!(status.latest, Some(version("2.0.0")));
            assert!(!status.is_outdated());
            assert!(!status.locked_yanked);
        }
    }

    #[test]
    fn an_unreadable_requirement_reports_the_newest_as_out_of_range() {
        let versions = [entry("1.0.0"), entry("1.5.0")];
        let status = version_status("not a requirement", Some("1.0.0"), &versions, None);
        assert_eq!(status.in_range, None);
        assert_eq!(target_version(&status.out_of_range).as_deref(), Some("1.5.0"));
    }

    #[test]
    fn a_locked_version_newer_than_the_registry_is_not_outdated() {
        // A local or just-published version the index copy doesn't have yet.
        let versions = [entry("1.0.0")];
        let status = version_status("^1.0", Some("1.2.0"), &versions, None);
        assert!(!status.is_outdated());
        assert_eq!(status.latest, Some(version("1.0.0")));
    }

    #[test]
    fn no_published_versions() {
        let status = version_status("^1.0", Some("1.0.0"), &[], None);
        assert_eq!(status.latest, None);
        assert!(!status.is_outdated());
    }

    #[test]
    fn available_versions_newest_first_without_yanked_or_prereleases() {
        let versions = [
            entry("1.0.0"),
            yanked("1.1.0"),
            entry("1.2.0"),
            entry("2.0.0-rc.1"),
            entry("1.10.0"),
        ];
        let order: Vec<String> = available_versions(&versions, None, false, false)
            .iter()
            .map(|choice| choice.version.to_string())
            .collect();
        assert_eq!(order, vec!["1.10.0", "1.2.0", "1.0.0"]);

        let with_prereleases: Vec<String> = available_versions(&versions, None, false, true)
            .iter()
            .map(|choice| choice.version.to_string())
            .collect();
        assert_eq!(with_prereleases, vec!["2.0.0-rc.1", "1.10.0", "1.2.0", "1.0.0"]);
    }

    #[test]
    fn compatible_only_hides_too_new_but_keeps_undeclared() {
        let versions = [
            entry("1.0.0"),
            needing("1.1.0", "1.70"),
            needing("1.2.0", "1.99"),
        ];

        let all = available_versions(&versions, Some("1.98.1"), false, false);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].rust, RustCompat::TooNew { needs: "1.99".into() });
        assert_eq!(all[1].rust, RustCompat::Compatible);
        assert_eq!(all[2].rust, RustCompat::NotDeclared);

        let compatible: Vec<String> = available_versions(&versions, Some("1.98.1"), true, false)
            .iter()
            .map(|choice| choice.version.to_string())
            .collect();
        assert_eq!(compatible, vec!["1.1.0", "1.0.0"]);

        // With no known toolchain nothing can be ruled out.
        assert_eq!(available_versions(&versions, None, true, false).len(), 3);
    }
}
