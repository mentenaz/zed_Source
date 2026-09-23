//! npm package-manager backend — CLI invocation, output parsing, and version
//! classification for the npm manager panel.
//!
//! No GPUI, no app-state dependency: every function here takes plain
//! strings/paths and returns a plain value or `Result` — a host panel wires
//! this into its own UI/state, this crate just does the work.
//!
//! Scope notes (mirrors the plan's §4.1/§4.3):
//! - `run_npm_cli` / `detect_package_manager` / the JSON parsers and the
//!   version classifiers are pure and live here.
//! - The Forge "security aggregation → Dashboard feed" half is intentionally
//!   dropped: this Zed host has no Dashboard consumer, so only the local
//!   per-package `npm audit` list is kept (see `parse_audit`).

use std::process::{Command, Stdio};

use semver::{Version, VersionReq};

mod path_env;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// ── Types ─────────────────────────────────────────────────────────────

/// A single installed package entry, as emitted by `npm ls --json`.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct NpmInstalledPkg {
    pub name: String,
    pub version: String,
    pub path: Option<String>,
    /// `true` when the package is a `devDependency` (npm ls marks these).
    pub is_dev: bool,
}

/// An "outdated" entry, as emitted by the flattened form of
/// `npm outdated --json` (name → current/wanted/latest/dependent/wanted).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct NpmOutdatedPkg {
    pub name: String,
    pub current: String,
    pub wanted: String,
    pub latest: String,
    pub location: Option<String>,
}

/// A local `npm audit` finding (kept; the Forge Dashboard aggregator is not).
#[derive(serde::Serialize, Clone, Debug)]
pub struct NpmAuditVuln {
    pub package: String,
    pub severity: String,
    pub title: String,
    pub url: String,
    pub exploitability: Option<u8>,
    /// Version that `npm audit` reports fixes this finding (if any).
    pub fixed_in: Option<String>,
}

/// Result of classifying a version delta.
#[derive(serde::Serialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum UpdateKind {
    /// Target is a patch of current.
    Patch,
    /// Target is a minor of current.
    Minor,
    /// Target is a major of current.
    Major,
    /// Target is the current
    Current,
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct EngineCompat {
    pub supported: bool,
    pub node_range: String,
    pub npm_range: String,
    pub node_version: String,
    pub npm_version: String,
}

/// Which package manager a project signals via its lockfile.
#[derive(serde::Serialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum PackageManager {
    Npm,
    Yarn,
    Pnpm,
    Bun,
}

impl PackageManager {
    pub fn cli_name(self) -> &'static str {
        match self {
            PackageManager::Npm => "npm",
            PackageManager::Yarn => "yarn",
            PackageManager::Pnpm => "pnpm",
            PackageManager::Bun => "bun",
        }
    }
}

// ── Registry data model ───────────────────────────────────────────────

/// A single match from the registry `/-/v1/search` endpoint. `compat` is
/// filled in by the caller once it knows the active node version.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct NpmSearchResult {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    /// Weekly downloads (present only when the registry search includes it).
    pub downloads: Option<u64>,
    /// The package's declared `engines.node` range, if any.
    pub engines_node: Option<String>,
    /// Whether the active node satisfies `engines_node`; `None` when unknown.
    pub compat: Option<bool>,
}

/// One version of a package from its registry "packument", carrying the
/// peer-dependency ranges it declares.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct NpmVersionEntry {
    pub version: String,
    pub peer_deps: Vec<(String, String)>,
}

/// The parsed `registry.npmjs.org/{name}` packument for one package — the
/// data behind the search details pane.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct NpmPackageDetails {
    pub name: String,
    /// Latest stable version (`dist-tags.latest`, falling back to top level).
    pub version: String,
    pub description: Option<String>,
    pub license: Option<String>,
    pub homepage: Option<String>,
    /// Weekly downloads; `None` when the downloads API 404s (no data).
    pub weekly_downloads: Option<u64>,
    /// All published versions (newest first), with peer-dependency ranges.
    pub versions: Vec<NpmVersionEntry>,
    /// The package README, when the packument carries a non-empty one.
    pub readme: Option<String>,
}

// ── CLI invocation ────────────────────────────────────────────────────

/// Runs the package manager CLI in `project_root` with `args`, returning
/// captured stdout on success. Stderr is folded into the error string.
///
/// On Windows the child is spawned with `CREATE_NO_WINDOW` to avoid a console
/// flash, and the current Node runtime dir is prepended to `PATH` so the CLI
/// can resolve its shell/Node dependencies (see `path_env`).
pub fn run_npm_cli(
    project_root: &str,
    cli_name: &str,
    args: &[String],
) -> Result<String, String> {
    let make_command = |exe: &str, run_args: &[String]| {
        let mut command = Command::new(exe);
        command
            .current_dir(project_root)
            .args(run_args)
            .env("PATH", path_env::path_with_node(project_root))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        command
    };

    let output = match make_command(cli_name, args).output() {
        Ok(output) => output,
        Err(_e) => {
            // Package-manager CLIs are `.cmd` shims on Windows (e.g.
            // `npm.cmd`); spawning them bare fails. Fall back to `cmd /C`
            // so PATH/PATHEXT resolution works, like `node_backend`.
            #[cfg(target_os = "windows")]
            {
                let mut wrapped = Vec::with_capacity(args.len() + 2);
                wrapped.push("/C".to_string());
                wrapped.push(cli_name.to_string());
                wrapped.extend(args.iter().cloned());
                make_command("cmd", &wrapped)
                    .output()
                    .map_err(|e| format!("failed to run `{cli_name}`: {e}"))?
            }
            #[cfg(not(target_os = "windows"))]
            {
                return Err(format!("failed to run `{cli_name}`: {e}"));
            }
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    // npm uses exit code 1 as "found something to report": `npm ls` exits 1
    // when the tree has missing/extraneous problems, `npm outdated` exits 1
    // when packages are behind, `npm audit` exits 1 when vulnerabilities
    // exist — all still printing valid JSON. Only code > 1 is a hard failure
    // (spawn error, bad project, audit error), matching the source at
    // `E:\Forge_GPUI\...\npm_manager_panel.rs` (`if code > 1`).
    let code = output.status.code().unwrap_or(-1);
    if code > 1 {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let mut msg = format!(
            "`{cli_name} {}` exited with {code:?}",
            args.join(" "),
        );
        if !stderr.trim().is_empty() {
            msg.push_str(&format!("\n{stderr}"));
        }
        return Err(msg);
    }
    Ok(stdout)
}

/// Determines the lockfile-driven package manager for `project_root`,
/// defaulting to npm when nothing (or only `package-lock.json`) exists.
pub fn detect_package_manager(project_root: &str) -> PackageManager {
    let dir = std::path::Path::new(project_root);
    for (file, mgr) in [
        ("pnpm-lock.yaml", PackageManager::Pnpm),
        ("yarn.lock", PackageManager::Yarn),
        ("bun.lockb", PackageManager::Bun),
        ("bun.lock", PackageManager::Bun),
    ] {
        if dir.join(file).exists() {
            return mgr;
        }
    }
    PackageManager::Npm
}

/// Lists installed packages (`npm ls --json --depth 0`), returned as tagged
/// entries ready to drop into a row model.
pub fn list_installed(project_root: &str, cli_name: &str) -> Result<Vec<NpmInstalledPkg>, String> {
    let output = run_npm_cli(
        project_root,
        cli_name,
        &["ls".into(), "--json".into(), "--depth".into(), "0".into()],
    )?;
    parse_installed(&output)
}

/// `npm outdated --json` → parsed rows (an empty set means up to date).
pub fn list_outdated(project_root: &str, cli_name: &str) -> Result<Vec<NpmOutdatedPkg>, String> {
    let output = run_npm_cli(
        project_root,
        cli_name,
        &["outdated".into(), "--json".into()],
    )?;
    parse_outdated(&output)
}

/// `npm audit --json` → the local per-package vuln list.
pub fn list_audit_vulns(project_root: &str, cli_name: &str) -> Result<Vec<NpmAuditVuln>, String> {
    let output = run_npm_cli(
        project_root,
        cli_name,
        &["audit".into(), "--json".into(), "--omit=dev".into()],
    )?;
    parse_audit(&output)
}

// ── Parsers ───────────────────────────────────────────────────────────

/// Flattens `npm ls --json` into direct-install rows.
///
/// Two shapes are handled, mirroring the source at
/// `E:\Forge_GPUI\...\npm_manager_panel.rs::parse_installed`:
/// - Classic: a top-level `dependencies` object keyed by package name
///   (`npm ls --json --depth 0` on npm v7+), each node holding at least
///   `version`. The name comes from the map key, not the node.
/// - v7+ lockfile map: a top-level `packages` object keyed by path
///   (`""` for the root, `node_modules/foo`, `node_modules/@scope/foo`, …).
///   The root entry and anything with more than one `node_modules` segment
///   (transitive installs) are skipped.
pub fn parse_installed(raw: &str) -> Result<Vec<NpmInstalledPkg>, String> {
    let json: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("cannot parse npm ls output: {e}"))?;

    let mut out = Vec::new();
    if let Some(deps) = json.get("dependencies").and_then(|d| d.as_object()) {
        for (name, val) in deps {
            out.push(NpmInstalledPkg {
                name: name.clone(),
                version: val
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                path: val
                    .get("path")
                    .and_then(|p| p.as_str())
                    .map(|s| s.to_string()),
                is_dev: val.get("dev").and_then(|d| d.as_bool()).unwrap_or(false),
            });
        }
    } else if let Some(packages) = json.get("packages").and_then(|d| d.as_object()) {
        for (path, entry) in packages {
            let path = path.as_str();
            if path.is_empty()
                || path == "."
                || !path.contains("node_modules")
                || path.match_indices("node_modules").count() > 1
            {
                continue;
            }
            out.push(NpmInstalledPkg {
                name: entry
                    .get("name")
                    .and_then(|n| n.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        path.rsplit('/').next().unwrap_or(path).to_string()
                    }),
                version: entry
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                path: Some(path.to_string()),
                is_dev: entry.get("dev").and_then(|d| d.as_bool()).unwrap_or(false),
            });
        }
    } else {
        return Err(
            "npm ls output had neither 'dependencies' nor 'packages'".to_string(),
        );
    }

    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

/// `npm outdated --json` usually comes back as a per-name object, but with
/// multiple dependents it can nest `{ current, wanted, latest, ... }` under
/// `dependents`. Both shapes flatten to rows here.
pub fn parse_outdated(raw: &str) -> Result<Vec<NpmOutdatedPkg>, String> {
    let json: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("cannot parse npm outdated output: {e}"))?;
    let Ok(obj) = json.as_object().ok_or_else(|| "npm outdated returned no object".to_string())
    else {
        return Ok(Vec::new());
    };

    let mut out = Vec::new();
    for (name, entry) in obj {
        let current = entry
            .get("current")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let wanted = entry
            .get("wanted")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let latest = entry
            .get("latest")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let location = entry.get("location").and_then(|v| v.as_str());
        if wanted.is_empty() || latest.is_empty() {
            continue;
        }
        out.push(NpmOutdatedPkg {
            name: name.clone(),
            current,
            wanted,
            latest,
            location: location.map(|s| s.to_string()),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Extract the local, actionable vuln list out of `npm audit --json`. The
/// modern (v2) shape is `{ vulnerabilities: { pkg: { severity, via: [...] } } }`;
/// the legacy shape is `{ advisories: { id: { module_name, severity, title,
/// url, ... } } }`.
pub fn parse_audit(raw: &str) -> Result<Vec<NpmAuditVuln>, String> {
    let json: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("cannot parse npm audit output: {e}"))?;

    let mut out = Vec::new();

    // Modern shape (npm 7+).
    if let Some(vulns) = json.get("vulnerabilities").and_then(|v| v.as_object()) {
        for (pkg, entry) in vulns {
            let severity = entry
                .get("severity")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let (title, url, exploitability) = entry
                .get("via")
                .and_then(|v| v.as_array())
                .and_then(|via| via.last())
                .and_then(|v| v.as_object())
                .map(|v| {
                    (
                        v.get("title").and_then(|t| t.as_str()).unwrap_or(""),
                        v.get("url").and_then(|u| u.as_str()).unwrap_or("").to_string(),
                        v.get("exploitability").and_then(|e| e.as_u64()).map(|e| e as u8),
                    )
                })
                .unwrap_or(("", String::new(), None));
            let fixed_in = entry
                .get("fixAvailable")
                .and_then(|f| f.get("version"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .filter(|s| !s.is_empty());
            out.push(NpmAuditVuln {
                package: pkg.clone(),
                severity: severity.to_string(),
                title: title.to_string(),
                url,
                exploitability,
                fixed_in,
            });
        }
    }

    // Legacy shape (npm 6).
    if let Some(advs) = json.get("advisories").and_then(|v| v.as_object()) {
        for entry in advs.values() {
            let Some(package) = entry.get("module_name").and_then(|v| v.as_str()) else {
                continue;
            };
            out.push(NpmAuditVuln {
                package: package.to_string(),
                severity: entry
                    .get("severity")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                title: entry
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                url: entry
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                exploitability: None,
                fixed_in: entry
                    .get("fixed_in")
                    .or_else(|| entry.get("patched_versions"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty()),
            });
        }
    }

    out.sort_by(|a, b| a.package.cmp(&b.package));
    Ok(out)
}

// ── Registry parsing (npmjs.org search, packuments, downloads) ────────

/// Parses the registry `/-/v1/search` response into `(results, total)`.
/// `compat` is left `None` — the caller resolves it against the active node
/// via [`node_engine_compatible`].
pub fn parse_npm_search(json: &serde_json::Value) -> (Vec<NpmSearchResult>, usize) {
    let mut out = Vec::new();
    if let Some(objects) = json.get("objects").and_then(|v| v.as_array()) {
        for obj in objects {
            let Some(pkg) = obj.get("package") else {
                continue;
            };
            let Some(name) = pkg.get("name").and_then(|v| v.as_str()) else {
                continue;
            };
            out.push(NpmSearchResult {
                name: name.to_string(),
                version: pkg
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                description: pkg
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                downloads: obj.get("downloads").and_then(|v| v.as_u64()),
                engines_node: pkg
                    .get("engines")
                    .and_then(|v| v.get("node"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                compat: None,
            });
        }
    }
    let total = json
        .get("total")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    let count = out.len();
    (out, total.max(count))
}

/// Parses a `registry.npmjs.org/{name}` packument into package details.
///
/// `versions` are sorted newest-first; each entry's version keeps the
/// peer-dependency ranges it declares (for peer-conflict filtering on the
/// details pane). `readme` is kept only when non-empty and non-placeholder.
pub fn parse_npm_packument(json: &serde_json::Value) -> Result<NpmPackageDetails, String> {
    let name = json
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "packument has no name".to_string())?
        .to_string();
    let version = json
        .get("dist-tags")
        .and_then(|v| v.get("latest"))
        .and_then(|v| v.as_str())
        .or_else(|| json.get("version").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();

    let latest = json
        .get("versions")
        .and_then(|v| v.get(&version));
    let description = latest
        .and_then(|v| v.get("description"))
        .or_else(|| json.get("description"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let license = match latest
        .and_then(|v| v.get("license"))
        .or_else(|| json.get("license"))
    {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Object(o)) => o
            .get("url")
            .and_then(|u| u.as_str())
            .or_else(|| o.get("type").and_then(|t| t.as_str()))
            .map(str::to_string),
        _ => None,
    };
    let homepage = latest
        .and_then(|v| v.get("homepage"))
        .or_else(|| json.get("homepage"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let readme = json
        .get("readme")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty() && *s != "No README found")
        .map(str::to_string);

    let mut versions = Vec::new();
    if let Some(obj) = json.get("versions").and_then(|v| v.as_object()) {
        for (v, info) in obj {
            let peer_deps = info
                .get("peerDependencies")
                .and_then(|d| d.as_object())
                .map(|deps| {
                    deps.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            versions.push(NpmVersionEntry {
                version: v.clone(),
                peer_deps,
            });
        }
    }
    versions.sort_by(|a, b| match (Version::parse(&a.version), Version::parse(&b.version)) {
        (Ok(va), Ok(vb)) => vb.cmp(&va),
        _ => b.version.cmp(&a.version),
    });

    Ok(NpmPackageDetails {
        name,
        version,
        description,
        license,
        homepage,
        weekly_downloads: None,
        versions,
        readme,
    })
}

/// Parses the weekly download count out of `api.npmjs.org/downloads/...`.
pub fn parse_npm_downloads(json: &serde_json::Value) -> Option<u64> {
    json.get("downloads").and_then(|v| v.as_u64())
}

/// Whether the active `node_version` satisfies the package's `engines.node`
/// range. `None` when either side can't be parsed (which also covers
/// `||`-joined ranges, which `semver` doesn't understand).
pub fn node_engine_compatible(node_version: &str, range: &str) -> Option<bool> {
    if range.trim().is_empty() || range.trim() == "*" {
        return Some(true);
    }
    let (Ok(ver), Ok(req)) = (Version::parse(node_version), VersionReq::parse(range)) else {
        return None;
    };
    Some(req.matches(&ver))
}

/// The peer-dependency constraints in `peers` that the *installed* packages
/// already violate (name + range, e.g. `"react@^17.0.0"`). Peers that aren't
/// installed can't be judged and are skipped (fails open).
pub fn peer_conflicts_with_installed(
    peers: &[(String, String)],
    installed: &[NpmInstalledPkg],
) -> Vec<String> {
    peers
        .iter()
        .filter(|(name, range)| {
            installed.iter().find(|p| &p.name == name).is_some_and(|p| {
                VersionReq::parse(range)
                    .map(|req| !req.matches(&Version::parse(&p.version).unwrap_or_else(|_| Version::new(0, 0, 0))))
                    .unwrap_or(false)
            })
        })
        .map(|(name, range)| format!("{name}@{range}"))
        .collect()
}

// ── Version classification ────────────────────────────────────────────

/// Classify an outdated row: how far is `latest` from `current`?
pub fn classify_update(current: &str, latest: &str) -> UpdateKind {
    let (Ok(cur), Ok(latest)) = (Version::parse(current), Version::parse(latest)) else {
        // Non-semver tags (git deps, URLs): out-of-spec → treat as major-range.
        return if current == latest { UpdateKind::Current } else { UpdateKind::Major };
    };
    if cur == latest {
        return UpdateKind::Current;
    }
    if cur.major != latest.major {
        UpdateKind::Major
    } else if cur.minor != latest.minor {
        UpdateKind::Minor
    } else {
        UpdateKind::Patch
    }
}

/// True when the candidate `version` satisfies *every* `ranges` (a set of
/// range strings sourced from package.json `dependencies`/`devDependencies`
/// for that dep).
pub fn version_matches_all(version: &str, ranges: &[String]) -> bool {
    let Ok(ver) = Version::parse(version) else {
        return false;
    };
    ranges.iter().all(|r| {
        match VersionReq::parse(r) {
            Ok(req) => req.matches(&ver),
            // Unparseable range (e.g. `file:`/`workspace:`) can't conflict.
            Err(_) => true,
        }
    })
}

/// Compatibility of a dep version chosen for install against the locked
/// peer-dependency ranges in the tree. Returns the list of peer range
/// descriptions that *would* break at `candidate`.
pub fn peer_conflicts_for(
    candidate: &str,
    peer_dep_ranges: &[(String, String)],
) -> Vec<String> {
    let candidate_ver = Version::parse(candidate).unwrap_or_else(|_| Version::new(0, 0, 0));
    peer_dep_ranges
        .iter()
        .filter(|(_, range)| {
            VersionReq::parse(range)
                .map(|req| !req.matches(&candidate_ver))
                .unwrap_or(false)
        })
        .map(|(pkg, range)| format!("{pkg}@{range}"))
        .collect()
}

/// Engine check: `engines.node` / `engines.npm` from package.json vs the
/// currently active node/npm. Missing engines ⇒ always supported.
pub fn engine_compat(
    engines_node: Option<&str>,
    engines_npm: Option<&str>,
    node_version: &str,
    npm_version: &str,
) -> EngineCompat {
    let node_ver = Version::parse(node_version).unwrap_or_else(|_| Version::new(0, 0, 0));
    let npm_ver = Version::parse(npm_version).unwrap_or_else(|_| Version::new(0, 0, 0));
    let supported = match engines_node {
        Some(range) => VersionReq::parse(range)
            .map(|req| req.matches(&node_ver))
            .unwrap_or(true),
        None => true,
    } && match engines_npm {
        Some(range) => VersionReq::parse(range)
            .map(|req| req.matches(&npm_ver))
            .unwrap_or(true),
        None => true,
    };
    EngineCompat {
        supported,
        node_range: engines_node.unwrap_or("*").to_string(),
        npm_range: engines_npm.unwrap_or("*").to_string(),
        node_version: node_version.to_string(),
        npm_version: npm_version.to_string(),
    }
}

/// Best-effort version of the active node/npm, parsing the `vX.Y.Z` prefix
/// out of their `--version` output.
pub fn version_from_output(output: &str) -> String {
    for token in output.split_whitespace() {
        let token = token.trim_start_matches('v');
        if Version::parse(token).is_ok() {
            return token.to_string();
        }
    }
    output.trim().to_string()
}

/// True when any of `pkg_names` shows up in `advisory_fields` (used by the
/// panel to mark installed packages that appear in the audit list).
pub fn has_vuln(pkg_names: &[String], audit: &[NpmAuditVuln]) -> bool {
    pkg_names
        .iter()
        .any(|name| audit.iter().any(|a| a.package == *name))
}