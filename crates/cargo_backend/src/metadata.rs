//! `cargo metadata --no-deps` → the workspace's crates and what each one
//! declares as a dependency.
//!
//! `--no-deps` is what keeps this cheap: Cargo loads the workspace's own
//! manifests and stops, without resolving the dependency graph. The output
//! still has `workspace = true` inheritance already applied — a dependency
//! declared as `serde.workspace = true` arrives with its real requirement
//! and merged feature list — which is the reason to ask Cargo rather than
//! parse `Cargo.toml` by hand.

use serde::Deserialize;

/// The crates.io registry as it appears in a dependency's `source`: the
/// git-index form, and the sparse-index form newer Cargo versions may emit.
const CRATES_IO_SOURCES: &[&str] = &[
    "registry+https://github.com/rust-lang/crates.io-index",
    "sparse+https://index.crates.io/",
];

pub(crate) fn is_crates_io(source: &str) -> bool {
    CRATES_IO_SOURCES.contains(&source)
}

/// Which table of the manifest a dependency was declared in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DependencyKind {
    /// `[dependencies]`
    Normal,
    /// `[build-dependencies]`
    Build,
    /// `[dev-dependencies]`
    Dev,
}

impl DependencyKind {
    pub fn label(self) -> &'static str {
        match self {
            DependencyKind::Normal => "normal",
            DependencyKind::Build => "build",
            DependencyKind::Dev => "dev",
        }
    }
}

/// Where a dependency comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DependencySource {
    /// crates.io — the only kind the manager lists, checks for updates, and
    /// checks for vulnerabilities.
    CratesIo,
    /// A local crate, by its directory. On a workspace like Zed's these are
    /// most of any crate's dependencies (its workspace siblings).
    Path(String),
    /// A git repository, as Cargo reports it (URL plus any `?rev=`/`?branch=`).
    Git(String),
    /// A registry other than crates.io.
    OtherRegistry(String),
}

/// One declared dependency of a crate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dependency {
    /// The package name on the registry — what `cargo add`/`remove` and the
    /// lockfile use, regardless of any rename.
    pub name: String,
    /// The name the crate imports it under, when that differs
    /// (`alias = { package = "name" }`).
    pub rename: Option<String>,
    /// The version requirement as Cargo normalizes it (`^1.0`, `*`, …).
    pub requirement: String,
    pub kind: DependencyKind,
    /// The `cfg(...)` or target triple of a `[target.….dependencies]` table.
    pub target: Option<String>,
    pub source: DependencySource,
    pub optional: bool,
    pub uses_default_features: bool,
    pub features: Vec<String>,
}

impl Dependency {
    /// Whether the manager lists this dependency. Path, git and
    /// other-registry dependencies are deliberately left out in v1 — see
    /// decision 10 in `docs/Rust_Manager_Design_Note.md`.
    pub fn is_listed(&self) -> bool {
        self.source == DependencySource::CratesIo
    }
}

/// One crate of the workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrateInfo {
    pub name: String,
    pub version: String,
    /// Absolute path of this crate's `Cargo.toml`.
    pub manifest_path: String,
    /// The crate's own declared minimum Rust version, if any.
    pub rust_version: Option<String>,
    /// Sorted by package name, then kind.
    pub dependencies: Vec<Dependency>,
}

impl CrateInfo {
    /// The directory holding this crate's `Cargo.toml` — where `cargo add
    /// -p`-style commands for it are run from.
    pub fn directory(&self) -> String {
        std::path::Path::new(&self.manifest_path)
            .parent()
            .map(|dir| dir.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// A Cargo workspace (a lone crate is a workspace of one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    /// The directory holding the root `Cargo.toml` and `Cargo.lock`.
    pub root: String,
    /// Every workspace member, sorted by name.
    pub crates: Vec<CrateInfo>,
    /// Names of the crates a bare `cargo build` targets
    /// (`default-members`, or the root package).
    pub default_crates: Vec<String>,
}

impl Workspace {
    pub fn find(&self, name: &str) -> Option<&CrateInfo> {
        self.crates.iter().find(|krate| krate.name == name)
    }

    /// The crate to select when the user hasn't picked one: the first
    /// default member, falling back to the first crate by name.
    pub fn default_crate(&self) -> Option<&CrateInfo> {
        self.default_crates
            .iter()
            .find_map(|name| self.find(name))
            .or_else(|| self.crates.first())
    }
}

// ── Raw `cargo metadata` shapes ────────────────────────────────────────
// Only the fields this crate reads; everything else is ignored, so new
// fields in future Cargo versions don't break parsing.

#[derive(Deserialize)]
struct RawMetadata {
    packages: Vec<RawPackage>,
    workspace_root: String,
    /// Absent before Cargo 1.71.
    #[serde(default)]
    workspace_default_members: Vec<String>,
}

#[derive(Deserialize)]
struct RawPackage {
    name: String,
    version: String,
    id: String,
    manifest_path: String,
    #[serde(default)]
    rust_version: Option<String>,
    #[serde(default)]
    dependencies: Vec<RawDependency>,
}

#[derive(Deserialize)]
struct RawDependency {
    name: String,
    #[serde(default)]
    source: Option<String>,
    req: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    rename: Option<String>,
    #[serde(default)]
    optional: bool,
    #[serde(default = "default_true")]
    uses_default_features: bool,
    #[serde(default)]
    features: Vec<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    path: Option<String>,
}

fn default_true() -> bool {
    true
}

impl RawDependency {
    fn into_dependency(self) -> Dependency {
        let source = match self.source {
            // Cargo reports no source for a path dependency.
            None => DependencySource::Path(self.path.unwrap_or_default()),
            Some(source) if is_crates_io(&source) => DependencySource::CratesIo,
            Some(source) => match source.strip_prefix("git+") {
                Some(url) => DependencySource::Git(url.to_string()),
                None => DependencySource::OtherRegistry(source),
            },
        };
        let kind = match self.kind.as_deref() {
            Some("dev") => DependencyKind::Dev,
            Some("build") => DependencyKind::Build,
            _ => DependencyKind::Normal,
        };
        Dependency {
            name: self.name,
            rename: self.rename,
            requirement: self.req,
            kind,
            target: self.target,
            source,
            optional: self.optional,
            uses_default_features: self.uses_default_features,
            features: self.features,
        }
    }
}

/// Parses the JSON printed by `cargo metadata --no-deps --format-version 1`.
pub fn parse_metadata(json: &str) -> Result<Workspace, String> {
    let raw: RawMetadata = serde_json::from_str(json)
        .map_err(|error| format!("Could not read cargo metadata output: {error}"))?;

    let default_crates = raw
        .workspace_default_members
        .iter()
        .filter_map(|id| raw.packages.iter().find(|package| &package.id == id))
        .map(|package| package.name.clone())
        .collect();

    let mut crates: Vec<CrateInfo> = raw
        .packages
        .into_iter()
        .map(|package| {
            let mut dependencies: Vec<Dependency> = package
                .dependencies
                .into_iter()
                .map(RawDependency::into_dependency)
                .collect();
            dependencies.sort_by(|a, b| {
                a.name
                    .to_lowercase()
                    .cmp(&b.name.to_lowercase())
                    .then(a.kind.cmp(&b.kind))
                    .then(a.target.cmp(&b.target))
            });
            CrateInfo {
                name: package.name,
                version: package.version,
                manifest_path: package.manifest_path,
                rust_version: package.rust_version,
                dependencies,
            }
        })
        .collect();
    crates.sort_by_key(|krate| krate.name.to_lowercase());

    Ok(Workspace {
        root: raw.workspace_root,
        crates,
        default_crates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> String {
        json!({
            "version": 1,
            "workspace_root": "/work/ws",
            "workspace_members": [
                "path+file:///work/ws/crates/zeta#0.2.0",
                "path+file:///work/ws/crates/app#1.0.0"
            ],
            "workspace_default_members": ["path+file:///work/ws/crates/app#1.0.0"],
            "resolve": null,
            "target_directory": "/work/ws/target",
            "packages": [
                {
                    "name": "zeta",
                    "version": "0.2.0",
                    "id": "path+file:///work/ws/crates/zeta#0.2.0",
                    "manifest_path": "/work/ws/crates/zeta/Cargo.toml",
                    "rust_version": "1.80",
                    "dependencies": []
                },
                {
                    "name": "app",
                    "version": "1.0.0",
                    "id": "path+file:///work/ws/crates/app#1.0.0",
                    "manifest_path": "/work/ws/crates/app/Cargo.toml",
                    "rust_version": null,
                    "some_future_field": { "ignored": true },
                    "dependencies": [
                        {
                            "name": "serde",
                            "source": "registry+https://github.com/rust-lang/crates.io-index",
                            "req": "^1.0",
                            "kind": null,
                            "rename": null,
                            "optional": false,
                            "uses_default_features": true,
                            "features": ["rc", "derive"],
                            "target": null,
                            "registry": null
                        },
                        {
                            "name": "tempfile",
                            "source": "registry+https://github.com/rust-lang/crates.io-index",
                            "req": "^3.20.0",
                            "kind": "dev",
                            "rename": null,
                            "optional": false,
                            "uses_default_features": true,
                            "features": [],
                            "target": null,
                            "registry": null
                        },
                        {
                            "name": "cc",
                            "source": "sparse+https://index.crates.io/",
                            "req": "^1",
                            "kind": "build",
                            "rename": null,
                            "optional": false,
                            "uses_default_features": true,
                            "features": [],
                            "target": null,
                            "registry": null
                        },
                        {
                            "name": "windows-registry",
                            "source": "registry+https://github.com/rust-lang/crates.io-index",
                            "req": "^0.6.0",
                            "kind": null,
                            "rename": null,
                            "optional": false,
                            "uses_default_features": true,
                            "features": [],
                            "target": "cfg(windows)",
                            "registry": null
                        },
                        {
                            "name": "zeta",
                            "source": null,
                            "req": "*",
                            "kind": null,
                            "rename": null,
                            "optional": false,
                            "uses_default_features": true,
                            "features": [],
                            "target": null,
                            "registry": null,
                            "path": "/work/ws/crates/zeta"
                        },
                        {
                            "name": "zed-scap",
                            "source": "git+https://github.com/zed-industries/scap?rev=4afea48",
                            "req": "^0.0.8-zed",
                            "kind": null,
                            "rename": "scap",
                            "optional": true,
                            "uses_default_features": false,
                            "features": [],
                            "target": null,
                            "registry": null
                        },
                        {
                            "name": "internal",
                            "source": "registry+https://registry.example.test/index",
                            "req": "^2",
                            "kind": null,
                            "rename": null,
                            "optional": false,
                            "uses_default_features": true,
                            "features": [],
                            "target": null,
                            "registry": "https://registry.example.test/index"
                        }
                    ]
                }
            ]
        })
        .to_string()
    }

    fn dependency<'a>(krate: &'a CrateInfo, name: &str) -> &'a Dependency {
        krate
            .dependencies
            .iter()
            .find(|dependency| dependency.name == name)
            .unwrap()
    }

    #[test]
    fn workspace_root_and_crates_sorted_by_name() {
        let workspace = parse_metadata(&sample()).unwrap();
        assert_eq!(workspace.root, "/work/ws");
        let names: Vec<&str> = workspace.crates.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["app", "zeta"]);

        let zeta = workspace.find("zeta").unwrap();
        assert_eq!(zeta.version, "0.2.0");
        assert_eq!(zeta.rust_version.as_deref(), Some("1.80"));
        assert_eq!(zeta.manifest_path, "/work/ws/crates/zeta/Cargo.toml");
        assert!(zeta.dependencies.is_empty());

        assert_eq!(workspace.find("app").unwrap().rust_version, None);
        assert!(workspace.find("missing").is_none());
    }

    #[test]
    fn default_crate_follows_default_members() {
        let workspace = parse_metadata(&sample()).unwrap();
        assert_eq!(workspace.default_crates, vec!["app"]);
        assert_eq!(workspace.default_crate().unwrap().name, "app");
    }

    #[test]
    fn default_crate_falls_back_to_the_first_by_name() {
        // Older Cargo versions don't report default members at all.
        let json = json!({
            "workspace_root": "/w",
            "packages": [
                { "name": "b", "version": "1.0.0", "id": "b", "manifest_path": "/w/b/Cargo.toml" },
                { "name": "a", "version": "1.0.0", "id": "a", "manifest_path": "/w/a/Cargo.toml" }
            ]
        })
        .to_string();
        let workspace = parse_metadata(&json).unwrap();
        assert!(workspace.default_crates.is_empty());
        assert_eq!(workspace.default_crate().unwrap().name, "a");

        let empty = parse_metadata(r#"{"workspace_root":"/w","packages":[]}"#).unwrap();
        assert!(empty.default_crate().is_none());
    }

    #[test]
    fn dependency_kinds() {
        let workspace = parse_metadata(&sample()).unwrap();
        let app = workspace.find("app").unwrap();
        assert_eq!(dependency(app, "serde").kind, DependencyKind::Normal);
        assert_eq!(dependency(app, "tempfile").kind, DependencyKind::Dev);
        assert_eq!(dependency(app, "cc").kind, DependencyKind::Build);
        assert_eq!(DependencyKind::Normal.label(), "normal");
        assert_eq!(DependencyKind::Build.label(), "build");
        assert_eq!(DependencyKind::Dev.label(), "dev");
    }

    #[test]
    fn dependency_sources() {
        let workspace = parse_metadata(&sample()).unwrap();
        let app = workspace.find("app").unwrap();
        assert_eq!(dependency(app, "serde").source, DependencySource::CratesIo);
        // The sparse-index spelling is crates.io too.
        assert_eq!(dependency(app, "cc").source, DependencySource::CratesIo);
        assert_eq!(
            dependency(app, "zeta").source,
            DependencySource::Path("/work/ws/crates/zeta".to_string())
        );
        assert_eq!(
            dependency(app, "zed-scap").source,
            DependencySource::Git("https://github.com/zed-industries/scap?rev=4afea48".to_string())
        );
        assert_eq!(
            dependency(app, "internal").source,
            DependencySource::OtherRegistry("registry+https://registry.example.test/index".to_string())
        );
    }

    #[test]
    fn only_crates_io_dependencies_are_listed() {
        let workspace = parse_metadata(&sample()).unwrap();
        let app = workspace.find("app").unwrap();
        let listed: Vec<&str> = app
            .dependencies
            .iter()
            .filter(|dependency| dependency.is_listed())
            .map(|dependency| dependency.name.as_str())
            .collect();
        assert_eq!(listed, vec!["cc", "serde", "tempfile", "windows-registry"]);
    }

    #[test]
    fn requirement_features_target_and_rename_are_kept() {
        let workspace = parse_metadata(&sample()).unwrap();
        let app = workspace.find("app").unwrap();

        let serde = dependency(app, "serde");
        assert_eq!(serde.requirement, "^1.0");
        assert_eq!(serde.features, vec!["rc", "derive"]);
        assert!(serde.uses_default_features);
        assert!(!serde.optional);
        assert_eq!(serde.rename, None);
        assert_eq!(serde.target, None);

        assert_eq!(
            dependency(app, "windows-registry").target.as_deref(),
            Some("cfg(windows)")
        );

        // Known by its package name; the import alias is kept alongside.
        let scap = dependency(app, "zed-scap");
        assert_eq!(scap.rename.as_deref(), Some("scap"));
        assert!(scap.optional);
        assert!(!scap.uses_default_features);
    }

    #[test]
    fn dependencies_are_sorted_by_name() {
        let workspace = parse_metadata(&sample()).unwrap();
        let names: Vec<&str> = workspace
            .find("app")
            .unwrap()
            .dependencies
            .iter()
            .map(|dependency| dependency.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["cc", "internal", "serde", "tempfile", "windows-registry", "zed-scap", "zeta"]
        );
    }

    #[test]
    fn crate_directory_is_the_manifests_parent() {
        let workspace = parse_metadata(&sample()).unwrap();
        let directory = workspace.find("app").unwrap().directory();
        assert!(directory.ends_with("app"), "{directory}");
        assert!(!directory.ends_with("Cargo.toml"));
    }

    #[test]
    fn missing_optional_fields_take_cargos_defaults() {
        let json = json!({
            "workspace_root": "/w",
            "packages": [{
                "name": "a", "version": "1.0.0", "id": "a", "manifest_path": "/w/Cargo.toml",
                "dependencies": [{ "name": "log", "req": "^0.4",
                    "source": "registry+https://github.com/rust-lang/crates.io-index" }]
            }]
        })
        .to_string();
        let workspace = parse_metadata(&json).unwrap();
        let log = dependency(&workspace.crates[0], "log");
        assert_eq!(log.kind, DependencyKind::Normal);
        assert!(log.uses_default_features);
        assert!(!log.optional);
        assert!(log.features.is_empty());
    }

    #[test]
    fn output_that_is_not_metadata_is_an_error() {
        assert!(parse_metadata("").is_err());
        assert!(parse_metadata("error: could not find `Cargo.toml`").is_err());
        assert!(parse_metadata("{}").is_err());
        let error = parse_metadata("[]").unwrap_err();
        assert!(error.contains("cargo metadata"), "{error}");
    }
}
