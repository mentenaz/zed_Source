//! Zed's own git credential helper (section 11, item 4 of
//! `helm_backend/octocrab-helm-coverage.md`).
//!
//! Git run in one of Zed's terminals asks this helper first for
//! `https://github.com` (the terminal's `GIT_CONFIG_*` settings, from
//! `ide_env::git_credential_terminal_env`). Git starts it as
//! `<zed> --git-credential get` and writes the request on its stdin; the
//! helper passes the request, with the folder git runs in, to a socket the
//! running Zed serves ([`start_server`]), and prints the answer. Zed
//! answers with the credential set for the repository that folder belongs
//! to (`ide_env::git_credential_for`), looked up when git asks, so an open
//! terminal follows an account switch. When there is nothing to answer, the
//! helper prints nothing and git asks the user's own helpers next.
//!
//! Only `get` is answered: git's `store` and `erase` are ignored, so the
//! token is never copied elsewhere. The token is never in the terminal's
//! environment, and never logged.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// The argument that makes the Zed binary run as the helper.
pub const HELPER_ARG: &str = "--git-credential";

/// What separates the request from the end of what the helper sends.
const END: u8 = 0;

/// Requests larger than this are refused (git's are a few lines).
const MAX_REQUEST: usize = 64 * 1024;

/// A git credential request: the fields Zed looks at.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    pub protocol: Option<String>,
    pub host: Option<String>,
    /// The folder git runs in, added by the helper.
    pub cwd: Option<PathBuf>,
}

/// Reads a request: `key=value` lines, as git writes them, plus the `cwd`
/// line the helper adds. Other lines are ignored.
pub fn parse_request(text: &str) -> Request {
    let mut request = Request::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "protocol" => request.protocol = Some(value.to_string()),
            "host" => request.host = Some(value.to_string()),
            "cwd" => request.cwd = Some(PathBuf::from(value)),
            _ => {}
        }
    }
    request
}

/// What to send to git for `request`, given the credential `lookup` finds
/// for a folder. Empty when Zed has nothing to say: not https, not
/// github.com, or no credential for the folder.
pub fn answer(request: &Request, lookup: impl Fn(&Path) -> Option<(String, String)>) -> String {
    let is_github = request.protocol.as_deref() == Some("https")
        && request
            .host
            .as_deref()
            .is_some_and(|host| host.eq_ignore_ascii_case("github.com"));
    if !is_github {
        return String::new();
    }
    let Some((username, password)) = request.cwd.as_deref().and_then(lookup) else {
        return String::new();
    };
    format!("username={username}\npassword={password}\n")
}

/// The helper as written in git's config: `!'<program>' --git-credential`.
/// Git runs a value starting with `!` through its shell; single quotes keep
/// a Windows path's backslashes as they are.
pub fn helper_command(program: &Path) -> String {
    let program = program.to_string_lossy().replace('\'', r"'\''");
    format!("!'{program}' {HELPER_ARG}")
}

/// Runs as git's credential helper, for `<zed> --git-credential
/// <operation>`. Returns the exit code; any failure is silent (exit 0, no
/// output), so git simply asks the next helper.
pub fn run_helper(operation: Option<&str>) -> i32 {
    let mut input = String::new();
    // Git writes the request whatever the operation; it is read either way.
    if io::stdin().read_to_string(&mut input).is_err() || operation != Some("get") {
        return 0;
    }
    let Ok(socket) = std::env::var(ide_env::GIT_CREDENTIAL_SOCKET_VAR) else {
        return 0;
    };
    let Ok(cwd) = std::env::current_dir() else {
        return 0;
    };
    let mut request = input.into_bytes();
    if !request.ends_with(b"\n") {
        request.push(b'\n');
    }
    request.extend_from_slice(format!("cwd={}\n", cwd.display()).as_bytes());
    request.push(END);

    let Ok(mut stream) = net::UnixStream::connect(&socket) else {
        return 0;
    };
    if stream.write_all(&request).is_err() {
        return 0;
    }
    let mut response = Vec::new();
    if stream.read_to_end(&mut response).is_err() {
        return 0;
    }
    io::stdout().write_all(&response).ok();
    0
}

/// Starts answering credential requests in this process, on a socket in a
/// new per-user temporary folder, and registers the helper with `ide_env`
/// so new terminals use it. `program` is this binary.
pub fn start_server(program: &Path) -> io::Result<PathBuf> {
    let folder = tempfile::Builder::new()
        .prefix("forge-git-credential")
        .tempdir()?;
    let socket = folder.path().join("credential.sock");
    let listener = net::UnixListener::bind(&socket)?;
    // The folder lives as long as the process; it holds the socket.
    let folder = folder.keep();
    log::info!("git credential helper listening in {}", folder.display());

    std::thread::Builder::new()
        .name("GitCredential".to_string())
        .spawn(move || {
            loop {
                match listener.accept() {
                    Ok((stream, _)) => serve(stream),
                    Err(error) => {
                        log::error!("git credential helper stopped: {error}");
                        break;
                    }
                }
            }
        })?;

    ide_env::set_git_credential_helper(helper_command(program), socket.clone());
    Ok(socket)
}

/// Answers one request.
fn serve(mut stream: net::UnixStream) {
    let mut request = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                request.extend_from_slice(&buffer[..read]);
                if request.contains(&END) || request.len() > MAX_REQUEST {
                    break;
                }
            }
            Err(_) => return,
        }
    }
    if request.len() > MAX_REQUEST {
        return;
    }
    let end = request.iter().position(|&byte| byte == END).unwrap_or(request.len());
    let request = parse_request(&String::from_utf8_lossy(&request[..end]));
    let response = answer(&request, ide_env::git_credential_for);
    stream.write_all(response.as_bytes()).ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(protocol: &str, host: &str, cwd: &str) -> Request {
        Request {
            protocol: Some(protocol.into()),
            host: Some(host.into()),
            cwd: Some(PathBuf::from(cwd)),
        }
    }

    #[test]
    fn git_requests_are_read_with_the_added_folder() {
        let text = "protocol=https\nhost=github.com\npath=owner/repo.git\ncwd=E:\\repo\n";
        assert_eq!(parse_request(text), request("https", "github.com", "E:\\repo"));
    }

    #[test]
    fn only_https_github_with_a_known_folder_is_answered() {
        let known = |path: &Path| {
            (path == Path::new("E:\\repo")).then(|| ("x-access-token".into(), "secret".into()))
        };
        assert_eq!(
            answer(&request("https", "GitHub.com", "E:\\repo"), known),
            "username=x-access-token\npassword=secret\n"
        );
        assert_eq!(answer(&request("https", "github.com", "E:\\other"), known), "");
        assert_eq!(answer(&request("https", "dev.azure.com", "E:\\repo"), known), "");
        assert_eq!(answer(&request("http", "github.com", "E:\\repo"), known), "");
        assert_eq!(answer(&Request::default(), known), "");
    }

    #[test]
    fn the_helper_command_quotes_the_program_for_gits_shell() {
        assert_eq!(
            helper_command(Path::new(r"E:\zed_Source\target\debug\zeddev.exe")),
            r"!'E:\zed_Source\target\debug\zeddev.exe' --git-credential"
        );
        assert_eq!(
            helper_command(Path::new("/opt/it's/zed")),
            r"!'/opt/it'\''s/zed' --git-credential"
        );
    }

    #[test]
    fn a_request_over_the_socket_gets_the_folders_credential() {
        let repository = std::env::temp_dir().join("git-credential-test-repository");
        ide_env::set_git_credential(
            &repository,
            Some(("x-access-token".to_string(), "secret".to_string())),
        );
        let folder = tempfile::tempdir().unwrap();
        let socket = folder.path().join("test.sock");
        let listener = net::UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            serve(stream);
        });

        let mut stream = net::UnixStream::connect(&socket).unwrap();
        let mut request = format!(
            "protocol=https\nhost=github.com\ncwd={}\n",
            repository.join("src").display()
        )
        .into_bytes();
        request.push(END);
        stream.write_all(&request).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        server.join().unwrap();
        ide_env::set_git_credential(&repository, None);

        assert_eq!(response, "username=x-access-token\npassword=secret\n");
    }
}
