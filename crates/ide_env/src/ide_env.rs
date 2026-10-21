//! The IDE's own runtimes, for anything that starts a process.
//!
//! The IDE can carry runtimes of its own — the Node.js versions the Node
//! panel installs, for one — that are deliberately not on the machine's
//! `PATH`. A process the IDE starts runs under them by having their folders
//! put first on *its* `PATH`, which changes nothing outside that process.
//!
//! This crate is where the two sides meet. Whatever manages a runtime
//! [`register`]s a provider at startup; whatever starts a process (a
//! terminal, the Script Runner, a workflow action) asks for [`bin_dirs`] or
//! [`path_env`] and never needs to know which runtimes exist.
//!
//! It has no dependencies, so the headless crates can use it too. A provider
//! may read the disk (a version store, a project's version file), so call
//! [`bin_dirs`] and [`path_env`] from a background thread.
//!
//! It also carries git settings for the git commands that talk to a remote
//! (push, pull, fetch), per repository folder: whatever knows which account
//! a repository uses (the Helm panel) sets them with
//! [`set_git_remote_config`], and the code that starts those commands adds
//! them with [`with_git_remote_config`]. They go to git as `GIT_CONFIG_*`
//! environment variables, never on the command line.
//!
//! For git run in a terminal, the token is not put in the environment at
//! all. Instead the terminal's git asks Zed's own credential helper
//! ([`git_credential_terminal_env`]), which looks the account up when git
//! asks ([`git_credential_for`]), so an open terminal follows an account
//! switch.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::hash::BuildHasher;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// Answers with the folders a process started in `directory` should have
/// first on its `PATH` for one runtime: usually one folder, or none when the
/// machine's own copy of that runtime applies.
pub type Provider = fn(directory: Option<&Path>) -> Vec<PathBuf>;

static PROVIDERS: RwLock<Vec<Provider>> = RwLock::new(Vec::new());

/// Adds a runtime. Call once per runtime, at app startup.
pub fn register(provider: Provider) {
    if let Ok(mut providers) = PROVIDERS.write() {
        providers.push(provider);
    }
}

/// The folders to put first on the `PATH` of a process started in
/// `directory`, across every registered runtime. Empty when the machine's own
/// runtimes apply and the `PATH` should be left alone.
pub fn bin_dirs(directory: Option<&Path>) -> Vec<PathBuf> {
    let providers = match PROVIDERS.read() {
        Ok(providers) => providers.clone(),
        Err(_) => return Vec::new(),
    };
    providers
        .into_iter()
        .flat_map(|provider| provider(directory))
        .collect()
}

/// The `PATH` to give a process started in `directory`: this process's own,
/// with the IDE's runtimes first. `None` when there is nothing to change.
pub fn path_env(directory: Option<&Path>) -> Option<OsString> {
    let path = std::env::var_os("PATH");
    prepend(&bin_dirs(directory), path.as_deref())
}

/// `path` with `dirs` first on it, and without any later mention of them.
/// `None` when `dirs` is empty, or can't be written as a `PATH`.
pub fn prepend(dirs: &[PathBuf], path: Option<&OsStr>) -> Option<OsString> {
    if dirs.is_empty() {
        return None;
    }
    let rest = path
        .map(|path| std::env::split_paths(path).collect::<Vec<_>>())
        .unwrap_or_default();
    let entries = dirs
        .iter()
        .cloned()
        .chain(rest.into_iter().filter(|entry| !dirs.contains(entry)));
    std::env::join_paths(entries).ok()
}

/// Puts `dirs` first on the `PATH` held in `env`, an environment that is
/// still being put together (a terminal's). When `env` has no `PATH` yet,
/// this process's own is what `dirs` go in front of, since that is what the
/// new process would otherwise inherit.
pub fn apply_to<S: BuildHasher>(env: &mut HashMap<String, String, S>, dirs: &[PathBuf]) {
    if dirs.is_empty() {
        return;
    }
    let key = env
        .keys()
        .find(|name| is_path_variable(name))
        .cloned()
        .unwrap_or_else(|| "PATH".to_string());
    let current = env
        .get(&key)
        .map(OsString::from)
        .or_else(|| std::env::var_os("PATH"));
    let Some(path) = prepend(dirs, current.as_deref()) else {
        return;
    };
    // A terminal's environment is text; a `PATH` that isn't is left as it was.
    if let Ok(path) = path.into_string() {
        env.insert(key, path);
    }
}

/// Git settings (`key`, `value`) for the remote commands of the repository
/// in each folder.
static GIT_REMOTE_CONFIG: RwLock<Vec<(PathBuf, Vec<(String, String)>)>> = RwLock::new(Vec::new());

/// Sets the git settings push, pull and fetch get in the repository at
/// `repository`, replacing any set before. Empty `entries` removes them.
pub fn set_git_remote_config(repository: &Path, entries: Vec<(String, String)>) {
    let Ok(mut config) = GIT_REMOTE_CONFIG.write() else {
        return;
    };
    config.retain(|(folder, _)| folder != repository);
    if !entries.is_empty() {
        config.push((repository.to_path_buf(), entries));
    }
}

/// The git settings set for the repository at `repository`.
pub fn git_remote_config(repository: &Path) -> Vec<(String, String)> {
    GIT_REMOTE_CONFIG
        .read()
        .ok()
        .and_then(|config| {
            config
                .iter()
                .find(|(folder, _)| folder == repository)
                .map(|(_, entries)| entries.clone())
        })
        .unwrap_or_default()
}

/// `env` with the git settings set for the repository at `repository`
/// added as `GIT_CONFIG_*` variables, after any `env` already has. `env`
/// itself when none are set.
pub fn with_git_remote_config<S: BuildHasher + Clone>(
    repository: &Path,
    env: Arc<HashMap<String, String, S>>,
) -> Arc<HashMap<String, String, S>> {
    let entries = git_remote_config(repository);
    if entries.is_empty() {
        return env;
    }
    let mut env = (*env).clone();
    append_git_config(&mut env, &entries);
    Arc::new(env)
}

/// Adds `entries` to the `GIT_CONFIG_COUNT` / `GIT_CONFIG_KEY_<n>` /
/// `GIT_CONFIG_VALUE_<n>` settings in `env`, numbering on from those already
/// there.
pub fn append_git_config<S: BuildHasher>(
    env: &mut HashMap<String, String, S>,
    entries: &[(String, String)],
) {
    let start = env
        .get("GIT_CONFIG_COUNT")
        .and_then(|count| count.parse::<usize>().ok())
        .unwrap_or(0);
    for (offset, (key, value)) in entries.iter().enumerate() {
        env.insert(format!("GIT_CONFIG_KEY_{}", start + offset), key.clone());
        env.insert(format!("GIT_CONFIG_VALUE_{}", start + offset), value.clone());
    }
    env.insert(
        "GIT_CONFIG_COUNT".to_string(),
        (start + entries.len()).to_string(),
    );
}

/// The github.com credential for the repository in each folder: (folder,
/// username, password).
static GIT_CREDENTIALS: RwLock<Vec<(PathBuf, String, String)>> = RwLock::new(Vec::new());

/// Zed's own git credential helper, once it is answering: the helper
/// command for git's config, and the socket it talks to.
static GIT_CREDENTIAL_HELPER: RwLock<Option<(String, PathBuf)>> = RwLock::new(None);

/// The environment variable telling Zed's credential helper which socket
/// to ask.
pub const GIT_CREDENTIAL_SOCKET_VAR: &str = "ZED_GIT_CREDENTIAL_SOCKET";

/// A path compared the way the file system does: on Windows, without regard
/// to case or to which slash separates the parts.
fn path_key(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        text.replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    } else {
        text.trim_end_matches('/').to_string()
    }
}

/// Sets, or with `None` removes, the github.com credential git run in the
/// repository at `repository` (or a folder below it) is given.
pub fn set_git_credential(repository: &Path, credential: Option<(String, String)>) {
    let Ok(mut credentials) = GIT_CREDENTIALS.write() else {
        return;
    };
    let key = path_key(repository);
    credentials.retain(|(folder, _, _)| path_key(folder) != key);
    if let Some((username, password)) = credential {
        credentials.push((repository.to_path_buf(), username, password));
    }
}

/// The github.com credential for git run in `directory`: the one set for the
/// nearest repository folder that contains it.
pub fn git_credential_for(directory: &Path) -> Option<(String, String)> {
    let directory = path_key(directory);
    let separator = if cfg!(windows) { '\\' } else { '/' };
    let credentials = GIT_CREDENTIALS.read().ok()?;
    credentials
        .iter()
        .filter_map(|(folder, username, password)| {
            let folder = path_key(folder);
            let contains = directory == folder
                || directory
                    .strip_prefix(&folder)
                    .is_some_and(|rest| rest.starts_with(separator));
            contains.then_some((folder.len(), username, password))
        })
        .max_by_key(|(length, _, _)| *length)
        .map(|(_, username, password)| (username.clone(), password.clone()))
}

/// Records that Zed's credential helper is answering: `command` is the
/// helper as written in git's config (`!'<program>' --git-credential`),
/// `socket` the socket it asks.
pub fn set_git_credential_helper(command: String, socket: PathBuf) {
    if let Ok(mut helper) = GIT_CREDENTIAL_HELPER.write() {
        *helper = Some((command, socket));
    }
}

/// The settings that make git run in a terminal opened in `directory` ask
/// Zed's credential helper first for github.com, then the user's own
/// helpers in their usual order: `GIT_CONFIG_*` entries, and the socket
/// variable. Empty when the helper is not answering. Runs `git config`, so
/// call it from a background thread.
pub fn git_credential_terminal_env(
    directory: Option<&Path>,
) -> (Vec<(String, String)>, Vec<(String, String)>) {
    let Some((command, socket)) = GIT_CREDENTIAL_HELPER
        .read()
        .ok()
        .and_then(|helper| helper.clone())
    else {
        return (Vec::new(), Vec::new());
    };
    const KEY: &str = "credential.https://github.com.helper";
    let mut config = vec![
        // An empty value clears the helpers configured so far; they are put
        // back after Zed's own.
        (KEY.to_string(), String::new()),
        (KEY.to_string(), command),
    ];
    config.extend(
        github_credential_helpers(directory)
            .into_iter()
            .map(|helper| (KEY.to_string(), helper)),
    );
    let variables = vec![(
        GIT_CREDENTIAL_SOCKET_VAR.to_string(),
        socket.to_string_lossy().into_owned(),
    )];
    (config, variables)
}

/// Adds [`git_credential_terminal_env`]'s settings to `env`, a terminal's
/// environment that is still being put together.
pub fn apply_git_credential_env<S: BuildHasher>(
    env: &mut HashMap<String, String, S>,
    settings: (Vec<(String, String)>, Vec<(String, String)>),
) {
    let (config, variables) = settings;
    if config.is_empty() {
        return;
    }
    append_git_config(env, &config);
    env.extend(variables);
}

/// The credential helpers git would use for `https://github.com` in
/// `directory`, in order, as the user's config sets them.
fn github_credential_helpers(directory: Option<&Path>) -> Vec<String> {
    let mut command = std::process::Command::new("git");
    command
        .args(["config", "--get-regexp", r"^credential\..*helper$"])
        .stdin(std::process::Stdio::null());
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0800_0000);
    }
    let Ok(output) = command.output() else {
        return Vec::new();
    };
    parse_github_credential_helpers(&String::from_utf8_lossy(&output.stdout))
}

/// The helpers for `https://github.com` out of `git config --get-regexp`
/// output (`key value` per line, in the order git reads them). An empty
/// value clears the ones before it, as it does for git. A helper set for a
/// path below github.com is left out.
pub fn parse_github_credential_helpers(output: &str) -> Vec<String> {
    let mut helpers = Vec::new();
    for line in output.lines() {
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        let Some(rest) = key.strip_prefix("credential.") else {
            continue;
        };
        let Some(url) = rest.strip_suffix("helper") else {
            continue;
        };
        let url = url.trim_end_matches('.').trim_end_matches('/');
        let applies = url.is_empty()
            || url.eq_ignore_ascii_case("https://github.com")
            || url.eq_ignore_ascii_case("github.com");
        if !applies {
            continue;
        }
        let value = value.trim();
        if value.is_empty() {
            helpers.clear();
        } else {
            helpers.push(value.to_string());
        }
    }
    helpers
}

/// Whether `name` is the `PATH` variable. Windows spells it `Path` as often
/// as `PATH`, and treats the two as one.
pub fn is_path_variable(name: &str) -> bool {
    if cfg!(windows) {
        name.eq_ignore_ascii_case("PATH")
    } else {
        name == "PATH"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(entries: &[&str]) -> OsString {
        std::env::join_paths(entries.iter().map(PathBuf::from)).unwrap()
    }

    fn entries(path: &OsStr) -> Vec<PathBuf> {
        std::env::split_paths(path).collect()
    }

    #[test]
    fn the_helpers_for_github_follow_the_users_config_order_and_resets() {
        let output = "credential.helper manager\n\
                      credential.helper manager\n\
                      credential.https://github.com.helper\n\
                      credential.https://github.com.helper !'C:\\gh.exe' auth git-credential\n\
                      credential.https://dev.azure.com.usehttppath true\n\
                      credential.https://gitlab.com.helper other\n";
        assert_eq!(
            parse_github_credential_helpers(output),
            vec!["!'C:\\gh.exe' auth git-credential".to_string()]
        );
        assert_eq!(
            parse_github_credential_helpers("credential.helper manager\n"),
            vec!["manager".to_string()]
        );
    }

    #[test]
    fn a_credential_is_found_from_the_nearest_repository_folder() {
        let outer = PathBuf::from("ide-env-cred-test").join("outer");
        let inner = outer.join("inner");
        set_git_credential(&outer, Some(("a".to_string(), "1".to_string())));
        set_git_credential(&inner, Some(("b".to_string(), "2".to_string())));

        let found = |path: &Path| git_credential_for(path).map(|(user, _)| user);
        assert_eq!(found(&outer), Some("a".to_string()));
        assert_eq!(found(&outer.join("src")), Some("a".to_string()));
        assert_eq!(found(&inner.join("deep")), Some("b".to_string()));
        // A sibling whose name merely starts the same is not inside.
        assert_eq!(found(&PathBuf::from("ide-env-cred-test").join("outer2")), None);

        set_git_credential(&inner, None);
        assert_eq!(found(&inner), Some("a".to_string()));
        set_git_credential(&outer, None);
        assert_eq!(found(&outer), None);
    }

    #[test]
    fn git_config_entries_are_numbered_after_existing_ones() {
        let mut env = HashMap::new();
        env.insert("GIT_CONFIG_COUNT".to_string(), "1".to_string());
        env.insert("GIT_CONFIG_KEY_0".to_string(), "gpg.program".to_string());
        env.insert("GIT_CONFIG_VALUE_0".to_string(), "wrapper".to_string());
        append_git_config(
            &mut env,
            &[("credential.helper".to_string(), String::new())],
        );
        assert_eq!(env["GIT_CONFIG_COUNT"], "2");
        assert_eq!(env["GIT_CONFIG_KEY_0"], "gpg.program");
        assert_eq!(env["GIT_CONFIG_KEY_1"], "credential.helper");
        assert_eq!(env["GIT_CONFIG_VALUE_1"], "");
    }

    #[test]
    fn git_remote_config_is_per_repository_and_can_be_removed() {
        let repository = PathBuf::from("ide-env-test-repository");
        let other = PathBuf::from("ide-env-test-other");
        set_git_remote_config(&repository, vec![("a".to_string(), "1".to_string())]);

        let env = with_git_remote_config(&repository, Arc::new(HashMap::new()));
        assert_eq!(env["GIT_CONFIG_KEY_0"], "a");
        let untouched = Arc::new(HashMap::new());
        assert!(Arc::ptr_eq(
            &with_git_remote_config(&other, untouched.clone()),
            &untouched
        ));

        set_git_remote_config(&repository, Vec::new());
        assert!(git_remote_config(&repository).is_empty());
    }

    #[test]
    fn nothing_to_add_leaves_the_path_alone() {
        assert_eq!(prepend(&[], Some(&joined(&["a", "b"]))), None);

        let mut env = HashMap::new();
        env.insert("PATH".to_string(), "untouched".to_string());
        apply_to(&mut env, &[]);
        assert_eq!(env["PATH"], "untouched");
    }

    #[test]
    fn runtimes_go_first_and_are_not_repeated() {
        let dirs = [PathBuf::from("node22"), PathBuf::from("python312")];
        let path = prepend(&dirs, Some(&joined(&["system", "node22", "tools"]))).unwrap();
        assert_eq!(
            entries(&path),
            ["node22", "python312", "system", "tools"].map(PathBuf::from)
        );

        // With no `PATH` to go in front of, they are the whole of it.
        let path = prepend(&dirs, None).unwrap();
        assert_eq!(entries(&path), dirs);
    }

    #[test]
    fn an_environment_keeps_its_own_spelling_of_path() {
        let dirs = [PathBuf::from("node22")];

        let mut env = HashMap::new();
        env.insert("PATH".to_string(), joined(&["system"]).into_string().unwrap());
        apply_to(&mut env, &dirs);
        assert_eq!(env.len(), 1);
        assert_eq!(
            entries(OsStr::new(&env["PATH"])),
            ["node22", "system"].map(PathBuf::from)
        );

        // Windows hands back `Path`; a second, differently spelled variable
        // beside it would leave it to chance which one the shell reads.
        if cfg!(windows) {
            let mut env = HashMap::new();
            env.insert("Path".to_string(), joined(&["system"]).into_string().unwrap());
            apply_to(&mut env, &dirs);
            assert_eq!(env.len(), 1);
            assert_eq!(
                entries(OsStr::new(&env["Path"])),
                ["node22", "system"].map(PathBuf::from)
            );
        }
    }

    #[test]
    fn an_environment_without_a_path_gets_one() {
        let dirs = [PathBuf::from("node22")];
        let mut env: HashMap<String, String> = HashMap::new();
        apply_to(&mut env, &dirs);
        let path = entries(OsStr::new(&env["PATH"]));
        assert_eq!(path[0], PathBuf::from("node22"));
        // In front of what would have been inherited, not instead of it.
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        let inherited: Vec<PathBuf> = std::env::split_paths(&inherited)
            .filter(|entry| entry != &dirs[0])
            .collect();
        assert_eq!(&path[1..], inherited.as_slice());
    }

    #[test]
    fn registered_runtimes_are_asked_about_the_directory() {
        fn only_in_projects(directory: Option<&Path>) -> Vec<PathBuf> {
            directory
                .filter(|directory| directory.ends_with("ide_env_test_project"))
                .map(|directory| vec![directory.join("runtime")])
                .unwrap_or_default()
        }
        register(only_in_projects);

        let project = Path::new("some").join("ide_env_test_project");
        assert!(bin_dirs(Some(&project)).contains(&project.join("runtime")));
        assert!(!bin_dirs(None).contains(&project.join("runtime")));
        let path = path_env(Some(&project)).unwrap();
        assert_eq!(entries(&path)[0], project.join("runtime"));
    }
}
