//! Python runtime backend — environment detection, project scanning, dependency
//! checking, framework detection, and package management.
//!
//! No GPUI, no app-state dependency: every function here takes plain
//! strings/paths and returns a plain value or `Result` — a host panel wires
//! this into its own UI/state, this crate just does the work.

use std::path::Path;

use serde::{Deserialize, Serialize};

mod path_env;

// ── Types ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PythonPackage {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PythonFramework {
    pub name: String,
    pub icon: String,
    pub dev_command: String,
    pub port: Option<u16>,
}

/// Package metadata fetched from the PyPI JSON API
/// (`https://pypi.org/pypi/<name>/json` or `/<name>/<version>/json`).
#[derive(Clone, Debug, Default)]
pub struct PyPiPackageInfo {
    pub name: String,
    pub version: String,
    pub summary: Option<String>,
    /// `info.description` — usually the full README content.
    pub readme: Option<String>,
    /// `info.description_content_type` — e.g. `text/markdown`, `text/x-rst`.
    pub readme_content_type: Option<String>,
    pub author: Option<String>,
    pub license: Option<String>,
    pub home_page: Option<String>,
    pub project_urls: Vec<(String, String)>,
    pub requires_python: Option<String>,
    pub requires_dist: Vec<String>,
    /// Python versions this release supports, parsed out of `classifiers`
    /// (e.g. "Programming Language :: Python :: 3.11").
    pub python_versions: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct PythonProjectScan {
    pub venvs: Vec<String>,
    pub requirements: Vec<String>,
    pub entry_points: Vec<(String, String)>, // (file, dir)
}

pub const FRAMEWORK_MARKERS: &[(&str, &str, &str, Option<u16>)] = &[
    ("flask", "Flask", "flask run --debug", Some(5000)),
    ("fastapi", "FastAPI", "uvicorn main:app --reload", Some(8000)),
    ("django", "Django", "python manage.py runserver 0.0.0.0:8000", Some(8000)),
    ("streamlit", "Streamlit", "streamlit run app.py", Some(8501)),
    ("gradio", "Gradio", "python app.py", Some(7860)),
    ("pytest", "pytest", "python -m pytest -v", None),
];

pub const PYTHON_ENTRY_POINTS: &[&str] = &[
    "app.py",
    "main.py",
    "run.py",
    "server.py",
    "manage.py",
    "wsgi.py",
    "asgi.py",
];

const SCAN_SKIP_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    ".forge",
    "dist",
    "build",
    "target",
    "__pycache__",
    ".next",
    "coverage",
    ".cache",
    "out",
    "venv",
    "env",
    ".venv",
];

const MAX_SCAN_DEPTH: usize = 5;

// ── Runtime queries ───────────────────────────────────────────────────

/// Run a command and return stdout (or stderr if stdout is empty).
fn run_captured(exe: &str, args: &[&str]) -> Result<String, String> {
    let mut c = gpui_util::new_std_command(exe);
    c.args(args);
    #[cfg(target_os = "windows")]
    path_env::enrich_path(&mut c);
    let out = match c.output() {
        Ok(out) => out,
        Err(e) => {
            log::debug!("run_captured: direct '{exe}' failed: {e}");
            #[cfg(target_os = "windows")]
            {
                let mut sh = gpui_util::new_std_command("cmd");
                sh.args(["/C", exe]);
                sh.args(args);
                path_env::enrich_path(&mut sh);
                match sh.output() {
                    Ok(fb) => fb,
                    Err(e2) => {
                        log::debug!("run_captured: cmd /C '{exe}' also failed: {e2}");
                        return Err(format!("{exe} not found"));
                    }
                }
            }
            #[cfg(not(target_os = "windows"))]
            return Err(format!("{exe} not found"));
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Ok(if stdout.is_empty() { stderr } else { stdout })
}

/// Query Python version with `python --version`.
pub fn query_python(exe: &str) -> Result<String, String> {
    run_captured(exe, &["--version"])
}

/// Query pip version with `python -m pip --version`.
pub fn query_pip(exe: &str) -> Result<String, String> {
    run_captured(exe, &["-m", "pip", "--version"])
}

/// List installed packages as JSON: `python -m pip list --format json`.
pub fn list_packages(exe: &str) -> Result<Vec<PythonPackage>, String> {
    let output = run_captured(exe, &["-m", "pip", "list", "--format", "json"])?;
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&output)
        .map_err(|e| format!("Failed to parse pip output: {e}"))?;

    let packages = parsed
        .iter()
        .filter_map(|p| {
            let name = p.get("name")?.as_str()?;
            let version = p.get("version")?.as_str()?;
            Some(PythonPackage {
                name: name.to_lowercase(),
                version: version.to_string(),
            })
        })
        .collect();

    Ok(packages)
}

/// Which part of a package's version moved between the installed and latest
/// releases — drives the severity badge on Outdated/Updates sections.
/// Mirrors `dotnet_backend::UpdateKind` (same three-tier logic, same
/// "unparseable or equal defaults to `Patch`" fallback) so both ecosystems
/// classify risk identically.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
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

/// Parses a version string's leading `major.minor.patch` numeric run,
/// ignoring anything after the first non-numeric/non-dot segment (pre-release
/// suffixes like `2.0.0rc1`, PEP 440 local versions like `1.0+local`, etc.).
fn version_parts(v: &str) -> Option<(u64, u64, u64)> {
    let mut parts = v
        .split(|c: char| c == '.' || c == '-' || c == '+')
        .map(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>())
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.parse::<u64>().ok());
    let major = parts.next()?;
    let minor = parts.next().unwrap_or(0);
    let patch = parts.next().unwrap_or(0);
    Some((major, minor, patch))
}

/// Classifies an installed→latest version jump. Matches
/// `dotnet_backend::classify_update`'s exact fallback: unparseable or equal
/// versions default to `Patch` (the least alarming classification), not
/// `Major` — since a version pip already reports as "outdated" but this
/// parser can't compare shouldn't read as a major-risk change by default.
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

/// One row from `pip list --outdated --format json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PythonOutdatedPkg {
    pub name: String,
    pub version: String,
    pub latest_version: String,
    pub kind: UpdateKind,
}

/// List outdated installed packages: `python -m pip list --outdated --format json`.
pub fn list_outdated(exe: &str) -> Result<Vec<PythonOutdatedPkg>, String> {
    let output = run_captured(exe, &["-m", "pip", "list", "--outdated", "--format", "json"])?;
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&output)
        .map_err(|e| format!("Failed to parse pip output: {e}"))?;

    let packages = parsed
        .iter()
        .filter_map(|p| {
            let name = p.get("name")?.as_str()?;
            let version = p.get("version")?.as_str()?;
            let latest_version = p.get("latest_version")?.as_str()?;
            Some(PythonOutdatedPkg {
                name: name.to_lowercase(),
                version: version.to_string(),
                latest_version: latest_version.to_string(),
                kind: classify_update(version, latest_version),
            })
        })
        .collect();

    Ok(packages)
}

// ── PyPI metadata ──────────────────────────────────────────────────────

/// Parses a `https://pypi.org/pypi/<name>/json` response body into
/// [`PyPiPackageInfo`]. Pure parsing — the HTTP fetch itself is the caller's
/// job (it needs an async HTTP client, which is a UI/app-layer concern this
/// crate deliberately has no opinion on).
pub fn parse_pypi_json(body: &serde_json::Value) -> Result<PyPiPackageInfo, String> {
    let info = body
        .get("info")
        .ok_or_else(|| "missing 'info' in PyPI response".to_string())?;

    let name = info
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing 'info.name'".to_string())?
        .to_string();
    let version = info
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let non_empty = |v: Option<&str>| v.map(str::trim).filter(|s| !s.is_empty()).map(String::from);

    let summary = non_empty(info.get("summary").and_then(|v| v.as_str()));
    let readme = non_empty(info.get("description").and_then(|v| v.as_str()));
    let readme_content_type = non_empty(info.get("description_content_type").and_then(|v| v.as_str()));
    let author = non_empty(info.get("author").and_then(|v| v.as_str()));
    let license = non_empty(info.get("license").and_then(|v| v.as_str()));
    let home_page = non_empty(info.get("home_page").and_then(|v| v.as_str()))
        .or_else(|| non_empty(info.get("project_url").and_then(|v| v.as_str())));
    let requires_python = non_empty(info.get("requires_python").and_then(|v| v.as_str()));

    let project_urls = info
        .get("project_urls")
        .and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();

    let requires_dist = info
        .get("requires_dist")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let python_versions = info
        .get("classifiers")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .filter_map(|s| s.strip_prefix("Programming Language :: Python :: "))
                .filter(|s| *s != "3" && *s != "2" && !s.starts_with("Implementation"))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();

    Ok(PyPiPackageInfo {
        name,
        version,
        summary,
        readme,
        readme_content_type,
        author,
        license,
        home_page,
        project_urls,
        requires_python,
        requires_dist,
        python_versions,
    })
}

/// One entry from a `https://pypi.org/pypi/<name>/<version>/json` response's
/// top-level `vulnerabilities` array — PyPI surfaces these straight from
/// OSV.dev (https://osv.dev) for the *exact* release requested, so unlike
/// `PyPiPackageInfo` (which comes off the version-less `/pypi/<name>/json`
/// endpoint and only ever describes the latest release) this has to be
/// fetched per installed package at its installed version.
#[derive(Clone, Debug, Default)]
pub struct PyPiVulnerability {
    /// OSV/PyPI advisory id, e.g. "PYSEC-2023-74" or "GHSA-...".
    pub id: String,
    pub summary: Option<String>,
    pub details: Option<String>,
    /// Other identifiers for the same advisory, e.g. "CVE-2023-32681".
    pub aliases: Vec<String>,
    /// Versions this vulnerability is fixed in, if known.
    pub fixed_in: Vec<String>,
    pub link: Option<String>,
    pub source: Option<String>,
}

/// Parses the `vulnerabilities` array out of a per-version PyPI JSON
/// response. Withdrawn advisories (later retracted by OSV) are dropped.
pub fn parse_pypi_vulnerabilities(body: &serde_json::Value) -> Vec<PyPiVulnerability> {
    let non_empty = |v: Option<&str>| v.map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    let str_list = |v: &serde_json::Value, key: &str| -> Vec<String> {
        v.get(key)
            .and_then(|a| a.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default()
    };

    body.get("vulnerabilities")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|v| v.get("withdrawn").map(|w| w.is_null()).unwrap_or(true))
                .filter_map(|v| {
                    let id = v.get("id")?.as_str()?.to_string();
                    Some(PyPiVulnerability {
                        id,
                        summary: non_empty(v.get("summary").and_then(|s| s.as_str())),
                        details: non_empty(v.get("details").and_then(|s| s.as_str())),
                        aliases: str_list(v, "aliases"),
                        fixed_in: str_list(v, "fixed_in"),
                        link: non_empty(v.get("link").and_then(|s| s.as_str())),
                        source: non_empty(v.get("source").and_then(|s| s.as_str())),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

// ── Multi-project detection ─────────────────────────────────────────────

/// Which dependency-declaration file a detected project uses to define its
/// environment. Checked in this priority order when a project has more than
/// one — `requirements.txt` wins because it's the simplest, most explicit
/// case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvMarker {
    Requirements,
    Pyproject,
    Pipfile,
}

impl EnvMarker {
    pub fn file_name(self) -> &'static str {
        match self {
            EnvMarker::Requirements => "requirements.txt",
            EnvMarker::Pyproject => "pyproject.toml",
            EnvMarker::Pipfile => "Pipfile",
        }
    }

    /// The command that bootstraps a venv (or, for Pipfile, a pipenv-managed
    /// environment) from this marker file, run from the project directory
    /// with `exe` as the interpreter used to create the venv.
    pub fn create_command(self, exe: &str) -> String {
        match self {
            EnvMarker::Requirements => format!(
                "{exe} -m venv venv && venv\\Scripts\\python.exe -m pip install -r requirements.txt"
            ),
            // `pip install -e .` reads build-system deps straight out of
            // pyproject.toml without requiring Poetry/PDM/etc. to be
            // installed — works for any PEP 517 backend.
            EnvMarker::Pyproject => {
                format!("{exe} -m venv venv && venv\\Scripts\\python.exe -m pip install -e .")
            }
            // Pipenv manages its own venv (not this project's `venv\`
            // folder) — there's nothing else to scope pip/venv lookups to
            // afterwards, but it's still the correct one-command bootstrap
            // for a Pipfile.
            EnvMarker::Pipfile => format!("{exe} -m pip install pipenv && {exe} -m pipenv install"),
        }
    }
}

/// A Python sub-project detected inside the workspace: any directory that
/// declares its dependencies via `requirements.txt`, `pyproject.toml`, or
/// `Pipfile`. Mirrors `NpmProject`/`node_backend::DetectedNodeProject` — lets
/// a monorepo-style workspace pick which project's venv/pip commands a panel
/// operates on, instead of guessing from a single workspace-wide scan.
/// Shared by `python_manager_panel` (its project switcher) and `python_panel`
/// (its Projects section) — moved here from `python_manager_panel` so both
/// can use one scanner instead of two.
#[derive(Clone, Debug)]
pub struct DetectedPythonProject {
    pub path: String,
    pub name: String,
    pub marker: EnvMarker,
}

const PROJECT_SCAN_SKIP_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "dist",
    "build",
    "target",
    "__pycache__",
    ".next",
    "coverage",
    ".cache",
    "out",
    "venv",
    "env",
    ".venv",
];
const PROJECT_SCAN_MAX_DEPTH: usize = 5;

pub fn scan_python_projects(root: &str, depth: usize) -> Vec<DetectedPythonProject> {
    if depth > PROJECT_SCAN_MAX_DEPTH {
        return Vec::new();
    }
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let entries: Vec<_> = entries.flatten().collect();

    let mut projects = Vec::new();

    let has = |file: &str| {
        entries
            .iter()
            .any(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(file))
    };
    let marker = if has("requirements.txt") {
        Some(EnvMarker::Requirements)
    } else if has("pyproject.toml") {
        Some(EnvMarker::Pyproject)
    } else if has("Pipfile") {
        Some(EnvMarker::Pipfile)
    } else {
        None
    };
    if let Some(marker) = marker {
        let name = Path::new(root)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.to_string());
        projects.push(DetectedPythonProject {
            path: root.to_string(),
            name,
            marker,
        });
    }

    for entry in &entries {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let dir_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if PROJECT_SCAN_SKIP_DIRS
            .iter()
            .any(|skip| *skip == dir_name.to_lowercase())
        {
            continue;
        }
        projects.extend(scan_python_projects(&path.to_string_lossy(), depth + 1));
    }

    projects
}

// ── Project scanning ───────────────────────────────────────────────────

pub fn scan_python_project(root: &str) -> Result<PythonProjectScan, String> {
    let mut result = PythonProjectScan {
        venvs: Vec::new(),
        requirements: Vec::new(),
        entry_points: Vec::new(),
    };
    walk(Path::new(root), 0, &mut result)?;
    Ok(result)
}

fn walk(dir: &Path, depth: usize, out: &mut PythonProjectScan) -> Result<(), String> {
    if depth > MAX_SCAN_DEPTH {
        return Ok(());
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_lowercase())
                .unwrap_or_default();

            // Check for venv/env directories
            if name == "venv" || name == "env" || name == ".venv" {
                for exe in &[
                    format!("{}\\Scripts\\python.exe", path.display()),
                    format!("{}/bin/python", path.display()),
                ] {
                    if Path::new(exe).exists() {
                        out.venvs.push(exe.clone());
                    }
                }
                // Don't recurse into venv directories
                continue;
            }

            if !SCAN_SKIP_DIRS.iter().any(|skip| skip == &name.as_str()) {
                walk(&path, depth + 1, out)?;
            }
        } else if path.is_file() {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();

            if name == "requirements.txt" {
                out.requirements.push(path.to_string_lossy().into_owned());
            } else if PYTHON_ENTRY_POINTS.iter().any(|ep| ep == &name) {
                let dir = path
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                out.entry_points.push((path.to_string_lossy().into_owned(), dir));
            }
        }
    }

    Ok(())
}

// ── Framework detection ────────────────────────────────────────────────

pub fn detect_framework(
    requirements: &[String],
    entry_points: &[(String, String)],
) -> Option<PythonFramework> {
    // Read requirements.txt files
    let mut req_content = String::new();
    for req_path in requirements {
        if let Ok(content) = std::fs::read_to_string(req_path) {
            req_content.push(' ');
            req_content.push_str(&content.to_lowercase());
        }
    }

    // Get entry point names
    let entry_names: Vec<String> = entry_points
        .iter()
        .map(|(file, _)| {
            Path::new(file)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_lowercase()
        })
        .collect();

    let search_str = format!("{} {}", req_content, entry_names.join(" "));

    // Check for framework markers
    for (marker, name, dev_cmd, port) in FRAMEWORK_MARKERS {
        if search_str.contains(marker) {
            return Some(PythonFramework {
                name: name.to_string(),
                icon: match *name {
                    "Flask" => "🌶",
                    "FastAPI" => "⚡",
                    "Django" => "🎸",
                    "Streamlit" => "🎈",
                    "Gradio" => "🤗",
                    "pytest" => "🧪",
                    _ => "🐍",
                }
                .to_string(),
                dev_command: dev_cmd.to_string(),
                port: *port,
            });
        }
    }

    None
}

/// Find missing dependencies by comparing requirements.txt with installed packages.
pub fn find_missing_deps(
    requirements_path: Option<&str>,
    installed: &[PythonPackage],
) -> Result<Vec<String>, String> {
    if let Some(path) = requirements_path {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read requirements.txt: {e}"))?;

        let installed_names: std::collections::HashSet<_> =
            installed.iter().map(|p| p.name.as_str()).collect();

        let missing: Vec<String> = content
            .lines()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(|line| {
                // Parse package name (before any operators like ==, >=, <, etc.)
                line.split(|c: char| c == '<' || c == '>' || c == '=' || c == '~' || c == '!')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_lowercase()
            })
            .filter(|pkg| !pkg.is_empty() && !installed_names.contains(pkg.as_str()))
            .collect();

        Ok(missing)
    } else {
        Ok(Vec::new())
    }
}

/// Count dependencies from requirements.txt.
pub fn count_requirements(path: Option<&str>) -> Result<usize, String> {
    if let Some(p) = path {
        let content = std::fs::read_to_string(p)
            .map_err(|e| format!("Failed to read requirements.txt: {e}"))?;
        let count = content
            .lines()
            .filter(|line| {
                let trimmed = line.trim();
                !trimmed.is_empty() && !trimmed.starts_with('#')
            })
            .count();
        Ok(count)
    } else {
        Ok(0)
    }
}
