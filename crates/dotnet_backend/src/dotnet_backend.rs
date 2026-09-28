//! .NET / NuGet backend — SDK detection, project scanning, package
//! reference reading, `dotnet list package` JSON parsing, and the pure
//! parsers for NuGet's registry JSON shapes.
//!
//! No GPUI, no app-state dependency: every function here takes plain
//! strings/paths and returns a plain value or `Result` — a host panel wires
//! this into its own UI/state, this crate just does the work. The host panel
//! is responsible for making the HTTP calls to the NuGet registry; the
//! *parsing* of those responses lives here so it stays testable and shared.

use std::path::Path;

use serde::Serialize;

mod path_env;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// ── Types ─────────────────────────────────────────────────────────────

#[derive(Serialize, Clone, Debug)]
pub struct DotnetProject {
    /// Absolute path to the `.csproj` or `.sln` file itself.
    pub path: String,
    /// File stem (`MyApp`, `MyApp.sln` → `MyApp`).
    pub name: String,
    pub is_solution: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct InstalledPackage {
    pub id: String,
    pub version: String,
}

/// Which part of a package's version moved between the installed and latest
/// releases — drives the severity badge on the Updates page.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum UpdateKind {
    Major,
    Minor,
    Patch,
}

impl UpdateKind {
    pub fn label(self) -> &'static str {
        match self {
            UpdateKind::Major => "major",
            UpdateKind::Minor => "minor",
            UpdateKind::Patch => "patch",
        }
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct OutdatedPackage {
    pub id: String,
    pub installed: String,
    pub latest: String,
    pub update_kind: UpdateKind,
}

#[derive(Serialize, Clone, Debug)]
pub struct VulnerablePackage {
    pub id: String,
    /// Normalized lowercase: "low" | "moderate" | "high" | "critical".
    pub severity: String,
    pub advisory_url: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct NugetSearchResult {
    pub id: String,
    pub version: String,
    pub description: Option<String>,
    pub total_downloads: u64,
}

#[derive(Serialize, Clone, Debug)]
pub struct NugetDependency {
    pub id: String,
    pub range: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct NugetDependencyGroup {
    pub target_framework: String,
    pub dependencies: Vec<NugetDependency>,
}

#[derive(Serialize, Clone, Debug)]
pub struct NugetPackageDetails {
    pub id: String,
    pub version: String,
    pub description: Option<String>,
    pub authors: String,
    pub project_url: Option<String>,
    pub license_url: Option<String>,
    /// Newest-first, full catalog order (capped by the UI, not here).
    pub versions: Vec<String>,
    /// `readme` is *not* resolved by the parser — the host fetches it from
    /// the v3-flatcontainer endpoint and hands it back in here.
    pub readme: Option<String>,
    pub dependencies: Vec<NugetDependencyGroup>,
    pub vulnerabilities: Vec<VulnerablePackage>,
}

// ── Filesystem ────────────────────────────────────────────────────────

fn read_dir_names(root: &Path) -> Vec<(String, bool)> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| {
            let entry = e.ok()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            Some((name, is_dir))
        })
        .collect()
}

// ── Project scanning ──────────────────────────────────────────────────

/// Maximum depth the .NET-project scanner descends from the workspace root.
pub const MAX_SCAN_DEPTH: usize = 5;
/// Directories never descended into while scanning for .NET projects.
pub const SCAN_SKIP_DIRS: &[&str] = &[
    ".git",
    ".vs",
    ".idea",
    ".cache",
    "node_modules",
    "bin",
    "obj",
    "target",
    "dist",
    "build",
    "out",
    "packages",
];

/// Walks `root` (to [`MAX_SCAN_DEPTH`], skipping [`SCAN_SKIP_DIRS`]) and
/// reports every `.csproj` / `.sln` file it finds. Stops descending once a
/// `.git` directory is found, treating that as a project boundary.
pub fn scan_dotnet_projects(root: &str, depth: usize) -> Vec<DotnetProject> {
    if depth > MAX_SCAN_DEPTH {
        return Vec::new();
    }
    let root = Path::new(root);
    let entries = read_dir_names(root);
    if entries.is_empty() {
        return Vec::new();
    }

    let mut projects = Vec::new();
    for (name, _) in &entries {
        if name.ends_with(".csproj") {
            let stem = name.trim_end_matches(".csproj").to_string();
            projects.push(DotnetProject {
                path: root.join(name).to_string_lossy().into_owned(),
                name: stem,
                is_solution: false,
            });
        } else if name.ends_with(".sln") {
            let stem = name.trim_end_matches(".sln").to_string();
            projects.push(DotnetProject {
                path: root.join(name).to_string_lossy().into_owned(),
                name: stem,
                is_solution: true,
            });
        }
    }

    if depth > 0
        && entries
            .iter()
            .any(|(name, is_dir)| name == ".git" && *is_dir)
    {
        return projects;
    }

    for (name, is_dir) in &entries {
        if !*is_dir {
            continue;
        }
        if SCAN_SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        let sub_root = root.join(name);
        projects.extend(scan_dotnet_projects(&sub_root.to_string_lossy(), depth + 1));
    }

    projects
}

/// Runtime/package operations (install/update/remove, `dotnet list package`)
/// target a project file, never a solution — and the CLI itself rejects a
/// directory that contains more than one project. This resolves the csproj a
/// host should operate on:
/// - a `.csproj` path → itself;
/// - a `.sln` path → a csproj sitting next to it (else the nearest one);
/// - a bare directory → the nearest csproj within it.
/// Returns `None` when no csproj can be found.
pub fn resolve_csproj(project_path: &str) -> Option<String> {
    let p = Path::new(project_path);
    if p.extension()
        .map(|e| e.eq_ignore_ascii_case("csproj"))
        .unwrap_or(false)
    {
        return Some(project_path.to_string());
    }

    let dir = if p.is_dir() {
        p
    } else {
        p.parent().unwrap_or(p)
    };
    // A csproj directly next to the selected sln is the obvious favourite.
    if let Some((name, _)) = read_dir_names(dir)
        .into_iter()
        .find(|(name, _)| name.ends_with(".csproj"))
    {
        return Some(dir.join(name).to_string_lossy().into_owned());
    }

    let dir_str = dir.to_string_lossy().into_owned();
    scan_dotnet_projects(&dir_str, 2)
        .into_iter()
        .find(|proj| !proj.is_solution)
        .map(|proj| proj.path)
}

/// First `.csproj` under `root` (bounded scan), used by the manager when the
/// user hasn't picked a specific project yet.
pub fn find_csproj(root: &str) -> Option<String> {
    scan_dotnet_projects(root, 2)
        .into_iter()
        .find(|proj| !proj.is_solution)
        .map(|proj| proj.path)
}

// ── package reference parsing ─────────────────────────────────────────

/// Extract `(id, version)` pairs from `<PackageReference Include="X"
/// Version="Y" />` tags, attribute order independent. Handles both the
/// self-closing form and the child-element form
/// (`<PackageReference Include="X"><Version>Y</Version></PackageReference>`),
/// and skips closing tags and `PackageReferenceUpdate` (dotnet-tool style,
/// not a package reference).
pub fn parse_package_refs(xml: &str) -> Vec<InstalledPackage> {
    const KEY: &str = "PackageReference";
    const CLOSE: &str = "</PackageReference>";
    let bytes = xml.as_bytes();
    let mut out = Vec::new();
    let mut idx = 0;
    while idx < bytes.len() {
        let Some(rel) = xml[idx..].find(KEY) else {
            break;
        };
        let start = idx + rel;

        // Skip closing tags (`prev == '/'`).
        if start > 0 && bytes[start - 1] == b'/' {
            idx = start + KEY.len();
            continue;
        }
        // Skip longer identifiers like `PackageReferenceUpdate`.
        let prev_ok =
            start == 0 || !bytes[start - 1].is_ascii_alphanumeric() && bytes[start - 1] != b'_';
        let after_word = &xml[start + KEY.len()..];
        let next_ok = after_word
            .chars()
            .next()
            .map(|c| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(true);
        if !prev_ok || !next_ok {
            idx = start + KEY.len();
            continue;
        }

        let Some(gt) = after_word.find('>') else {
            break;
        };
        let attrs = &after_word[..gt];
        let self_closing = attrs.trim_end().ends_with('/');
        let id = attr_val(attrs, "Include");
        let mut version = attr_val(attrs, "Version");

        let mut next_idx = start + KEY.len() + gt + 1;
        if !self_closing && version.is_none() {
            // Child-element form: look for `<Version>…</Version>` inside the
            // element body before the matching `</PackageReference>`.
            if let Some(close) = after_word[gt + 1..].find(CLOSE) {
                let body = &after_word[gt + 1..gt + 1 + close];
                version = child_element(body, "Version");
                next_idx = start + KEY.len() + gt + 1 + close + CLOSE.len();
            }
        }

        if let (Some(id), Some(version)) = (id, version) {
            if !id.trim().is_empty() && !version.trim().is_empty() {
                out.push(InstalledPackage { id, version });
            }
        }
        idx = next_idx;
    }
    out
}

/// Extract `(id, version)` pairs from a legacy `packages.config`
/// (`<package id="X" version="Y" targetFramework="…" />`).
pub fn parse_packages_config(xml: &str) -> Vec<InstalledPackage> {
    const KEY: &str = "<package";
    let mut outer_idx = 0;
    let mut out = Vec::new();
    while outer_idx < xml.len() {
        let Some(rel) = xml[outer_idx..].find(KEY) else {
            break;
        };
        let start = outer_idx + rel;
        let after = &xml[start + KEY.len()..];
        let next_ok = after
            .chars()
            .next()
            .map(|c| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(true);
        if !next_ok {
            outer_idx = start + KEY.len();
            continue;
        }
        let Some(gt) = after.find('>') else {
            break;
        };
        let attrs = &after[..gt];
        if let (Some(id), Some(version)) = (attr_val(attrs, "id"), attr_val(attrs, "version")) {
            if !id.trim().is_empty() && !version.trim().is_empty() {
                out.push(InstalledPackage { id, version });
            }
        }
        outer_idx = start + KEY.len() + gt + 1;
    }
    out
}

/// Read an attribute out of a tag's attribute list, respecting either quote
/// style and any whitespace around the `=`.
fn attr_val(attrs: &str, name: &str) -> Option<String> {
    let mut rest = attrs;
    while let Some(i) = rest.find(name) {
        rest = &rest[i..];
        if !rest.starts_with(&format!("{name}=")) {
            rest = &rest[name.len()..];
            continue;
        }
        let after_eq = rest[name.len() + 1..].trim_start();
        let quote = after_eq.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let body = &after_eq[1..];
        let end = body.find(quote)?;
        return Some(body[..end].to_string());
    }
    None
}

/// Extract the body of the first `<name>…</name>` element in `element`.
fn child_element(element: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let body = element.split(&open).nth(1)?.split(&close).next()?;
    let trimmed = body.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Read every `<PackageReference>` (csproj) plus a sibling `packages.config`
/// (legacy), deduped by `(id, version)` and sorted by id. Missing/unreadable
/// files yield an empty list rather than an error — a project with no
/// package refs is legitimately not an error.
pub fn read_installed_packages(csproj_path: &str) -> Vec<InstalledPackage> {
    let mut out: Vec<InstalledPackage> = Vec::new();
    if let Ok(content) = std::fs::read_to_string(csproj_path) {
        out.extend(parse_package_refs(&content));
    }
    if let Some(dir) = Path::new(csproj_path).parent() {
        if let Ok(content) = std::fs::read_to_string(dir.join("packages.config")) {
            out.extend(parse_packages_config(&content));
        }
    }

    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert((p.id.to_lowercase(), p.version.clone())));
    out.sort_by(|a, b| a.id.to_lowercase().cmp(&b.id.to_lowercase()));
    out
}

// ── version helpers ───────────────────────────────────────────────────

/// Parse `major.minor.patch` from a semver-ish string, tolerating
/// 1- and 2-part versions and prerelease/build suffixes
/// (`1.2.3-beta.1` → `(1, 2, 3)`).
fn version_parts(v: &str) -> Option<(u32, u32, u32)> {
    let mut it = v.split('.');
    let major = it.next()?.trim().parse().ok()?;
    let minor = it
        .next()
        .map(|s| s.trim().parse().unwrap_or(0))
        .unwrap_or(0);
    let patch = it
        .next()
        .map(|s| {
            let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse().unwrap_or(0)
        })
        .unwrap_or(0);
    Some((major, minor, patch))
}

/// Classify the kind of update from `installed` → `latest`. When the parts
/// can't be parsed at all it defaults to `Major` (the conservative choice:
/// a visual nudge to at least review the bump).
pub fn classify_update(installed: &str, latest: &str) -> UpdateKind {
    match (version_parts(installed), version_parts(latest)) {
        (Some(inst), Some(lat)) if inst != lat => {
            if inst.0 != lat.0 {
                UpdateKind::Major
            } else if inst.1 != lat.1 {
                UpdateKind::Minor
            } else {
                UpdateKind::Patch
            }
        }
        _ => UpdateKind::Patch,
    }
}

/// Human-friendly download count: `999 → "999"`, `1500 → "1.5K"`,
/// `2_400_000 → "2.4M"`, `3_100_000_000 → "3.1B"`.
pub fn fmt_downloads(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.1}B", n as f64 / 1_000_000_000.0)
    } else if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

// ── dotnet CLI ────────────────────────────────────────────────────────

/// Run `dotnet` synchronously with `args` in `cwd`, returning captured
/// stdout. Falls back to `cmd /C dotnet …` on Windows if the bare spawn
/// fails (odd PATH setups where only the `.bat` shim is resolvable), and
/// reports stderr when the CLI exits non-zero (which is how
/// `dotnet list package` surfaces "must restore first" / "unrecognized
/// project" style failures).
fn run_dotnet_captured(args: &[&str], cwd: &str) -> Result<String, String> {
    let mut c = gpui_util::new_std_command("dotnet");
    c.args(args).current_dir(cwd);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(CREATE_NO_WINDOW);
        path_env::enrich_path(&mut c);
    }
    let out = match c.output() {
        Ok(out) => out,
        Err(e) => {
            log::debug!("run_dotnet_captured: direct 'dotnet' failed: {e}");
            #[cfg(target_os = "windows")]
            {
                let mut sh = gpui_util::new_std_command("cmd");
                sh.args(["/C", "dotnet"]).args(args).current_dir(cwd);
                use std::os::windows::process::CommandExt;
                sh.creation_flags(CREATE_NO_WINDOW);
                path_env::enrich_path(&mut sh);
                match sh.output() {
                    Ok(fb) => {
                        log::debug!(
                            "run_dotnet_captured: cmd /C 'dotnet' status={} stdout={} stderr={}",
                            fb.status,
                            String::from_utf8_lossy(&fb.stdout).trim(),
                            String::from_utf8_lossy(&fb.stderr).trim(),
                        );
                        fb
                    }
                    Err(e2) => {
                        log::debug!("run_dotnet_captured: cmd /C 'dotnet' also failed: {e2}");
                        return Err("`dotnet` was not found on PATH".to_string());
                    }
                }
            }
            #[cfg(not(target_os = "windows"))]
            return Err("`dotnet` was not found on PATH".to_string());
        }
    };

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("dotnet exited with code {:?}", out.status.code())
        } else {
            stderr
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// [`Self::run_dotnet_captured`] wrapper for the manager's background-run
/// fallback (panel code holds its arguments as `Vec<String>` instead of
/// string literals).
pub fn run_dotnet_args(args: &[String], cwd: &str) -> Result<String, String> {
    let strs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    run_dotnet_captured(&strs, cwd)
}

/// Query `dotnet --version`.
pub fn query_runtime(_exe: String) -> Result<String, String> {
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_string());
    run_dotnet_captured(&["--version"], &cwd)
}

/// Outdated packages (`dotnet list package --outdated --format json`), run
/// in the *folder containing* the solution/underlying csproj — the CLI
/// refuses directories that hold more than one project file. Blank output
/// (no projects / no refs) is a success, not an error.
pub fn list_outdated(dir: String) -> Result<Vec<OutdatedPackage>, String> {
    let stdout = run_dotnet_captured(&["list", "package", "--outdated", "--format", "json"], &dir)?;
    if stdout.trim().is_empty() {
        return Ok(Vec::new());
    }
    parse_outdated_json(&stdout)
}

/// Vulnerable packages (`dotnet list package --vulnerable --format json`).
pub fn list_vulnerable(dir: String) -> Result<Vec<VulnerablePackage>, String> {
    let stdout = run_dotnet_captured(
        &["list", "package", "--vulnerable", "--format", "json"],
        &dir,
    )?;
    if stdout.trim().is_empty() {
        return Ok(Vec::new());
    }
    parse_vulnerable_json(&stdout)
}

/// Parse `dotnet list package --outdated --format json` (verified against
/// .NET 8/9/10 SDK output): `projects[].frameworks[].topLevelPackages[]`
/// with `resolvedVersion` / `latestVersion` per entry. Duplicate ids across
/// frameworks are collapsed (first occurrence wins, sorted).
pub fn parse_outdated_json(json: &str) -> Result<Vec<OutdatedPackage>, String> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| format!("Failed to parse dotnet list package output: {e}"))?;

    let mut seen = std::collections::HashSet::new();
    let mut entries = Vec::new();
    for project in value
        .get("projects")
        .and_then(|p| p.as_array())
        .into_iter()
        .flatten()
    {
        for fw in project
            .get("frameworks")
            .and_then(|f| f.as_array())
            .into_iter()
            .flatten()
        {
            for pkg in fw
                .get("topLevelPackages")
                .and_then(|p| p.as_array())
                .into_iter()
                .flatten()
            {
                let id = pkg
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if id.is_empty() || !seen.insert(id.to_lowercase()) {
                    continue;
                }
                let installed = pkg
                    .get("resolvedVersion")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let latest = pkg
                    .get("latestVersion")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if installed.is_empty() || latest.is_empty() || installed == latest {
                    continue;
                }
                entries.push(OutdatedPackage {
                    update_kind: classify_update(&installed, &latest),
                    id,
                    installed,
                    latest,
                });
            }
        }
    }
    entries.sort_by(|a, b| a.id.to_lowercase().cmp(&b.id.to_lowercase()));
    Ok(entries)
}

/// Parse `dotnet list package --vulnerable --format json`. The SDK emits
/// Title-Case severity ("Low"/"Moderate"/…) and a lowercase `advisoryurl`
/// field — both normalized here so the panel can treat it like the audit
/// data it renders alongside.
pub fn parse_vulnerable_json(json: &str) -> Result<Vec<VulnerablePackage>, String> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| format!("Failed to parse dotnet list package output: {e}"))?;

    let mut out = Vec::new();
    for project in value
        .get("projects")
        .and_then(|p| p.as_array())
        .into_iter()
        .flatten()
    {
        for fw in project
            .get("frameworks")
            .and_then(|f| f.as_array())
            .into_iter()
            .flatten()
        {
            for pkg in fw
                .get("topLevelPackages")
                .and_then(|p| p.as_array())
                .into_iter()
                .flatten()
            {
                let id = pkg
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                for vuln in pkg
                    .get("vulnerabilities")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                {
                    let severity = vuln
                        .get("severity")
                        .and_then(|s| s.as_str())
                        .unwrap_or("low")
                        .to_lowercase();
                    let advisory_url = vuln
                        .get("advisoryurl")
                        .and_then(|u| u.as_str())
                        .unwrap_or("")
                        .to_string();
                    out.push(VulnerablePackage {
                        id: id.clone(),
                        severity,
                        advisory_url,
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| a.id.to_lowercase().cmp(&b.id.to_lowercase()));
    Ok(out)
}

// ── NuGet registry parsing (pure) ─────────────────────────────────────
//
// The host performs the GETs; these functions turn the JSON bodies into the
// panel-friendly shapes. Endpoints verified against the real service index
// at https://api.nuget.org/v3/index.json.

/// Registration index URL for `id` (lowercased — registry is case-insensitive
/// but the CDN paths are not). Uses `registration5-semver1` (plain JSON,
/// prereleases) rather than the bare `registration5` path, which 404s.
pub fn nuget_registration_url(id: &str) -> String {
    format!(
        "https://api.nuget.org/v3/registration5-semver1/{}/index.json",
        id.to_lowercase()
    )
}

/// Raw README URL for a package/version via the v3-flatcontainer — always
/// returns markdown/text, never nuget.org's HTML gallery page (the
/// catalog's own `readmeUrl` can point at that, which renders terribly).
pub fn nuget_flat_readme_url(id: &str, version: &str) -> String {
    format!(
        "https://api.nuget.org/v3-flatcontainer/{}/{}/readme",
        id.to_lowercase(),
        version.to_lowercase()
    )
}

/// Parse the Azure-hosted search response
/// (`https://azuresearch-usnc.nuget.org/query`): `totalHits` + `data[]`.
pub fn parse_nuget_search(
    json: &serde_json::Value,
) -> Result<(Vec<NugetSearchResult>, u32), String> {
    let total_hits = json.get("totalHits").and_then(|v| v.as_u64()).unwrap_or(0);
    let data = json
        .get("data")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut results = Vec::new();
    for d in data {
        let id = d
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            continue;
        }
        results.push(NugetSearchResult {
            version: d
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            description: d
                .get("description")
                .and_then(|v| v.as_str())
                .map(String::from),
            total_downloads: d
                .get("totalDownloads")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            id,
        });
    }
    Ok((results, total_hits as u32))
}

/// Flatten the `items` of every registration page (the host fetches each
/// page's `@id` when the page has `items` missing / empty — that's the
/// "too many versions to inline" case). Registration pages are ordered
/// oldest-to-newest.
pub fn registration_items_from_pages(pages: &[serde_json::Value]) -> Vec<serde_json::Value> {
    pages
        .iter()
        .flat_map(|p| {
            p.get("items")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default()
        })
        .collect()
}

/// Build [`NugetPackageDetails`] from the flattened catalog (`items`) of a
/// registration — newest-first versions, plus latest-version metadata. The
/// `readme` field is left `None` for the host to fill in (there is no way
/// to know without an extra HTTP request whether the version has one).
pub fn parse_nuget_details(catalog: &[serde_json::Value]) -> Result<NugetPackageDetails, String> {
    let versions: Vec<String> = catalog
        .iter()
        .rev()
        .filter_map(|e| {
            e.get("catalogEntry")?
                .get("version")?
                .as_str()
                .map(String::from)
        })
        .collect();

    let latest_entry = catalog
        .last()
        .and_then(|e| e.get("catalogEntry"))
        .ok_or_else(|| "No version metadata found for this package".to_string())?;

    let id = latest_entry
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let version = latest_entry
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if id.is_empty() || version.is_empty() {
        return Err("Empty package metadata".to_string());
    }

    let dependencies = latest_entry
        .get("dependencyGroups")
        .and_then(|v| v.as_array())
        .map(|groups| {
            groups
                .iter()
                .map(|g| {
                    let target_framework = g
                        .get("targetFramework")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let deps = g
                        .get("dependencies")
                        .and_then(|v| v.as_array())
                        .map(|deps| {
                            deps.iter()
                                .filter_map(|d| {
                                    let dep_id = d.get("id")?.as_str()?.to_string();
                                    Some(NugetDependency {
                                        range: d
                                            .get("range")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string(),
                                        id: dep_id,
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    NugetDependencyGroup {
                        target_framework,
                        dependencies: deps,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let vulnerabilities = latest_entry
        .get("vulnerabilities")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    Some(VulnerablePackage {
                        id: v
                            .get("id")
                            .and_then(|i| i.as_str())
                            .unwrap_or(&id)
                            .to_string(),
                        severity: v.get("severity")?.as_str()?.to_lowercase(),
                        advisory_url: v
                            .get("advisoryUrl")
                            .and_then(|u| u.as_str())
                            .unwrap_or("")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let authors = normalize_authors(latest_entry.get("authors"));

    Ok(NugetPackageDetails {
        version,
        id,
        description: latest_entry
            .get("description")
            .and_then(|v| v.as_str())
            .map(String::from),
        authors,
        project_url: latest_entry
            .get("projectUrl")
            .and_then(|v| v.as_str())
            .map(String::from),
        license_url: latest_entry
            .get("licenseUrl")
            .and_then(|v| v.as_str())
            .map(String::from),
        versions,
        readme: None,
        dependencies,
        vulnerabilities,
    })
}

/// `authors` is a string on most registration entries, but some packages
/// publish it as a JSON array — normalize both to a single comma-joined
/// string.
fn normalize_authors(v: Option<&serde_json::Value>) -> String {
    match v {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|a| a.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        _ => String::new(),
    }
}
