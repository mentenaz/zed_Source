# script_runner_panel

A bottom-dock panel that runs a shell command and streams its output live.
It is also the shared output console for the other fork panels.

## Why it exists

The Node, Python and .NET panels and the three package managers all need
somewhere to run a command (`npm run dev`, `pip install …`,
`dotnet add package …`) and show what it printed. Rather than each one
growing its own console, they all send their commands here.

In Forge this happened over a host `AppState` broadcast channel. That channel
does not exist in this tree, so other panels call the runner directly through
its entity instead.

## Using it

Open with `ctrl-k k` (`cmd-k k` on macOS), the status bar icon (tooltip
"Script Runner"), or `script runner panel: toggle focus`.

Type a command into "Enter a command…" and run it. Output appears line by
line as the process prints it, and a running command can be stopped
mid-flight.

When a command prints an SPFx workbench debug URL (from `gulp serve` or
`heft start`), the panel surfaces the query-string fragment in its own bar
for easy copying.

## Calling it from another panel

```rust
// `runner` is an Entity<ScriptRunnerPanel> (or an upgraded WeakEntity).
runner.update(cx, |runner, cx| {
    runner.run_external("npm install".into(), project_dir, cx);
});

// Watch for completion.
cx.observe(&runner, |this, runner, cx| {
    if !runner.read(cx).is_running() {
        // reload your own state
    }
})
.detach();
```

| Method | Purpose |
| --- | --- |
| `run_external(command, cwd, cx)` | Run `command` in `cwd` and stream output into the panel |
| `is_running()` | Whether a command is currently running |

Dock panels (`node_panel`, `python_panel`) receive a
`WeakEntity<ScriptRunnerPanel>` from `initialize_panels` once everything has
loaded. Workspace tabs (the package managers) look the panel up with
`workspace.open_panel::<ScriptRunnerPanel>` when they need it.

### Building commands safely

The runner executes one string through a shell, so anything a panel pastes
into that string is interpreted by the shell. `script_runner_panel::command`
holds the checks every panel in this fork uses before building a command:

| Function | Accepts |
| --- | --- |
| `check_package_name` | Letters, digits and `@ / . _ -` |
| `check_version` | Letters, digits and `. _ + -` |
| `check_script_name` | Letters, digits and `: . _ - @ / +` |
| `shell_program(path, cwd)` | A program path with no spaces or shell syntax |
| `shell_path(path, cwd)` | The same, for a file passed as an argument |

All of them return `Err(message)` instead of producing something unsafe; show
that message to the user rather than running the command. None of them
quote: `pwsh`, `cmd` and `sh` disagree on quoting, so the rule is to refuse
anything that needs it.

`shell_program` and `shell_path` rewrite a path inside `cwd` as relative to
it. That is what lets a project under a directory with spaces in its name
(`C:\Users\First Last\…`) work, since the space is no longer part of the
command. A path that still contains a space after that — an interpreter
installed under `C:\Program Files`, say — is refused with a message saying
so.

Values starting with `-` are refused too, so a "package" named
`--registry=…` cannot be read as an option.

## How it is wired

- `script_runner_panel::init(cx)` registers `ToggleFocus`.
- `ScriptRunnerPanel::load(workspace, cx)` is awaited in `initialize_panels`
  (`crates/zed/src/zed.rs`), together with the panels that depend on it.
- Implements `workspace::dock::Panel`, docked at the bottom by default (the
  `script_runner_panel.dock` setting moves it to any dock; `script_runner_panel.button` hides
  its status bar button), `activation_priority() = 23`.

## Platform notes

On Windows, commands are run through `pwsh.exe`, falling back to `cmd.exe`
when PowerShell 7 is not installed. On other platforms they are run through
`sh -c`. Output from stderr is merged into the same stream either way.

The non-Windows path has only been compiled, not exercised: it has not been
run on Linux or macOS yet.

Two details matter for long-running commands:

- `PYTHONUNBUFFERED=1` is set so Python prints line by line instead of
  buffering when its output is a pipe.
- Stop kills the whole process tree on Windows (`taskkill /T`), since the
  direct child is the shell rather than the program you started. Off Windows
  it sends `kill -9` to the shell only, so a program the shell started may
  outlive it.

## Development

```sh
cargo check -p script_runner_panel -j 8
cargo test -p script_runner_panel -j 8
```

The tests cover stripping terminal colour codes, URL and SPFx debug-link
detection, and the command-safety checks in `src/command.rs`.

To run the tests of every fork crate at once: `script/test-fork-crates.ps1`.
