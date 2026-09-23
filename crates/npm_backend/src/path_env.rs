//! Windows PATH enrichment for spawned npm/yarn/pnpm/bun processes.
//!
//! GUI-launched apps inherit a stripped-down PATH; a spawned package manager
//! needs the real user PATH (registry `Environment` values) AND the active
//! Node runtime directory so the CLI's own scripts can resolve `node`.
//! [`merged_path`] handles the registry half (same logic as `node_backend`),
//! and [`path_with_node`] additionally prepends the resolved runtime dir.

#[cfg(target_os = "windows")]
use windows_registry::{CURRENT_USER, LOCAL_MACHINE};

#[cfg(target_os = "windows")]
fn expand_env_vars(input: &str) -> String {
    let mut result = input.to_string();
    let mut start = 0;
    while let Some(open) = result[start..].find('%') {
        let open = start + open;
        let rest = &result[open + 1..];
        let Some(close_rel) = rest.find('%') else {
            break;
        };
        let close = open + 1 + close_rel;
        let name = &result[open + 1..close];
        if let Ok(value) = std::env::var(name) {
            result.replace_range(open..close + 1, &value);
            start = open + value.len();
        } else {
            start = close + 1;
        }
    }
    result
}

#[cfg(target_os = "windows")]
fn read_env_path(key: &windows_registry::Key) -> Option<String> {
    let val = key.get_string("Path").ok()?;
    if val.is_empty() { return None; }
    Some(expand_env_vars(&val))
}

#[cfg(target_os = "windows")]
fn dedupe_path(path: &str) -> String {
    let mut seen: Vec<String> = Vec::new();
    let mut out: Vec<String> = Vec::new();
    for part in path.split(';') {
        let part = part.trim();
        if part.is_empty() { continue; }
        let key = part.to_lowercase();
        if !seen.contains(&key) {
            seen.push(key);
            out.push(part.to_string());
        }
    }
    out.join(";")
}

#[cfg(target_os = "windows")]
pub fn merged_path() -> Option<String> {
    let current = std::env::var("PATH").unwrap_or_default();
    let mut parts = vec![current];

    match CURRENT_USER.open("Environment") {
        Ok(env_key) => {
            if let Some(val) = read_env_path(&env_key) {
                parts.push(val);
            } else {
                log::debug!("npm_backend: HKCU Path missing");
            }
        }
        Err(_) => log::debug!("npm_backend: cannot open HKCU Environment"),
    }

    match LOCAL_MACHINE
        .open(r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment")
    {
        Ok(env_key) => {
            if let Some(val) = read_env_path(&env_key) {
                parts.push(val);
            } else {
                log::debug!("npm_backend: HKLM Path missing");
            }
        }
        Err(_) => log::debug!("npm_backend: cannot open HKLM Environment"),
    }

    let merged = dedupe_path(&parts.join(";"));
    if merged.is_empty() { None } else { Some(merged) }
}

#[cfg(not(target_os = "windows"))]
pub fn merged_path() -> Option<String> {
    None
}

/// Resolves the active Node runtime directory to prepend to PATH: the
/// `%NVM_HOME%\current` symlink when nvm-windows is in play, otherwise the
/// parent of whatever `node` the merged PATH already points at.
#[cfg(target_os = "windows")]
fn node_runtime_dir() -> Option<String> {
    let nvm_home = std::env::var("NVM_HOME").ok()?;
    let candidate = std::path::Path::new(&nvm_home).join("current");
    if candidate.is_dir() {
        return Some(candidate.to_string_lossy().into_owned());
    }
    None
}

#[cfg(not(target_os = "windows"))]
fn node_runtime_dir() -> Option<String> {
    None
}

/// The PATH value to hand a spawned package manager: merged registry path,
/// with the active Node runtime dir prepended so `npm run` children and the
/// CLI's shell scripts can find `node`.
pub fn path_with_node(project_root: &str) -> String {
    let base = merged_path().unwrap_or_else(|| std::env::var("PATH").unwrap_or_default());
    let mut parts = Vec::new();
    if let Some(dir) = node_runtime_dir() {
        parts.push(dir);
    }
    parts.push(base);
    // Also allow a local node_modules/.bin for lifecycle scripts.
    let local_bin = std::path::Path::new(project_root).join("node_modules").join(".bin");
    if local_bin.exists() {
        parts.push(local_bin.to_string_lossy().into_owned());
    }
    let joined = parts.join(";");
    log::debug!("npm_backend: enriched PATH = {joined}");
    joined
}