//! Imports settings and data from this fork's earlier builds, which kept them
//! in folders named `ZedDev`, into this app's own folders.
//!
//! The import runs at startup, before anything opens the settings or the
//! database: automatically on the first launch (when this app's folders do
//! not exist yet), and on the launch after the "Import Data from Previous
//! Version" action asked for it. The old folders are only read, never changed,
//! and upstream Zed's own `Zed` folders are never touched.

use std::{
    fmt::Write as _,
    fs, io,
    path::{Path, PathBuf},
};

use gpui::{App, PromptLevel, Window};
use workspace::Workspace;

/// The folder name earlier builds of this fork used for their settings and
/// data.
const PREVIOUS_APP_NAME: &str = "ZedDev";

/// Written to the data folder by [`ImportDataFromPreviousVersion`]; the next
/// launch imports and deletes it.
const PENDING_FILE: &str = "import-previous-data.pending";

/// Where entries this app already had are moved when an import replaces
/// them, inside the data folder.
const BACKUP_DIR: &str = "before-import";

/// Entries of the old data folder that are left behind: logs, and caches the
/// app downloads again when it needs them.
const SKIPPED_DATA_ENTRIES: &[&str] = &[
    "logs",
    "hang_traces",
    "languages",
    "node",
    "prettier",
    "debug_adapters",
    "external_agents",
    "copilot",
    "remote_servers",
    "tmp",
    "temp",
    PENDING_FILE,
    BACKUP_DIR,
];

/// Parts of `extensions` that are rebuilt or downloaded again: dev-extension
/// build output and language servers, together gigabytes.
const SKIPPED_EXTENSION_ENTRIES: &[&str] = &["build", "work", "staging"];

gpui::actions!(
    zed,
    [
        /// Imports settings, recent projects, extensions and agent history from
        /// the previous version (the `ZedDev` folders), then restarts.
        ImportDataFromPreviousVersion
    ]
);

/// The old config and data folders.
struct PreviousFolders {
    config: PathBuf,
    data: PathBuf,
}

/// The previous version's folders, if any exist and they are not this app's
/// own.
fn previous_folders() -> Option<PreviousFolders> {
    let lowercase = PREVIOUS_APP_NAME.to_ascii_lowercase();
    let (config, data) = if cfg!(target_os = "windows") {
        (
            dirs::config_dir()?.join(PREVIOUS_APP_NAME),
            dirs::data_local_dir()?.join(PREVIOUS_APP_NAME),
        )
    } else if cfg!(target_os = "macos") {
        let home = paths::home_dir();
        (
            home.join(".config").join(&lowercase),
            home.join("Library/Application Support")
                .join(PREVIOUS_APP_NAME),
        )
    } else {
        (
            dirs::config_dir()?.join(&lowercase),
            dirs::data_local_dir()?.join(&lowercase),
        )
    };
    if config == *paths::config_dir() || data == *paths::data_dir() {
        return None;
    }
    (config.is_dir() || data.is_dir()).then_some(PreviousFolders { config, data })
}

/// Runs the import when it is due. Call before anything reads the config or
/// data folders, and only for the default folders (not `--user-data-dir`).
///
/// Returns what happened, to log once logging is set up.
pub fn import_if_due() -> Option<String> {
    let data_dir = paths::data_dir();
    let config_dir = paths::config_dir();
    let pending = data_dir.join(PENDING_FILE);
    let requested = pending.exists();
    let first_launch = !data_dir.exists() && !config_dir.exists();
    if !requested && !first_launch {
        return None;
    }
    if requested {
        fs::remove_file(&pending).ok();
    }
    let Some(previous) = previous_folders() else {
        return requested.then(|| {
            format!("import requested, but no {PREVIOUS_APP_NAME} folders were found")
        });
    };

    let mut report = Report::default();
    let backup = data_dir.join(BACKUP_DIR);
    if previous.config.is_dir() {
        import_folder(
            &previous.config,
            config_dir,
            &[],
            &backup.join("config"),
            &mut report,
        );
    }
    if previous.data.is_dir() {
        import_folder(
            &previous.data,
            data_dir,
            SKIPPED_DATA_ENTRIES,
            &backup.join("data"),
            &mut report,
        );
    }
    Some(report.summary(&previous))
}

#[derive(Default)]
struct Report {
    imported: Vec<String>,
    backed_up: Vec<String>,
    failed: Vec<String>,
}

impl Report {
    fn summary(&self, previous: &PreviousFolders) -> String {
        let mut summary = format!(
            "imported from {} and {}: {}",
            previous.config.display(),
            previous.data.display(),
            if self.imported.is_empty() {
                "nothing".to_string()
            } else {
                self.imported.join(", ")
            }
        );
        if !self.backed_up.is_empty() {
            write!(
                summary,
                "; replaced entries moved to {BACKUP_DIR}: {}",
                self.backed_up.join(", ")
            )
            .ok();
        }
        if !self.failed.is_empty() {
            write!(summary, "; failed: {}", self.failed.join("; ")).ok();
        }
        summary
    }
}

/// Copies each top-level entry of `from` into `to`, except `skipped`. An entry
/// `to` already has is moved into `backup` first.
fn import_folder(from: &Path, to: &Path, skipped: &[&str], backup: &Path, report: &mut Report) {
    let entries = match fs::read_dir(from) {
        Ok(entries) => entries,
        Err(error) => {
            report.failed.push(format!("{}: {error}", from.display()));
            return;
        }
    };
    if let Err(error) = fs::create_dir_all(to) {
        report.failed.push(format!("{}: {error}", to.display()));
        return;
    }
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if skipped.contains(&name_str.as_ref()) || name_str.starts_with("zed-crash-handler") {
            continue;
        }
        let target = to.join(&name);
        if fs::symlink_metadata(&target).is_ok() {
            let moved = fs::create_dir_all(backup)
                .and_then(|()| fs::rename(&target, backup.join(&name)));
            if let Err(error) = moved {
                report
                    .failed
                    .push(format!("{name_str}: could not move the existing one aside: {error}"));
                continue;
            }
            report.backed_up.push(name_str.to_string());
        }
        let only_some = (name_str == "extensions").then_some(SKIPPED_EXTENSION_ENTRIES);
        match copy_tree(&entry.path(), &target, from, to, only_some.unwrap_or(&[])) {
            Ok(()) => report.imported.push(name_str.to_string()),
            Err(error) => report.failed.push(format!("{name_str}: {error}")),
        }
    }
}

/// Copies `from` to `to` recursively, leaving out the direct children named in
/// `skipped`. Links are re-created rather than followed; a link into
/// `source_root` is pointed at the same place under `dest_root`, so a copied
/// junction to an installed Node version points at the copy.
fn copy_tree(
    from: &Path,
    to: &Path,
    source_root: &Path,
    dest_root: &Path,
    skipped: &[&str],
) -> io::Result<()> {
    let mut links = Vec::new();
    copy_recursive(from, to, skipped, &mut links)?;
    // Made last, so a link's target has been copied before the link exists.
    for (link, target) in links {
        let target = match target.strip_prefix(source_root) {
            Ok(rest) => dest_root.join(rest),
            Err(_) => target,
        };
        create_link(&target, &link)?;
    }
    Ok(())
}

fn copy_recursive(
    from: &Path,
    to: &Path,
    skipped: &[&str],
    links: &mut Vec<(PathBuf, PathBuf)>,
) -> io::Result<()> {
    let file_type = fs::symlink_metadata(from)?.file_type();
    if file_type.is_symlink() {
        links.push((to.to_path_buf(), without_verbatim_prefix(fs::read_link(from)?)));
    } else if file_type.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            if skipped.contains(&entry.file_name().to_string_lossy().as_ref()) {
                continue;
            }
            copy_recursive(&entry.path(), &to.join(entry.file_name()), &[], links)?;
        }
    } else {
        fs::copy(from, to)?;
    }
    Ok(())
}

/// Windows reports link targets as `\\?\C:\…` (or `\??\C:\…` for junctions);
/// the plain form is needed to compare them with ordinary paths.
fn without_verbatim_prefix(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text
        .strip_prefix(r"\\?\")
        .or_else(|| text.strip_prefix(r"\??\"))
    {
        Some(rest) => PathBuf::from(rest),
        None => path,
    }
}

#[cfg(target_os = "windows")]
fn create_link(target: &Path, link: &Path) -> io::Result<()> {
    if target.is_dir() {
        // A junction, like the Node and Python version managers make; unlike
        // a symlink it needs no special rights.
        junction::create(target, link)
    } else {
        fs::copy(target, link).map(|_| ())
    }
}

#[cfg(not(target_os = "windows"))]
fn create_link(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Asks the user, then restarts the app to import on the next launch. The
/// copy cannot happen now, because the database is open.
pub fn import_data_from_previous_version(
    _: &mut Workspace,
    _: &ImportDataFromPreviousVersion,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    let app_name = release_channel::ReleaseChannel::global(cx).display_name();
    let Some(previous) = previous_folders() else {
        let detail = format!("No {PREVIOUS_APP_NAME} folders were found.");
        drop(window.prompt(
            PromptLevel::Info,
            "Nothing to import",
            Some(&detail),
            &["OK"],
            cx,
        ));
        return;
    };
    let detail = format!(
        "Copies settings, recent projects, extensions and agent history from \
         {} and {}, replacing {app_name}'s own. What {app_name} has now is kept \
         in a \"{BACKUP_DIR}\" folder. {app_name} restarts to do this.",
        previous.config.display(),
        previous.data.display(),
    );
    let answer = window.prompt(
        PromptLevel::Warning,
        "Import data from the previous version?",
        Some(&detail),
        &["Import and Restart", "Cancel"],
        cx,
    );
    cx.spawn(async move |_, cx| {
        if answer.await != Ok(0) {
            return;
        }
        if let Err(error) = fs::write(paths::data_dir().join(PENDING_FILE), "") {
            log::error!("could not request the data import: {error}");
            return;
        }
        cx.update(|cx: &mut App| cx.restart());
    })
    .detach();
}
