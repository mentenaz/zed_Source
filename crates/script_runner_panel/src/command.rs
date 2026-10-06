//! Checks for the pieces panels splice into Script Runner command lines.
//!
//! The runner executes one *string* through a shell (`pwsh`, `cmd` or `sh`
//! — see `run_shell_blocking`), so anything a panel pastes into that string
//! is interpreted by the shell: a package name of `x; rm -rf ~` is two
//! commands, and an interpreter at `C:\My Projects\venv\…` is split at the
//! space. Quoting can't fix this portably — the three shells disagree on
//! quote characters, on escaping inside them, and on whether a quoted
//! program needs a call operator (`& "…"` in PowerShell, a syntax error in
//! `cmd`) — and the caller doesn't know which shell will end up running the
//! line.
//!
//! So instead of quoting, these functions *refuse* anything that isn't made
//! solely of characters every one of those shells passes through untouched.
//! Callers surface the returned message to the user rather than running a
//! command that would mean something other than what the button says.

use std::path::{MAIN_SEPARATOR, Path};

/// Whether `value` is non-empty, doesn't start like a command-line option,
/// and is made only of letters, digits and the characters in `extra`.
fn is_plain(value: &str, extra: &[char]) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value
            .chars()
            .all(|c| c.is_alphanumeric() || extra.contains(&c))
}

/// Accepts a package name as npm, NuGet and PyPI write them: letters,
/// digits, and `@ / . _ -` (npm scopes are `@scope/name`).
///
/// The leading-`-` rule is what stops a "package" called `--registry=…`
/// from being read as an option by the package manager itself.
pub fn check_package_name(name: &str) -> Result<(), String> {
    if is_plain(name, &['@', '/', '.', '_', '-']) {
        Ok(())
    } else {
        Err(format!(
            "Not running this: {name:?} is not a valid package name \
             (only letters, digits and @ / . _ - are allowed)."
        ))
    }
}

/// Accepts an exact version: letters, digits, and `. _ + -` (covers semver
/// pre-release/build suffixes and PEP 440 forms like `2.0.0rc1`).
pub fn check_version(version: &str) -> Result<(), String> {
    if is_plain(version, &['.', '_', '+', '-']) {
        Ok(())
    } else {
        Err(format!(
            "Not running this: {version:?} is not a valid version \
             (only letters, digits and . _ + - are allowed)."
        ))
    }
}

/// Accepts a `package.json` script name: letters, digits, and
/// `: . _ - @ / +` (`build:prod`, `test.unit`, `lint-fix`).
pub fn check_script_name(name: &str) -> Result<(), String> {
    if is_plain(name, &[':', '.', '_', '-', '@', '/', '+']) {
        Ok(())
    } else {
        Err(format!(
            "Not running this: the script name {name:?} contains characters the Script Runner \
             can't pass to a shell safely. Run it from a terminal instead."
        ))
    }
}

/// `path` as it can be written into a command that runs in `cwd`.
///
/// A path inside `cwd` is rewritten relative to it — which is what makes a
/// project living under a directory with spaces in its name (a Windows user
/// profile, typically) work at all, since the space is then no longer part
/// of what the shell sees. Whatever is left must still be shell-plain;
/// otherwise this returns an error naming the path.
fn plain_path(path: &str, cwd: &str, is_program: bool) -> Result<String, String> {
    // An empty `cwd` is a prefix of everything, which would make a bare
    // program name look like a file in the working directory.
    let relative = (!cwd.is_empty())
        .then(|| Path::new(path).strip_prefix(Path::new(cwd)).ok())
        .flatten()
        .filter(|relative| relative.components().next().is_some());

    let candidate = match relative {
        Some(relative) => {
            let relative = relative.to_string_lossy().into_owned();
            // A bare file name is looked up on PATH, not in the current
            // directory, when it's the program being run — PowerShell and
            // POSIX shells both need the explicit `./`.
            if is_program && Path::new(&relative).components().count() == 1 {
                format!(".{MAIN_SEPARATOR}{relative}")
            } else {
                relative
            }
        }
        None => path.to_string(),
    };

    // `~` only expands at the start of a word; elsewhere it's the literal
    // character Windows 8.3 names (`PROGRA~1`) are full of.
    if is_plain(&candidate, &['\\', '/', '.', '_', '-', ':', '+', '~']) && !candidate.starts_with('~')
    {
        Ok(candidate)
    } else {
        Err(format!(
            "Not running this: the path {path:?} contains spaces or other characters the Script \
             Runner can't pass to a shell safely. Move it to a path without them, or run the \
             command from a terminal."
        ))
    }
}

/// A program path (an interpreter, typically) ready to start a command run
/// in `cwd`. A bare name such as `python` is left for the shell to find on
/// `PATH`.
pub fn shell_program(program: &str, cwd: &str) -> Result<String, String> {
    plain_path(program, cwd, true)
}

/// A file path ready to be passed as an argument in a command run in `cwd`.
pub fn shell_path(path: &str, cwd: &str) -> Result<String, String> {
    plain_path(path, cwd, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn join(parts: &[&str]) -> String {
        parts.join(&MAIN_SEPARATOR.to_string())
    }

    #[test]
    fn ordinary_package_names_are_accepted() {
        for name in [
            "react",
            "@types/node",
            "@scope/my-package.js",
            "Newtonsoft.Json",
            "Microsoft.Extensions.Logging",
            "typing_extensions",
            "left-pad",
            "h11",
        ] {
            assert_eq!(check_package_name(name), Ok(()), "{name}");
        }
    }

    #[test]
    fn shell_metacharacters_in_package_names_are_refused() {
        for name in [
            "react; rm -rf ~",
            "react && calc",
            "react|more",
            "react`whoami`",
            "$(whoami)",
            "react > out.txt",
            "a b",
            "react\"",
            "react'",
            "%PATH%",
            "react\nnext",
            "react#comment",
            "(react)",
            "",
        ] {
            assert!(check_package_name(name).is_err(), "{name:?} should be refused");
        }
    }

    #[test]
    fn option_like_values_are_refused() {
        assert!(check_package_name("--registry=http://evil.test").is_err());
        assert!(check_package_name("-g").is_err());
        assert!(check_version("--pre").is_err());
        assert!(check_script_name("--version").is_err());
        // A dash elsewhere is fine.
        assert_eq!(check_package_name("left-pad"), Ok(()));
        assert_eq!(check_version("1.0.0-beta.1"), Ok(()));
    }

    #[test]
    fn versions() {
        for version in ["1.2.3", "18.2.0", "1.0.0-beta.1", "1.0.0+build.5", "2.0.0rc1", "3.6", "2024.1"] {
            assert_eq!(check_version(version), Ok(()), "{version}");
        }
        for version in ["1.0 && calc", "^1.0.0", ">=1.0", "1.0;", "latest version", "*", ""] {
            assert!(check_version(version).is_err(), "{version:?} should be refused");
        }
    }

    #[test]
    fn script_names() {
        for name in ["dev", "build:prod", "test.unit", "lint-fix", "pre+post", "@org/task"] {
            assert_eq!(check_script_name(name), Ok(()), "{name}");
        }
        for name in ["dev && calc", "say hi", "a;b", "x|y", "$(x)", ""] {
            assert!(check_script_name(name).is_err(), "{name:?} should be refused");
        }
    }

    #[test]
    fn refusals_name_the_offending_value() {
        let error = check_package_name("bad name").unwrap_err();
        assert!(error.contains("bad name"), "{error}");
        let error = shell_program(&join(&["", "opt", "my tools", "python"]), "/elsewhere").unwrap_err();
        assert!(error.contains("my tools"), "{error}");
    }

    #[test]
    fn bare_program_names_are_left_for_path_lookup() {
        assert_eq!(shell_program("python", &join(&["", "work", "app"])).as_deref(), Ok("python"));
        assert_eq!(shell_program("py", "").as_deref(), Ok("py"));
    }

    #[test]
    fn paths_inside_the_working_directory_become_relative() {
        // The directory has a space in it; the relative path doesn't.
        let cwd = join(&["", "Users", "My Name", "project"]);
        let interpreter = join(&[&cwd, "venv", "bin", "python"]);
        assert_eq!(
            shell_program(&interpreter, &cwd),
            Ok(join(&["venv", "bin", "python"]))
        );

        let entry = join(&[&cwd, "src", "main.py"]);
        assert_eq!(shell_path(&entry, &cwd), Ok(join(&["src", "main.py"])));
    }

    #[test]
    fn a_program_directly_in_the_working_directory_gets_a_dot_prefix() {
        let cwd = join(&["", "work", "app"]);
        let program = join(&[&cwd, "tool"]);
        assert_eq!(shell_program(&program, &cwd), Ok(join(&[".", "tool"])));
        // As an argument, a bare file name needs no prefix.
        assert_eq!(shell_path(&program, &cwd).as_deref(), Ok("tool"));
    }

    #[test]
    fn plain_paths_outside_the_working_directory_pass_through() {
        let cwd = join(&["", "work", "app"]);
        let interpreter = join(&["", "opt", "python3.12", "bin", "python"]);
        assert_eq!(shell_program(&interpreter, &cwd), Ok(interpreter.clone()));
        // Windows drive letters and 8.3 short names are plain too.
        assert_eq!(
            shell_program("C:\\PROGRA~1\\Python312\\python.exe", &cwd).as_deref(),
            Ok("C:\\PROGRA~1\\Python312\\python.exe")
        );
    }

    #[test]
    fn unsafe_paths_are_refused() {
        let cwd = join(&["", "work", "app"]);
        for path in [
            join(&["", "Program Files", "Python312", "python"]),
            join(&[&cwd, "my venv", "bin", "python"]),
            join(&["", "opt", "py&calc", "python"]),
            join(&["", "opt", "py;x", "python"]),
            join(&["", "opt", "$(x)", "python"]),
            "~/bin/python".to_string(),
            String::new(),
        ] {
            assert!(shell_program(&path, &cwd).is_err(), "{path:?} should be refused");
            assert!(shell_path(&path, &cwd).is_err(), "{path:?} should be refused");
        }
    }

    #[test]
    fn non_ascii_letters_in_paths_are_allowed() {
        let cwd = join(&["", "work", "app"]);
        let interpreter = join(&["", "Users", "José", "venv", "bin", "python"]);
        assert_eq!(shell_program(&interpreter, &cwd), Ok(interpreter.clone()));
    }
}
