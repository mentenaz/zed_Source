//! Changing a crate's dependencies: the `cargo add`, `cargo remove` and
//! `cargo update` command lines, what each will do to the manifests beyond
//! the obvious, and reading back what `cargo update --dry-run` says it would
//! change.
//!
//! The arguments built here end up in a shell command line (the host runs
//! them through the Script Runner), so every value placed in one is checked
//! first and refused with a message otherwise. Nothing here runs a command
//! that changes a file: the only Cargo invocation is the dry run.
//!
//! Three things Cargo does that the manager has to know about before it
//! runs anything, all confirmed in a scratch workspace with Cargo 1.98.1:
//!
//! - `cargo add` writes `name.workspace = true` when the root manifest's
//!   `[workspace.dependencies]` has the package, and a literal version into
//!   the member when it doesn't ([`add_effect`]).
//! - `cargo add name@version` on a dependency the member *inherits* does not
//!   touch the root manifest. It replaces `workspace = true` with a literal
//!   version in the member, quietly taking that crate off the workspace's
//!   shared version ([`declaration`]). No Cargo command edits
//!   `[workspace.dependencies]`.
//! - `cargo remove` deletes the `[workspace.dependencies]` entry too when
//!   the removed dependency was its last user ([`remove_effect`]).

use std::path::Path;

use crate::metadata::{CrateInfo, DependencyKind, Workspace};
use crate::{MANIFEST_FILE, run_cargo_output};

// ── Validation ─────────────────────────────────────────────────────────

/// Accepts a crate name as Cargo writes them: ASCII letters, digits, `-`
/// and `_`, at most 64 characters.
///
/// The leading-`-` rule is what stops a "crate" called `--manifest-path`
/// from being read by Cargo as an option.
pub fn check_crate_name(name: &str) -> Result<(), String> {
    let plain = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if plain {
        Ok(())
    } else {
        Err(format!("\"{name}\" is not a valid crate name"))
    }
}

/// Accepts one exact version (`1.2.3`, `1.0.0-rc.1`), not a requirement.
pub fn check_exact_version(version: &str) -> Result<(), String> {
    semver::Version::parse(version)
        .map(|_| ())
        .map_err(|_| format!("\"{version}\" is not a valid version"))
}

/// A target that can be passed on a command line as it stands: a triple
/// such as `x86_64-pc-windows-msvc`. A `cfg(...)` expression is refused,
/// because its parentheses, quotes and spaces mean different things to each
/// shell the command might run in.
fn check_target(target: &str, dependency: &str) -> Result<(), String> {
    let plain = !target.is_empty()
        && !target.starts_with('-')
        && target
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if plain {
        Ok(())
    } else {
        Err(format!(
            "{dependency} is declared under [target.'{target}'], which can't be passed on a \
             command line safely. Edit Cargo.toml by hand."
        ))
    }
}

fn push_kind(args: &mut Vec<String>, kind: DependencyKind) {
    match kind {
        DependencyKind::Normal => {}
        DependencyKind::Dev => args.push("--dev".to_string()),
        DependencyKind::Build => args.push("--build".to_string()),
    }
}

// ── Command lines ──────────────────────────────────────────────────────

/// The arguments for `cargo add`: add `name` to the crate `member`, or, when
/// the crate already has it, change its requirement to `version`.
///
/// Without a version Cargo picks the newest one (or `workspace = true` when
/// the workspace declares the package). `kind` selects the table, so a
/// dev-dependency stays a dev-dependency when its version is changed.
pub fn add_args(
    name: &str,
    version: Option<&str>,
    member: &str,
    kind: DependencyKind,
) -> Result<Vec<String>, String> {
    check_crate_name(name)?;
    check_crate_name(member)?;
    let spec = match version {
        Some(version) => {
            check_exact_version(version)?;
            format!("{name}@{version}")
        }
        None => name.to_string(),
    };
    let mut args = vec!["add".to_string(), spec, "-p".to_string(), member.to_string()];
    push_kind(&mut args, kind);
    Ok(args)
}

/// The arguments for `cargo remove`: remove `name` from the crate `member`.
///
/// `kind` and `target` say which table it is in. Cargo refuses to guess: a
/// dev-dependency removed without `--dev` is an error, not a search.
pub fn remove_args(
    name: &str,
    member: &str,
    kind: DependencyKind,
    target: Option<&str>,
) -> Result<Vec<String>, String> {
    check_crate_name(name)?;
    check_crate_name(member)?;
    let mut args = vec![
        "remove".to_string(),
        name.to_string(),
        "-p".to_string(),
        member.to_string(),
    ];
    push_kind(&mut args, kind);
    if let Some(target) = target {
        check_target(target, name)?;
        args.push("--target".to_string());
        args.push(target.to_string());
    }
    Ok(args)
}

/// One package for `cargo update` to move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateSpec {
    pub name: String,
    /// The version currently locked. Given whenever it is known: with two
    /// versions of a package in the lockfile, the name alone is an error
    /// ("specification `syn` is ambiguous").
    pub locked: Option<String>,
}

impl UpdateSpec {
    pub fn new(name: impl Into<String>, locked: Option<&str>) -> Self {
        UpdateSpec {
            name: name.into(),
            locked: locked.map(str::to_string),
        }
    }
}

/// The arguments for `cargo update` over the named packages, each as
/// `name@locked-version` when the locked version is known.
///
/// `Ok(None)` when `specs` is empty. That is the whole point of this
/// function's shape: a bare `cargo update` re-resolves everything (621
/// lockfile entries on this fork), so there is no way to build one here.
/// "Update all" passes every listed row; a single update passes one.
pub fn update_args(specs: &[UpdateSpec], dry_run: bool) -> Result<Option<Vec<String>>, String> {
    if specs.is_empty() {
        return Ok(None);
    }
    let mut args = vec!["update".to_string()];
    for spec in specs {
        check_crate_name(&spec.name)?;
        args.push(match &spec.locked {
            Some(locked) => {
                check_exact_version(locked)?;
                format!("{}@{locked}", spec.name)
            }
            None => spec.name.clone(),
        });
    }
    if dry_run {
        args.push("--dry-run".to_string());
    }
    Ok(Some(args))
}

/// Joins Cargo's arguments into the one command line the Script Runner
/// executes.
pub fn command_line(args: &[String]) -> String {
    let mut command = "cargo".to_string();
    for arg in args {
        command.push(' ');
        command.push_str(arg);
    }
    command
}

// ── What `cargo update` would change ───────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockChangeKind {
    Update,
    Downgrade,
    Add,
    Remove,
}

/// One line of what `cargo update` did, or with `--dry-run` would do, to
/// `Cargo.lock`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockChange {
    pub kind: LockChangeKind,
    pub name: String,
    /// The version leaving the lockfile. `None` for an addition.
    pub from: Option<String>,
    /// The version entering it. `None` for a removal.
    pub to: Option<String>,
}

fn version_token(token: Option<&str>) -> Option<String> {
    let version = token?.strip_prefix('v')?;
    version
        .starts_with(|c: char| c.is_ascii_digit())
        .then(|| version.to_string())
}

/// Reads the changes out of `cargo update`'s output (it prints them on
/// stderr):
///
/// ```text
///     Updating crates.io index
///      Locking 6 packages to latest compatible versions
///       Adding anstream v1.0.0
///     Updating clap v4.5.49 -> v4.6.1 (available: v4.6.7)
/// ```
///
/// Updating one package routinely moves others with it, as above, which is
/// why the confirmation step shows this list rather than a single version.
/// Lines that are not a change (`Updating crates.io index`, notes,
/// warnings) are skipped.
pub fn parse_update_output(output: &str) -> Vec<LockChange> {
    output
        .lines()
        .filter_map(|line| {
            let mut tokens = line.split_whitespace();
            let verb = tokens.next()?;
            let name = tokens.next()?.to_string();
            let first = version_token(tokens.next())?;
            match verb {
                "Adding" => Some(LockChange {
                    kind: LockChangeKind::Add,
                    name,
                    from: None,
                    to: Some(first),
                }),
                "Removing" => Some(LockChange {
                    kind: LockChangeKind::Remove,
                    name,
                    from: Some(first),
                    to: None,
                }),
                "Updating" | "Downgrading" => {
                    if tokens.next() != Some("->") {
                        return None;
                    }
                    Some(LockChange {
                        kind: if verb == "Updating" {
                            LockChangeKind::Update
                        } else {
                            LockChangeKind::Downgrade
                        },
                        name,
                        from: Some(first),
                        to: Some(version_token(tokens.next())?),
                    })
                }
                _ => None,
            }
        })
        .collect()
}

/// Asks Cargo what updating `specs` would change, without changing it.
///
/// Runs `cargo update --dry-run`, which may refresh the registry index over
/// the network but writes nothing to the project. An empty answer means
/// Cargo would leave the lockfile as it is; that happens when another
/// package in the workspace holds the version down. Blocking: call from a
/// background task.
pub fn update_dry_run(workspace_root: &str, specs: &[UpdateSpec]) -> Result<Vec<LockChange>, String> {
    let Some(args) = update_args(specs, true)? else {
        return Ok(Vec::new());
    };
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let (_, stderr) = run_cargo_output(&args, Some(workspace_root))?;
    Ok(parse_update_output(&stderr))
}

// ── Manifests ──────────────────────────────────────────────────────────

/// A `Cargo.toml`, read only for how its dependencies are written. What
/// they resolve to comes from `cargo metadata`; this answers the one thing
/// that output leaves out, which is whether a dependency is inherited from
/// the workspace.
#[derive(Clone, Debug, PartialEq)]
pub struct Manifest(toml::Table);

/// Parses the contents of a `Cargo.toml`.
pub fn parse_manifest(contents: &str) -> Result<Manifest, String> {
    toml::from_str(contents)
        .map(Manifest)
        .map_err(|error| format!("Could not read Cargo.toml: {error}"))
}

/// Reads and parses the `Cargo.toml` at `path`.
pub fn read_manifest(path: &str) -> Result<Manifest, String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|error| format!("Could not read {path}: {error}"))?;
    parse_manifest(&contents)
}

fn table_name(kind: DependencyKind) -> &'static str {
    match kind {
        DependencyKind::Normal => "dependencies",
        DependencyKind::Dev => "dev-dependencies",
        DependencyKind::Build => "build-dependencies",
    }
}

const KINDS: [DependencyKind; 3] = [
    DependencyKind::Normal,
    DependencyKind::Dev,
    DependencyKind::Build,
];

fn is_inherited(entry: &toml::Value) -> bool {
    entry.get("workspace").and_then(toml::Value::as_bool) == Some(true)
}

/// The package a dependency entry refers to: its `package` field when it is
/// renamed, otherwise its key.
fn entry_package<'a>(key: &'a str, entry: &'a toml::Value) -> &'a str {
    entry
        .get("package")
        .and_then(toml::Value::as_str)
        .unwrap_or(key)
}

impl Manifest {
    fn workspace_dependencies(&self) -> Option<&toml::Table> {
        self.0.get("workspace")?.get("dependencies")?.as_table()
    }

    /// Whether this (root) manifest has a `[workspace.dependencies]` table
    /// at all. A workspace without one declares every version in its
    /// members, so a literal version there is the convention, not a
    /// departure from it.
    pub fn has_workspace_dependencies(&self) -> bool {
        self.workspace_dependencies().is_some()
    }

    /// The key `package` is declared under in `[workspace.dependencies]`.
    /// Usually the package name; different when the workspace renames it.
    pub fn workspace_key(&self, package: &str) -> Option<&str> {
        self.workspace_dependencies()?
            .iter()
            .find(|(key, entry)| entry_package(key, entry) == package)
            .map(|(key, _)| key.as_str())
    }

    /// The dependency table for `kind`, under `[target.<target>]` when a
    /// target is given.
    fn dependency_table(&self, kind: DependencyKind, target: Option<&str>) -> Option<&toml::Table> {
        let scope = match target {
            Some(target) => self.0.get("target")?.get(target)?.as_table()?,
            None => &self.0,
        };
        scope.get(table_name(kind))?.as_table()
    }

    /// Every dependency table in the manifest, of every kind and target.
    fn dependency_tables(&self) -> Vec<&toml::Table> {
        let mut scopes = vec![&self.0];
        if let Some(targets) = self.0.get("target").and_then(toml::Value::as_table) {
            scopes.extend(targets.values().filter_map(toml::Value::as_table));
        }
        scopes
            .into_iter()
            .flat_map(|scope| {
                KINDS
                    .iter()
                    .filter_map(move |kind| scope.get(table_name(*kind))?.as_table())
            })
            .collect()
    }

    /// How many of this manifest's dependency tables inherit the workspace
    /// entry `key`.
    fn inherited_uses(&self, key: &str) -> usize {
        self.dependency_tables()
            .into_iter()
            .filter(|table| table.get(key).is_some_and(is_inherited))
            .count()
    }
}

/// How a crate's manifest writes one of its dependencies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Declaration {
    /// `name.workspace = true`: the version lives in the root manifest's
    /// `[workspace.dependencies]`, shared by every crate that inherits it.
    Inherited,
    /// The requirement is written in this crate's own manifest.
    Literal,
}

/// How `member` declares `package` in the table for `kind` and `target`.
/// `None` when that table doesn't have it.
///
/// This decides whether a requirement can be changed with `cargo add
/// name@version`. For a [`Declaration::Literal`] it can. For a
/// [`Declaration::Inherited`] one the same command would write a literal
/// version over `workspace = true` and leave the workspace's own entry
/// alone, so the host must not run it: the version to change is in the root
/// manifest, which no Cargo command edits.
pub fn declaration(
    root: &Manifest,
    member: &Manifest,
    package: &str,
    kind: DependencyKind,
    target: Option<&str>,
) -> Option<Declaration> {
    let table = member.dependency_table(kind, target)?;
    // An inherited entry is keyed by the workspace's name for the package.
    if let Some(key) = root.workspace_key(package)
        && table.get(key).is_some_and(is_inherited)
    {
        return Some(Declaration::Inherited);
    }
    table
        .iter()
        .any(|(key, entry)| !is_inherited(entry) && entry_package(key, entry) == package)
        .then_some(Declaration::Literal)
}

/// What `cargo add <package>` will write for a package the crate does not
/// have yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddEffect {
    /// The workspace declares the package: Cargo writes
    /// `name.workspace = true`. Nothing to warn about.
    Inherits,
    /// The workspace has a `[workspace.dependencies]` table without this
    /// package: Cargo writes a literal version into the member, which is not
    /// how such a workspace declares dependencies. Warn and ask first
    /// (decision 5 in `docs/Rust_Manager_Design_Note.md`).
    LiteralOutsideWorkspaceTable,
    /// There is no `[workspace.dependencies]` table: a literal version in
    /// the crate is the only way, and the usual one.
    Literal,
}

pub fn add_effect(root: &Manifest, package: &str) -> AddEffect {
    if root.workspace_key(package).is_some() {
        AddEffect::Inherits
    } else if root.has_workspace_dependencies() {
        AddEffect::LiteralOutsideWorkspaceTable
    } else {
        AddEffect::Literal
    }
}

/// What `cargo remove` will change besides the member's own manifest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RemoveEffect {
    /// The root manifest's `[workspace.dependencies]` entry goes too,
    /// because nothing else in the workspace inherits it. The confirmation
    /// says so before anything runs (behaviour rule 5).
    pub removes_workspace_entry: bool,
}

/// Works out the side effect of removing `package` from the `kind`/`target`
/// table of the crate named `member`.
///
/// `members` is every workspace member's manifest by crate name, the crate
/// itself included. Cargo drops a `[workspace.dependencies]` entry once no
/// member inherits it, so this counts the inheriting tables that would be
/// left: the crate's other tables (a dependency can be both normal and dev)
/// and every other member's.
pub fn remove_effect(
    root: &Manifest,
    members: &[(String, Manifest)],
    member: &str,
    package: &str,
    kind: DependencyKind,
    target: Option<&str>,
) -> RemoveEffect {
    let Some(key) = root.workspace_key(package) else {
        return RemoveEffect::default();
    };
    let removed_is_inherited = members
        .iter()
        .find(|(name, _)| name == member)
        .is_some_and(|(_, manifest)| {
            declaration(root, manifest, package, kind, target) == Some(Declaration::Inherited)
        });
    if !removed_is_inherited {
        return RemoveEffect::default();
    }
    let uses: usize = members
        .iter()
        .map(|(_, manifest)| manifest.inherited_uses(key))
        .sum();
    RemoveEffect {
        // The one use counted is the table being removed from.
        removes_workspace_entry: uses <= 1,
    }
}

/// The workspace's root manifest.
pub fn read_root_manifest(workspace: &Workspace) -> Result<Manifest, String> {
    read_manifest(&Path::new(&workspace.root).join(MANIFEST_FILE).to_string_lossy())
}

/// One crate's own manifest.
pub fn read_crate_manifest(krate: &CrateInfo) -> Result<Manifest, String> {
    read_manifest(&krate.manifest_path)
}

/// Every member's manifest by crate name, for [`remove_effect`].
///
/// Reads one small file per crate (286 on this fork), so it is blocking:
/// call from a background task, and only when a removal is being confirmed.
pub fn read_member_manifests(workspace: &Workspace) -> Result<Vec<(String, Manifest)>, String> {
    workspace
        .crates
        .iter()
        .map(|krate| Ok((krate.name.clone(), read_crate_manifest(krate)?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    const ROOT: &str = r#"
[workspace]
members = ["a", "b"]

[workspace.dependencies]
serde = { version = "1.0", features = ["derive"] }
itoa = "1"
async_zip = { package = "async-zip", version = "0.0.17" }
"#;

    fn root() -> Manifest {
        parse_manifest(ROOT).unwrap()
    }

    fn member(contents: &str) -> Manifest {
        parse_manifest(&format!(
            "[package]\nname = \"m\"\nversion = \"0.1.0\"\n{contents}"
        ))
        .unwrap()
    }

    #[test]
    fn crate_names_are_plain() {
        for name in ["serde", "serde_json", "tree-sitter", "h2", "_private"] {
            assert!(check_crate_name(name).is_ok(), "{name}");
        }
        for name in [
            "",
            "-p",
            "--manifest-path",
            "a b",
            "a;rm",
            "a&b",
            "$(x)",
            "@scope/name",
            "serde@1.0",
            "naïve",
            &"x".repeat(65),
        ] {
            assert!(check_crate_name(name).is_err(), "{name}");
        }
    }

    #[test]
    fn versions_are_exact() {
        for version in ["1.2.3", "0.0.17", "1.0.0-rc.1", "1.0.0+build.5"] {
            assert!(check_exact_version(version).is_ok(), "{version}");
        }
        for version in ["", "1", "1.2", "^1.2.3", ">=1.0.0", "1.2.3 --precise", "*", "latest"] {
            assert!(check_exact_version(version).is_err(), "{version}");
        }
    }

    #[test]
    fn add_names_the_crate_and_the_table() {
        assert_eq!(
            add_args("serde", None, "app", DependencyKind::Normal).unwrap(),
            strings(&["add", "serde", "-p", "app"])
        );
        assert_eq!(
            add_args("tempfile", None, "app", DependencyKind::Dev).unwrap(),
            strings(&["add", "tempfile", "-p", "app", "--dev"])
        );
        assert_eq!(
            add_args("cc", Some("1.2.0"), "app", DependencyKind::Build).unwrap(),
            strings(&["add", "cc@1.2.0", "-p", "app", "--build"])
        );
        assert_eq!(
            command_line(&add_args("serde", Some("2.0.0"), "app", DependencyKind::Normal).unwrap()),
            "cargo add serde@2.0.0 -p app"
        );
    }

    #[test]
    fn add_refuses_anything_that_is_not_a_name_or_a_version() {
        assert!(add_args("serde; rm -rf .", None, "app", DependencyKind::Normal).is_err());
        assert!(add_args("serde", None, "app && x", DependencyKind::Normal).is_err());
        assert!(add_args("--git", None, "app", DependencyKind::Normal).is_err());
        assert!(add_args("serde", Some("1.0 --features x"), "app", DependencyKind::Normal).is_err());
        assert!(add_args("serde", Some("^1"), "app", DependencyKind::Normal).is_err());
    }

    #[test]
    fn remove_names_the_table_the_dependency_is_in() {
        assert_eq!(
            remove_args("serde", "app", DependencyKind::Normal, None).unwrap(),
            strings(&["remove", "serde", "-p", "app"])
        );
        assert_eq!(
            remove_args("tempfile", "app", DependencyKind::Dev, None).unwrap(),
            strings(&["remove", "tempfile", "-p", "app", "--dev"])
        );
        assert_eq!(
            remove_args(
                "windows",
                "app",
                DependencyKind::Normal,
                Some("x86_64-pc-windows-msvc")
            )
            .unwrap(),
            strings(&[
                "remove",
                "windows",
                "-p",
                "app",
                "--target",
                "x86_64-pc-windows-msvc"
            ])
        );
    }

    #[test]
    fn remove_refuses_a_cfg_target_with_a_reason() {
        let error = remove_args(
            "libc",
            "app",
            DependencyKind::Normal,
            Some("cfg(target_os = \"linux\")"),
        )
        .unwrap_err();
        assert!(error.contains("libc"), "{error}");
        assert!(error.contains("by hand"), "{error}");
        assert!(remove_args("libc", "app", DependencyKind::Normal, Some("cfg(unix)")).is_err());
        assert!(remove_args("libc", "app", DependencyKind::Normal, Some("--all")).is_err());
    }

    #[test]
    fn update_always_names_a_package() {
        // The rule this function exists for: nothing to update is no
        // command at all, never a bare `cargo update`.
        assert_eq!(update_args(&[], false).unwrap(), None);
        assert_eq!(update_args(&[], true).unwrap(), None);

        let one = [UpdateSpec::new("clap", Some("4.5.49"))];
        assert_eq!(
            update_args(&one, false).unwrap().unwrap(),
            strings(&["update", "clap@4.5.49"])
        );
        assert_eq!(
            update_args(&one, true).unwrap().unwrap(),
            strings(&["update", "clap@4.5.49", "--dry-run"])
        );

        // "Update all": every row named in one command.
        let all = [
            UpdateSpec::new("clap", Some("4.5.49")),
            UpdateSpec::new("tokio", Some("1.52.1")),
            UpdateSpec::new("brand-new", None),
        ];
        let args = update_args(&all, false).unwrap().unwrap();
        assert_eq!(
            command_line(&args),
            "cargo update clap@4.5.49 tokio@1.52.1 brand-new"
        );
        assert!(args.len() > 1);
    }

    #[test]
    fn update_refuses_a_bad_row_rather_than_skipping_it() {
        let specs = [
            UpdateSpec::new("clap", Some("4.5.49")),
            UpdateSpec::new("--aggressive", None),
        ];
        assert!(update_args(&specs, false).is_err());
        let specs = [UpdateSpec::new("clap", Some("4 --precise 1"))];
        assert!(update_args(&specs, false).is_err());
    }

    #[test]
    fn update_output_is_read_into_changes() {
        // Real output of `cargo update --dry-run clap` on this fork.
        let output = "    Updating crates.io index
     Locking 6 packages to latest compatible versions
      Adding anstream v1.0.0
      Adding anstyle-parse v1.0.0
    Updating clap v4.5.49 -> v4.6.1 (available: v4.6.7)
    Updating clap_builder v4.5.49 -> v4.6.0 (available: v4.6.7)
    Updating clap_derive v4.5.49 -> v4.6.1 (available: v4.6.7)
    Updating clap_lex v0.7.6 -> v1.1.1
note: pass `--verbose` to see 530 unchanged dependencies behind latest
warning: not updating lockfile due to dry run
";
        let changes = parse_update_output(output);
        assert_eq!(changes.len(), 6);
        assert_eq!(
            changes[0],
            LockChange {
                kind: LockChangeKind::Add,
                name: "anstream".into(),
                from: None,
                to: Some("1.0.0".into()),
            }
        );
        assert_eq!(
            changes[2],
            LockChange {
                kind: LockChangeKind::Update,
                name: "clap".into(),
                from: Some("4.5.49".into()),
                to: Some("4.6.1".into()),
            }
        );
        assert_eq!(changes[5].name, "clap_lex");
        assert_eq!(changes[5].to.as_deref(), Some("1.1.1"));
    }

    #[test]
    fn update_output_covers_removals_and_downgrades() {
        let output = "    Removing old-dep v0.3.1
 Downgrading time v0.3.41 -> v0.3.36
    Updating git repository `https://example.test/repo`
";
        assert_eq!(
            parse_update_output(output),
            vec![
                LockChange {
                    kind: LockChangeKind::Remove,
                    name: "old-dep".into(),
                    from: Some("0.3.1".into()),
                    to: None,
                },
                LockChange {
                    kind: LockChangeKind::Downgrade,
                    name: "time".into(),
                    from: Some("0.3.41".into()),
                    to: Some("0.3.36".into()),
                },
            ]
        );
    }

    #[test]
    fn update_output_with_nothing_to_change() {
        let output = "    Updating crates.io index
     Locking 0 packages to latest compatible versions
note: pass `--verbose` to see 1 unchanged dependencies behind latest
warning: not updating lockfile due to dry run
";
        assert!(parse_update_output(output).is_empty());
        assert!(parse_update_output("").is_empty());
        assert!(parse_update_output("error: specification `syn` is ambiguous").is_empty());
    }

    #[test]
    fn workspace_entries_are_found_by_package_name() {
        let root = root();
        assert!(root.has_workspace_dependencies());
        assert_eq!(root.workspace_key("serde"), Some("serde"));
        // Renamed in the workspace table: found by the package it names.
        assert_eq!(root.workspace_key("async-zip"), Some("async_zip"));
        assert_eq!(root.workspace_key("async_zip"), None);
        assert_eq!(root.workspace_key("tokio"), None);

        let lone = member("[dependencies]\nserde = \"1\"\n");
        assert!(!lone.has_workspace_dependencies());
        assert_eq!(lone.workspace_key("serde"), None);
    }

    #[test]
    fn adding_follows_the_workspace_table() {
        assert_eq!(add_effect(&root(), "serde"), AddEffect::Inherits);
        assert_eq!(add_effect(&root(), "async-zip"), AddEffect::Inherits);
        assert_eq!(
            add_effect(&root(), "tokio"),
            AddEffect::LiteralOutsideWorkspaceTable
        );

        // No table to follow: a literal version is just how it is done.
        let plain = parse_manifest("[workspace]\nmembers = [\"a\"]\n").unwrap();
        assert_eq!(add_effect(&plain, "tokio"), AddEffect::Literal);
        let lone = member("[dependencies]\n");
        assert_eq!(add_effect(&lone, "tokio"), AddEffect::Literal);
    }

    #[test]
    fn declarations_tell_inherited_from_literal() {
        let root = root();
        let manifest = member(
            r#"
[dependencies]
serde.workspace = true
async_zip = { workspace = true, features = ["tokio"] }
log = "0.4"
zip = { package = "async-zip-other", version = "1" }

[dev-dependencies]
itoa = { workspace = true }
tempfile = "3"

[target.'cfg(windows)'.dependencies]
windows = "0.61"
"#,
        );
        let normal = DependencyKind::Normal;
        assert_eq!(
            declaration(&root, &manifest, "serde", normal, None),
            Some(Declaration::Inherited)
        );
        assert_eq!(
            declaration(&root, &manifest, "async-zip", normal, None),
            Some(Declaration::Inherited)
        );
        assert_eq!(
            declaration(&root, &manifest, "log", normal, None),
            Some(Declaration::Literal)
        );
        // Renamed in the member: found by package name, not by key.
        assert_eq!(
            declaration(&root, &manifest, "async-zip-other", normal, None),
            Some(Declaration::Literal)
        );
        assert_eq!(declaration(&root, &manifest, "zip", normal, None), None);

        // The table matters: itoa is a dev-dependency only.
        assert_eq!(declaration(&root, &manifest, "itoa", normal, None), None);
        assert_eq!(
            declaration(&root, &manifest, "itoa", DependencyKind::Dev, None),
            Some(Declaration::Inherited)
        );
        assert_eq!(declaration(&root, &manifest, "windows", normal, None), None);
        assert_eq!(
            declaration(&root, &manifest, "windows", normal, Some("cfg(windows)")),
            Some(Declaration::Literal)
        );
        assert_eq!(
            declaration(&root, &manifest, "serde", DependencyKind::Build, None),
            None
        );
    }

    #[test]
    fn a_literal_version_of_a_workspace_package_is_literal() {
        // What `cargo add serde@0.9.0 -p m` leaves behind on an inherited
        // dependency: the workspace still declares serde, the member no
        // longer inherits it.
        let manifest = member("[dependencies]\nserde = \"0.9.0\"\n");
        assert_eq!(
            declaration(&root(), &manifest, "serde", DependencyKind::Normal, None),
            Some(Declaration::Literal)
        );
    }

    fn members(a: &str, b: &str) -> Vec<(String, Manifest)> {
        vec![("a".to_string(), member(a)), ("b".to_string(), member(b))]
    }

    fn removing(members: &[(String, Manifest)], package: &str, kind: DependencyKind) -> bool {
        remove_effect(&root(), members, "a", package, kind, None).removes_workspace_entry
    }

    #[test]
    fn removing_the_last_user_takes_the_workspace_entry_with_it() {
        let normal = DependencyKind::Normal;

        // b inherits it too: the entry stays.
        let shared = members(
            "[dependencies]\nitoa.workspace = true\n",
            "[dependencies]\nitoa.workspace = true\n",
        );
        assert!(!removing(&shared, "itoa", normal));

        // a is the only user: the entry goes.
        let last = members(
            "[dependencies]\nitoa.workspace = true\n",
            "[dependencies]\nserde.workspace = true\n",
        );
        assert!(removing(&last, "itoa", normal));

        // b has its own literal version, which doesn't keep the entry.
        let literal_elsewhere = members(
            "[dependencies]\nitoa.workspace = true\n",
            "[dependencies]\nitoa = \"1\"\n",
        );
        assert!(removing(&literal_elsewhere, "itoa", normal));

        // b inherits it under a target table: that counts.
        let under_target = members(
            "[dependencies]\nitoa.workspace = true\n",
            "[target.'cfg(unix)'.build-dependencies]\nitoa.workspace = true\n",
        );
        assert!(!removing(&under_target, "itoa", normal));
    }

    #[test]
    fn a_second_table_in_the_same_crate_keeps_the_entry() {
        let both = members(
            "[dependencies]\nitoa.workspace = true\n\n[dev-dependencies]\nitoa.workspace = true\n",
            "",
        );
        assert!(!removing(&both, "itoa", DependencyKind::Normal));
        assert!(!removing(&both, "itoa", DependencyKind::Dev));
    }

    #[test]
    fn removing_touches_nothing_else_when_it_is_not_inherited() {
        let normal = DependencyKind::Normal;
        // A literal version of a package the workspace also declares.
        let literal = members("[dependencies]\nitoa = \"1\"\n", "");
        assert!(!removing(&literal, "itoa", normal));
        // Not a workspace package at all.
        let outside = members("[dependencies]\nlog = \"0.4\"\n", "");
        assert!(!removing(&outside, "log", normal));
        // Not in the table named.
        let dev_only = members("[dev-dependencies]\nitoa.workspace = true\n", "");
        assert!(!removing(&dev_only, "itoa", normal));
        // A crate that isn't in the workspace.
        assert!(
            !remove_effect(&root(), &dev_only, "missing", "itoa", DependencyKind::Dev, None)
                .removes_workspace_entry
        );
    }

    #[test]
    fn removing_a_renamed_workspace_package() {
        let last = members(
            "[dependencies]\nasync_zip.workspace = true\n",
            "[dependencies]\nserde.workspace = true\n",
        );
        assert!(removing(&last, "async-zip", DependencyKind::Normal));
    }

    #[test]
    fn manifests_are_read_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), ROOT).unwrap();
        std::fs::create_dir(dir.path().join("a")).unwrap();
        let a = dir.path().join("a").join("Cargo.toml");
        std::fs::write(
            &a,
            "[package]\nname = \"a\"\nversion = \"0.1.0\"\n\n[dependencies]\nitoa.workspace = true\n",
        )
        .unwrap();

        let workspace = Workspace {
            root: dir.path().to_string_lossy().into_owned(),
            crates: vec![CrateInfo {
                name: "a".to_string(),
                version: "0.1.0".to_string(),
                manifest_path: a.to_string_lossy().into_owned(),
                rust_version: None,
                dependencies: Vec::new(),
            }],
            default_crates: Vec::new(),
        };
        let root = read_root_manifest(&workspace).unwrap();
        let members = read_member_manifests(&workspace).unwrap();
        assert_eq!(members.len(), 1);
        assert!(
            remove_effect(&root, &members, "a", "itoa", DependencyKind::Normal, None)
                .removes_workspace_entry
        );

        let error = read_manifest(&dir.path().join("missing.toml").to_string_lossy()).unwrap_err();
        assert!(error.contains("missing.toml"), "{error}");
        assert!(parse_manifest("not = [toml").is_err());
    }
}
