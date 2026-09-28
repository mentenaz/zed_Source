//! Node.js runtime backend — NVM/runtime detection, filesystem helpers, and
//! package.json editing support.
//!
//! No GPUI, no app-state dependency: every function here takes plain
//! strings/paths and returns a plain value or `Result` — a host panel wires
//! this into its own UI/state, this crate just does the work.

use std::path::Path;

use serde::Serialize;

mod path_env;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// ── Types ─────────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
}

#[derive(Serialize, Clone)]
pub struct NvmVersion {
    pub version: String,
    pub current: bool,
}

/// A directory that looks like a Node project: it directly contains a
/// `package.json`. The scanner walks `scan_node_projects` and reports these.
#[derive(Serialize, Clone, Debug)]
pub struct DetectedNodeProject {
    pub path: String,
    pub name: String,
}

// ── Filesystem ────────────────────────────────────────────────────────

pub fn read_text_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| format!("Cannot read {path}: {e}"))
}

pub fn write_file(path: String, content: String) -> Result<(), String> {
    // Create the parent directory if it doesn't exist yet — without this,
    // writing into a not-yet-existing directory fails outright.
    if let Some(parent) = Path::new(&path).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Cannot create directory for {path}: {e}"))?;
    }
    std::fs::write(&path, content).map_err(|e| format!("Cannot write {path}: {e}"))
}

/// Sends a path to the OS file manager. On Windows this opens Explorer with
/// the file selected (`explorer /select,<path>`); the `CREATE_NO_WINDOW` flag
/// prevents a console flash. On other platforms it opens the parent
/// directory.
pub fn reveal_in_explorer(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        use std::process::Command;
        let parent = Path::new(&path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or(path.clone());
        let dir = dunce::canonicalize(&parent).map_err(|e| format!("{e}"))?;
        let args = if Path::new(&path).is_dir() {
            vec![dir.to_string_lossy().into_owned()]
        } else {
            vec!["/select,".to_string() + &path]
        };
        Command::new("explorer")
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Cannot open Explorer: {e}"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        use std::process::Command;
        let parent = Path::new(&path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or(path);
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        Command::new(opener)
            .arg(parent)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Cannot open file manager: {e}"))
    }
}

pub fn read_dir(path: String) -> Result<Vec<DirEntry>, String> {
    let entries = std::fs::read_dir(&path).map_err(|e| format!("Cannot read {path}: {e}"))?;
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // `<name>.flow.layout.json` is a hidden companion to `<name>.flow.json`,
        // so it's excluded here rather than in just the Explorer: every panel
        // that lists a directory goes through this same function and the
        // sidecar shouldn't turn up as a stray row.
        if name.ends_with(".flow.layout.json") {
            continue;
        }
        let ft = entry.file_type().map_err(|e| e.to_string())?;
        out.push(DirEntry {
            name,
            path: entry.path().to_string_lossy().into_owned(),
            is_dir: ft.is_dir(),
            is_symlink: ft.is_symlink(),
        });
    }
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

// ── Project scanning ──────────────────────────────────────────────────

/// Maximum depth the node-project scanner descends from the workspace root.
pub const MAX_SCAN_DEPTH: usize = 5;
/// Directories never descended into while scanning for node projects.
pub const SCAN_SKIP_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    ".forge",
    "dist",
    "build",
    "target",
    ".next",
    "coverage",
    ".cache",
    "out",
    ".turbo",
];

/// Walks `root` (to [`MAX_SCAN_DEPTH`], skipping [`SCAN_SKIP_DIRS`]) and
/// reports every directory that directly contains a `package.json`. Stops
/// descending once a `.git` directory is found, treating that as a project
/// boundary.
pub fn scan_node_projects(root: &str, depth: usize) -> Vec<DetectedNodeProject> {
    if depth > MAX_SCAN_DEPTH {
        return Vec::new();
    }
    let entries = match read_dir(root.to_string()) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut projects = Vec::new();

    if entries
        .iter()
        .any(|e| e.name.to_lowercase() == "package.json")
    {
        let name = std::path::Path::new(root)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "main".into());
        projects.push(DetectedNodeProject {
            path: root.to_string(),
            name,
        });
    }

    if depth > 0 && entries.iter().any(|e| e.name == ".git" && e.is_dir) {
        return projects;
    }

    for entry in &entries {
        if !entry.is_dir {
            continue;
        }
        if SCAN_SKIP_DIRS.contains(&entry.name.as_str()) {
            continue;
        }
        let sub = scan_node_projects(&entry.path, depth + 1);
        projects.extend(sub);
    }

    projects
}

// ── Runtime queries ───────────────────────────────────────────────────

/// Run a command that might be a `.cmd` shim, returning its captured stdout
/// (or stderr when stdout is empty). Falls back to `cmd /C` on Windows so
/// npm-installed `.cmd` CLIs resolve through PATHEXT.
fn run_captured(exe: &str, args: &[&str]) -> Result<String, String> {
    let mut c = gpui_util::new_std_command(exe);
    c.args(args);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(CREATE_NO_WINDOW);
        path_env::enrich_path(&mut c);
    }
    let out = match c.output() {
        Ok(out) => out,
        Err(e) => {
            log::debug!("run_captured: direct '{exe}' failed: {e}");
            #[cfg(target_os = "windows")]
            {
                let mut sh = gpui_util::new_std_command("cmd");
                sh.args(["/C", exe]);
                sh.args(args);
                use std::os::windows::process::CommandExt;
                sh.creation_flags(CREATE_NO_WINDOW);
                path_env::enrich_path(&mut sh);
                match sh.output() {
                    Ok(fb) => {
                        log::debug!(
                            "run_captured: cmd /C '{exe}' status={} stdout={} stderr={}",
                            fb.status,
                            String::from_utf8_lossy(&fb.stdout).trim(),
                            String::from_utf8_lossy(&fb.stderr).trim(),
                        );
                        fb
                    }
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

/// Query `exe --version`.
pub fn query_runtime(exe: String) -> Result<String, String> {
    run_captured(&exe, &["--version"])
}

/// List installed NVM Node versions (`nvm list`), marking the current one.
pub fn nvm_list() -> Result<Vec<NvmVersion>, String> {
    let stdout = run_captured("nvm", &["list"])?;
    let versions = stdout
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            let current = trimmed.starts_with('*');
            let rest = trimmed.trim_start_matches('*').trim();
            let version = rest.split_whitespace().next()?;
            if version.chars().next()?.is_ascii_digit() {
                Some(NvmVersion {
                    version: version.to_string(),
                    current,
                })
            } else {
                None
            }
        })
        .collect();
    Ok(versions)
}

/// List NVM versions available to install (`nvm list available`).
pub fn nvm_list_available() -> Result<Vec<String>, String> {
    let stdout = run_captured("nvm", &["list", "available"])?;
    let mut versions: Vec<String> = Vec::new();
    for line in stdout.lines() {
        if !line.contains('|') {
            continue;
        }
        for cell in line.split('|') {
            let v = cell.trim();
            if v.chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false)
            {
                versions.push(v.to_string());
            }
        }
    }
    Ok(versions)
}

/// Switch NVM to a specific version (`nvm use <version>`).
pub fn nvm_use(version: &str) -> Result<String, String> {
    run_captured("nvm", &["use", version])
}

/// Install an NVM version (`nvm install <version>`).
pub fn nvm_install(version: &str) -> Result<String, String> {
    run_captured("nvm", &["install", version])
}

/// Uninstall an NVM version (`nvm uninstall <version>`). The version must be
/// an exact, already-installed version — no "latest"/"lts" aliases.
pub fn nvm_uninstall(version: &str) -> Result<String, String> {
    run_captured("nvm", &["uninstall", version])
}
