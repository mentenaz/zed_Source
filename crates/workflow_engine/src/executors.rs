//! §4 of `DOCS/workflow-schema/execution-engine-design.md` — one async
//! function per `type_id`, dispatched by `execute_leaf`'s `match`. This is
//! the `execute` closure `scheduler::run_level` expects, for every action
//! type except containers (§5, scheduled separately by their own executor
//! recursing back into `run_level`).
//!
//! I/O-bound work (`http`, `WaitForPort`, `delay`, `script`) routes through
//! `backend::on_tokio`, same bridge `github/cli.rs` already uses — GPUI has
//! no ambient tokio reactor.

use std::path::Path;
use std::process::Stdio;

use serde_json::{Map, Value};
use tokio::io::AsyncWriteExt;
use tokio::process::Command as TokioCommand;

use crate::async_rt::on_tokio;

use super::engine::RunContext;
use super::process_tracker;
use super::schema::{Action, ScriptRuntime, ScriptSource};

/// Leaf-action dispatch. `Err` means the action resolved `Failed` (the
/// scheduler, not this function, decides what that means for `runAfter`
/// gating downstream — see `scheduler::run_level`); `Ok` means `Succeeded`
/// with the returned map as `outputs`. `solution_root` is only consulted by
/// `script`'s `file` mode (`DOCS/forge-workflow-engine-design.md` §4: path
/// resolution is always relative to the solution root, never the running
/// process's own cwd — a direct lesson from ForgeFlow's `uri_to_path` bug).
/// `action_id` is only consulted by `StartProcess`, to tag the tracked
/// process with which action started it.
pub async fn execute_leaf(
    action_id: &str,
    action: &Action,
    ctx: &RunContext,
    solution_root: &Path,
) -> Result<Map<String, Value>, String> {
    match action.type_id.as_str() {
        "http" => execute_http(action).await,
        "transform" => execute_transform(action, ctx).await,
        "script" => execute_script(action, solution_root).await,
        "delay" => execute_delay(action).await,
        "notify" => execute_notify(action, ctx).await,
        "log" => execute_log(action, ctx).await,
        "sendEmail" => execute_send_email(action, ctx).await,
        "WaitForPort" => execute_wait_for_port(action).await,
        "SetEnv" => execute_set_env(action),
        "StartProcess" => execute_start_process(action_id, action, solution_root).await,
        "DbMigration" | "AiCheck" => Err(format!(
            "{} is not implemented — stub in the original Tauri app too",
            action.type_id
        )),
        other => Err(format!("unknown action type: {other}")),
    }
}

fn require_str<'a>(action: &'a Action, field: &str) -> Result<&'a str, String> {
    action
        .inputs
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| {
            format!(
                "{}: missing or non-string \"{field}\" input",
                action.type_id
            )
        })
}

/// Like `require_str`, but for a `FieldKind::String` field a flow author
/// may reasonably want to interpolate from another action's `outputs` —
/// `log`/`notify`/`sendEmail`'s message-ish fields, not typed
/// `FieldKind::Expression` in the registry (they're ordinary display
/// text most of the time), but a bare string is too limiting for "log
/// what just failed". If the raw JSON value is already a string, use it
/// literally (the common case, and the only case that costs nothing extra
/// to evaluate); otherwise treat it as a JSONLogic expression, evaluate it
/// against `ctx`, and stringify the result (a string result is used as-is,
/// anything else — including `null` for a `{"var": ...}` that resolved to
/// nothing — is JSON-serialized so it's still visible rather than silently
/// blank).
fn resolve_string_field(action: &Action, field: &str, ctx: &RunContext) -> Result<String, String> {
    let value = action
        .inputs
        .get(field)
        .ok_or_else(|| format!("{}: missing \"{field}\" input", action.type_id))?;
    if let Some(s) = value.as_str() {
        return Ok(s.to_string());
    }
    let evaluated = ctx.evaluate(value)?;
    Ok(match evaluated {
        Value::String(s) => s,
        other => serde_json::to_string(&other).unwrap_or_default(),
    })
}

fn require_f64(action: &Action, field: &str) -> Result<f64, String> {
    action
        .inputs
        .get(field)
        .and_then(Value::as_f64)
        .ok_or_else(|| {
            format!(
                "{}: missing or non-number \"{field}\" input",
                action.type_id
            )
        })
}

async fn execute_http(action: &Action) -> Result<Map<String, Value>, String> {
    let method = require_str(action, "method")?.to_string();
    let url = require_str(action, "url")?.to_string();
    let body = action.inputs.get("body").cloned();

    on_tokio(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| format!("http: failed to build client: {e}"))?;
        let parsed_method: reqwest::Method = method
            .parse()
            .map_err(|_| format!("http: invalid method \"{method}\""))?;
        let mut req = client.request(parsed_method, &url);
        if let Some(b) = body {
            let body_text = serde_json::to_string(&b)
                .map_err(|e| format!("http: failed to serialize body: {e}"))?;
            req = req
                .header("Content-Type", "application/json")
                .body(body_text);
        }
        let res = req
            .send()
            .await
            .map_err(|e| format!("http: network error: {e}"))?;
        let status = res.status().as_u16();
        let text = res
            .text()
            .await
            .map_err(|e| format!("http: failed to read response body: {e}"))?;
        let body_value = serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text));

        let mut outputs = Map::new();
        outputs.insert("status".to_string(), Value::from(status));
        outputs.insert("body".to_string(), body_value);
        Ok(outputs)
    })
    .await
}

/// §4's "most important to resolve" decision (2026-09-10, now Locked in
/// the design doc): evaluate `expression` (a real `JSONLogicExpr` input,
/// replacing the old free-text `strategy: String` the registry used to
/// have) against the current `RunContext`, store the result as
/// `outputs.result`.
async fn execute_transform(
    action: &Action,
    ctx: &RunContext,
) -> Result<Map<String, Value>, String> {
    let expr = action
        .inputs
        .get("expression")
        .ok_or_else(|| "transform: missing \"expression\" input".to_string())?;
    let result = ctx.evaluate(expr)?;
    let mut outputs = Map::new();
    outputs.insert("result".to_string(), result);
    Ok(outputs)
}

/// `DOCS/forge-workflow-engine-design.md` §4's already-locked contract:
/// `inputs.args` (if present) is serialized JSON piped to the process's
/// **stdin**; the script's **last line of stdout** must be a JSON object,
/// which becomes `outputs`; a non-zero exit is `Failed`. Spawns an
/// independent process per node — deliberately **not** routed through
/// `ScriptRunner` (`script_runner_panel.rs`), which is single-concurrency
/// by design and would silently serialize the exact thing this schema
/// exists to let run in parallel.
async fn execute_script(
    action: &Action,
    solution_root: &Path,
) -> Result<Map<String, Value>, String> {
    let source_value = action
        .inputs
        .get("source")
        .ok_or_else(|| "script: missing \"source\" input".to_string())?;
    let source: ScriptSource = serde_json::from_value(source_value.clone())
        .map_err(|e| format!("script: invalid \"source\": {e}"))?;
    let stdin_args = action.inputs.get("args").cloned();
    let solution_root = solution_root.to_path_buf();

    on_tokio(async move {
        let (program, args, _temp_guard) = resolve_script_invocation(&source, &solution_root)?;
        run_script_process(&program, &args, stdin_args).await
    })
    .await
}

/// Resolves what to actually spawn. For `Inline` mode, spills `code` to a
/// real temp file first (§4: "needed for a real interpreter invocation
/// either way") and returns it as the third element so the caller can keep
/// it alive (and get it cleaned up on drop) until the process has finished
/// reading it.
fn resolve_script_invocation(
    source: &ScriptSource,
    solution_root: &Path,
) -> Result<(String, Vec<String>, Option<tempfile::TempPath>), String> {
    let (runtime, path, temp_guard) = match source {
        ScriptSource::Inline { runtime, code } => {
            // §4: ".NET deliberately excluded at the JSON Schema level (no
            // equivalent to executing a loose code string)" — file mode's
            // .csproj is the natural fit for .NET instead.
            if *runtime == ScriptRuntime::Dotnet {
                return Err(
                    "script: inline mode doesn't support the \"dotnet\" runtime — use file mode with a .csproj"
                        .to_string(),
                );
            }
            let mut file = tempfile::Builder::new()
                .prefix("forge-workflow-")
                .suffix(&format!(".{}", extension_for(*runtime)))
                .tempfile()
                .map_err(|e| format!("script: failed to create a temp file: {e}"))?;
            std::io::Write::write_all(&mut file, code.as_bytes())
                .map_err(|e| format!("script: failed to write the temp file: {e}"))?;
            let temp_path = file.into_temp_path();
            (*runtime, temp_path.to_path_buf(), Some(temp_path))
        }
        ScriptSource::File { path, runtime } => {
            // §4: "always relative to the solution root, explicitly —
            // never relative to the running process's own working
            // directory" — the direct lesson from ForgeFlow's
            // `uri_to_path` bug.
            let resolved = solution_root.join(path);
            let runtime = match runtime {
                Some(r) => *r,
                None => infer_runtime_from_extension(&resolved).ok_or_else(|| {
                    format!("script: couldn't infer a runtime from \"{path}\" — give an explicit \"runtime\" override")
                })?,
            };
            (runtime, resolved, None)
        }
    };
    let (program, args) = build_command_args(runtime, &path);
    Ok((program, args, temp_guard))
}

fn extension_for(runtime: ScriptRuntime) -> &'static str {
    match runtime {
        ScriptRuntime::Python => "py",
        ScriptRuntime::Node => "js",
        ScriptRuntime::Powershell => "ps1",
        // Unreachable — inline mode rejects `Dotnet` before this is called.
        ScriptRuntime::Dotnet => "csproj",
    }
}

fn infer_runtime_from_extension(path: &Path) -> Option<ScriptRuntime> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "py" => Some(ScriptRuntime::Python),
        "js" | "ts" => Some(ScriptRuntime::Node),
        "ps1" => Some(ScriptRuntime::Powershell),
        "csproj" => Some(ScriptRuntime::Dotnet),
        _ => None,
    }
}

fn build_command_args(runtime: ScriptRuntime, path: &Path) -> (String, Vec<String>) {
    let path = path.to_string_lossy().to_string();
    match runtime {
        ScriptRuntime::Python => ("python".to_string(), vec![path]),
        ScriptRuntime::Node => ("node".to_string(), vec![path]),
        ScriptRuntime::Powershell => (
            "pwsh".to_string(),
            vec![
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-File".to_string(),
                path,
            ],
        ),
        ScriptRuntime::Dotnet => (
            "dotnet".to_string(),
            vec!["run".to_string(), "--project".to_string(), path],
        ),
    }
}

/// Spawns `program args...`, writes `stdin_json` (if any) to its stdin then
/// closes it (so the script sees EOF even when there's nothing to write),
/// and parses the last non-empty stdout line as the JSON-object `outputs`.
async fn run_script_process(
    program: &str,
    args: &[String],
    stdin_json: Option<Value>,
) -> Result<Map<String, Value>, String> {
    let mut cmd = TokioCommand::new(program);
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "windows")]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW — no console flash.

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("script: failed to start \"{program}\": {e}"))?;

    let mut stdin = child.stdin.take().expect("stdin was piped");
    if let Some(value) = stdin_json {
        let payload = serde_json::to_vec(&value)
            .map_err(|e| format!("script: failed to serialize \"args\": {e}"))?;
        stdin
            .write_all(&payload)
            .await
            .map_err(|e| format!("script: failed to write stdin: {e}"))?;
    }
    drop(stdin); // EOF, whether or not anything was written.

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("script: failed waiting for the process: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "script: exited with {} — {}",
            output.status,
            stderr.trim()
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let last_line = stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| {
            "script: no stdout output — the last line must be a JSON object".to_string()
        })?;
    match serde_json::from_str::<Value>(last_line) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err("script: last stdout line wasn't a JSON object".to_string()),
        Err(e) => Err(format!("script: last stdout line wasn't valid JSON: {e}")),
    }
}

async fn execute_delay(action: &Action) -> Result<Map<String, Value>, String> {
    let ms = require_f64(action, "ms")?;
    if ms < 0.0 {
        return Err("delay: \"ms\" must be >= 0".to_string());
    }
    on_tokio(async move {
        tokio::time::sleep(std::time::Duration::from_millis(ms as u64)).await;
    })
    .await;
    Ok(Map::new())
}

/// Webhook-based (decided 2026-09-12, superseding the original "UI-only
/// toast" v1 decision — no `Window`/`App` needed for this module to stay
/// gpui-free, just like `http`). POSTs a Slack/Mattermost-compatible
/// JSON body (`{"channel": ..., "text": ...}`, `channel` omitted when not
/// set — that shape is readable by a generic webhook receiver too, not
/// just those two specifically) to `url`. A non-2xx response or network
/// failure resolves `Failed`, same as `http`.
async fn execute_notify(action: &Action, ctx: &RunContext) -> Result<Map<String, Value>, String> {
    let url = resolve_string_field(action, "url", ctx)?;
    let channel = action
        .inputs
        .contains_key("channel")
        .then(|| resolve_string_field(action, "channel", ctx))
        .transpose()?;
    let message = resolve_string_field(action, "message", ctx)?;

    on_tokio(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| format!("notify: failed to build client: {e}"))?;

        let mut body = Map::new();
        if let Some(channel) = channel {
            body.insert("channel".to_string(), Value::String(channel));
        }
        body.insert("text".to_string(), Value::String(message));

        let body_text = serde_json::to_string(&Value::Object(body))
            .map_err(|e| format!("notify: failed to serialize body: {e}"))?;
        let res = client
            .post(&url)
            .header("Content-Type", "application/json")
            .body(body_text)
            .send()
            .await
            .map_err(|e| format!("notify: network error: {e}"))?;
        let status = res.status();
        if !status.is_success() {
            let text = res.text().await.unwrap_or_default();
            return Err(format!(
                "notify: webhook returned {} — {}",
                status.as_u16(),
                text.trim()
            ));
        }

        let mut outputs = Map::new();
        outputs.insert("status".to_string(), Value::from(status.as_u16()));
        Ok(outputs)
    })
    .await
}

async fn execute_log(action: &Action, ctx: &RunContext) -> Result<Map<String, Value>, String> {
    let level = action
        .inputs
        .get("level")
        .and_then(Value::as_str)
        .unwrap_or("info");
    let message = resolve_string_field(action, "message", ctx)?;
    match level {
        "error" => log::error!("[workflow] {message}"),
        "warn" | "warning" => log::warn!("[workflow] {message}"),
        "debug" => log::debug!("[workflow] {message}"),
        _ => log::info!("[workflow] {message}"),
    }
    Ok(Map::new())
}

/// Stubbed as log-only (design doc §4, decided 2026-09-10) — Forge has no
/// existing SMTP/email-sending infrastructure. Real delivery is separate
/// follow-up work once there's an actual transport to use.
async fn execute_send_email(
    action: &Action,
    ctx: &RunContext,
) -> Result<Map<String, Value>, String> {
    let to = resolve_string_field(action, "to", ctx)?;
    let subject = resolve_string_field(action, "subject", ctx)?;
    let body = resolve_string_field(action, "body", ctx)?;
    log::info!("[workflow sendEmail] would send to={to:?} subject={subject:?}: {body}");
    Ok(Map::new())
}

/// Direct port of the old `chain.rs`'s `execute_wait_for_port_step` — polls
/// a local TCP connect on an interval until it succeeds or `timeout_ms`
/// (default 30s) elapses.
async fn execute_wait_for_port(action: &Action) -> Result<Map<String, Value>, String> {
    let port = require_f64(action, "port")? as u16;
    let timeout_ms = action
        .inputs
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(30_000);

    on_tokio(async move {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        loop {
            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_ok()
            {
                let mut outputs = Map::new();
                outputs.insert("ready".to_string(), Value::Bool(true));
                return Ok(outputs);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "WaitForPort: port {port} not ready after {timeout_ms}ms"
                ));
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await
}

/// Starts a long-running/detached process (a dev server, typically) and
/// resolves `Succeeded` the moment it *starts* — not when it exits, unlike
/// every other executor in this file. `args` is a JSON array of strings
/// (`["run", "dev"]`), not one shell command-line string — keeps the schema
/// itself free of quoting ambiguity, even though the string that's actually
/// exec'd underneath does get shell-quoted on Windows (see below). `cwd`,
/// when given, resolves against `solution_root` (same convention as
/// `script`'s `file` mode). The spawned child is handed to
/// `process_tracker`'s registry, which owns it from here — stdout/stderr
/// are piped (`Stdio::piped()`), each drained by its own background reader
/// (`process_tracker::spawn_log_reader`, started right after `register`
/// hands back the tracking id) so the pipe buffer can never fill and block
/// the child; each line lands in that process's own retained log buffer
/// and is broadcast live to the Designer's Processes panel log viewer, if
/// one happens to be open for it. Pair with `WaitForPort` (`runAfter` this
/// action's own id) to confirm the process actually came up on whatever
/// port it's expected to serve.
///
/// **Windows-only wrinkle** (confirmed live: `command: "npm"` failed with
/// "program not found" even though `npm` runs fine from a terminal):
/// `Command::new` can only launch real PE executables directly.
/// `npm`/`npx`/`flask`/etc. are `.cmd`/`.bat` shims (or extensionless
/// scripts resolved via `PATHEXT`), which only the command interpreter can
/// run — the same reason `script_runner_panel::run_shell_blocking` already
/// routes its own shelled-out commands through `cmd.exe`/`pwsh.exe` rather
/// than exec'ing them directly. This does the same for `StartProcess`,
/// meaning the *tracked* child becomes `cmd.exe`, not the real dev server —
/// `process_tracker::stop`'s Windows tree-kill exists specifically to
/// account for that (killing just the direct child would orphan its real
/// descendant instead of stopping it).
async fn execute_start_process(
    action_id: &str,
    action: &Action,
    solution_root: &Path,
) -> Result<Map<String, Value>, String> {
    let command = require_str(action, "command")?.to_string();
    let args: Vec<String> = action
        .inputs
        .get("args")
        .map(|v| {
            v.as_array()
                .ok_or_else(|| "StartProcess: \"args\" must be an array of strings".to_string())?
                .iter()
                .map(|item| {
                    item.as_str().map(str::to_string).ok_or_else(|| {
                        "StartProcess: \"args\" must be an array of strings".to_string()
                    })
                })
                .collect::<Result<Vec<String>, String>>()
        })
        .transpose()?
        .unwrap_or_default();
    let cwd = action
        .inputs
        .get("cwd")
        .and_then(Value::as_str)
        .map(|c| solution_root.join(c));
    let url = action
        .inputs
        .get("url")
        .and_then(Value::as_str)
        .map(str::to_string);
    let action_id = action_id.to_string();

    on_tokio(async move {
        #[cfg(target_os = "windows")]
        let mut cmd = {
            let mut c = TokioCommand::new("cmd.exe");
            c.args(["/C", &windows_command_line(&command, &args)]);
            c
        };
        #[cfg(not(target_os = "windows"))]
        let mut cmd = {
            let mut c = TokioCommand::new(&command);
            c.args(&args);
            c
        };

        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &cwd {
            cmd.current_dir(cwd);
        }
        #[cfg(target_os = "windows")]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW — no console flash.

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("StartProcess: failed to start \"{command}\": {e}"))?;
        let pid = child.id();
        // Taken before `child` moves into `register` — piped stdout/stderr
        // are read by dedicated background readers (`spawn_log_reader`),
        // not by anything that owns `child` itself, so the Processes panel
        // can show live output without `process_tracker` needing to know
        // anything about reading pipes.
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let process_id = process_tracker::registry().register(action_id, command, args, url, child);
        if let Some(stdout) = stdout {
            process_tracker::spawn_log_reader(process_id.clone(), stdout, false);
        }
        if let Some(stderr) = stderr {
            process_tracker::spawn_log_reader(process_id.clone(), stderr, true);
        }

        let mut outputs = Map::new();
        outputs.insert(
            "pid".to_string(),
            pid.map(Value::from).unwrap_or(Value::Null),
        );
        outputs.insert("processId".to_string(), Value::String(process_id));
        Ok(outputs)
    })
    .await
}

/// Builds the `cmd.exe /C` command line for [`execute_start_process`] —
/// double-quotes any piece containing whitespace/quotes (covers the common
/// case: a path with spaces, an arg with an embedded flag value), not a
/// fully general `cmd.exe` escaper — `cmd.exe` quoting rules are notoriously
/// inconsistent even in the real shell, and `script_runner_panel`'s own
/// shelling-out doesn't attempt full generality either.
#[cfg(target_os = "windows")]
fn windows_command_line(command: &str, args: &[String]) -> String {
    let mut line = quote_windows_arg(command);
    for arg in args {
        line.push(' ');
        line.push_str(&quote_windows_arg(arg));
    }
    line
}

#[cfg(target_os = "windows")]
fn quote_windows_arg(arg: &str) -> String {
    if arg.is_empty() || arg.chars().any(|c| c.is_whitespace() || c == '"') {
        format!("\"{}\"", arg.replace('"', "\\\""))
    } else {
        arg.to_string()
    }
}

/// Direct port of the old `chain.rs`'s `execute_set_env_step`. Process-
/// global, same as the original — not sandboxed per-run.
fn execute_set_env(action: &Action) -> Result<Map<String, Value>, String> {
    let key = require_str(action, "key")?;
    let value = require_str(action, "value")?;
    // SAFETY: matches the existing `unsafe { std::env::set_var(...) }`
    // call sites already in this codebase (e.g. `zed/util/src/util.rs`) —
    // edition 2024 marks `set_var` unsafe because it's not thread-safe to
    // race with a concurrent `getenv` on some platforms; this call site
    // has no such concurrent reader.
    unsafe {
        std::env::set_var(key, value);
    }
    Ok(Map::new())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn action_with_inputs(type_id: &str, inputs: Vec<(&str, Value)>) -> Action {
        let mut action = Action::new(type_id);
        for (k, v) in inputs {
            action.inputs.insert(k.to_string(), v);
        }
        action
    }

    /// Spawns a one-shot raw HTTP mock server on an OS-assigned local port
    /// (no real network access, no external dependency) that answers every
    /// connection with `body` as a `200 application/json` response, then
    /// returns the `http://127.0.0.1:<port>/` URL to hit it.
    async fn mock_json_server(body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        });
        format!("http://127.0.0.1:{port}/")
    }

    #[tokio::test]
    async fn http_get_parses_json_body_and_status() {
        let url = mock_json_server(r#"{"total": 42}"#).await;
        let action = action_with_inputs(
            "http",
            vec![("method", Value::from("GET")), ("url", Value::from(url))],
        );
        let outputs = execute_http(&action).await.unwrap();
        assert_eq!(outputs.get("status"), Some(&Value::from(200)));
        assert_eq!(outputs.get("body"), Some(&serde_json::json!({"total": 42})));
    }

    #[tokio::test]
    async fn http_rejects_a_missing_url() {
        let action = action_with_inputs("http", vec![("method", Value::from("GET"))]);
        let err = execute_http(&action).await.unwrap_err();
        assert!(err.contains("url"));
    }

    #[tokio::test]
    async fn transform_evaluates_expression_against_run_context() {
        let ctx = RunContext::new();
        ctx.record_outputs(
            "fetchOrder",
            serde_json::json!({"total": 42})
                .as_object()
                .unwrap()
                .clone(),
        );
        let action = action_with_inputs(
            "transform",
            vec![(
                "expression",
                serde_json::json!({"var": "fetchOrder.outputs.total"}),
            )],
        );
        let outputs = execute_transform(&action, &ctx).await.unwrap();
        assert_eq!(outputs.get("result"), Some(&Value::from(42)));
    }

    #[tokio::test]
    async fn delay_actually_waits_at_least_the_requested_time() {
        let action = action_with_inputs("delay", vec![("ms", Value::from(30))]);
        let started = std::time::Instant::now();
        execute_delay(&action).await.unwrap();
        assert!(started.elapsed() >= std::time::Duration::from_millis(30));
    }

    #[tokio::test]
    async fn wait_for_port_succeeds_once_something_is_listening() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        // Keep the listener alive for the duration of the wait.
        let _keep_alive = tokio::spawn(async move {
            let _ = listener.accept().await;
        });

        let action = action_with_inputs(
            "WaitForPort",
            vec![
                ("port", Value::from(port)),
                ("timeout_ms", Value::from(2000)),
            ],
        );
        let outputs = execute_wait_for_port(&action).await.unwrap();
        assert_eq!(outputs.get("ready"), Some(&Value::Bool(true)));
    }

    #[tokio::test]
    async fn wait_for_port_times_out_when_nothing_is_listening() {
        // A bound-then-immediately-dropped listener frees the port but is
        // very likely nothing else grabs it in the ~150ms this test runs.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let action = action_with_inputs(
            "WaitForPort",
            vec![
                ("port", Value::from(port)),
                ("timeout_ms", Value::from(150)),
            ],
        );
        let err = execute_wait_for_port(&action).await.unwrap_err();
        assert!(err.contains("not ready"));
    }

    #[tokio::test]
    async fn start_process_registers_a_running_process_and_returns_a_pid() {
        #[cfg(target_os = "windows")]
        let (command, args) = ("cmd", vec!["/C".to_string(), "timeout /T 5".to_string()]);
        #[cfg(not(target_os = "windows"))]
        let (command, args) = ("sleep", vec!["5".to_string()]);

        let action = action_with_inputs(
            "StartProcess",
            vec![
                ("command", Value::from(command)),
                ("args", serde_json::json!(args)),
            ],
        );
        let outputs = execute_start_process("myStart", &action, std::path::Path::new("."))
            .await
            .unwrap();
        assert!(outputs.get("pid").is_some());
        let process_id = outputs
            .get("processId")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();

        let list = process_tracker::registry().list();
        let info = list.iter().find(|p| p.id == process_id).unwrap();
        assert_eq!(info.action_id, "myStart");
        assert_eq!(info.status, process_tracker::ProcessStatus::Running);

        // Cleanup so this test doesn't leave a lingering process behind.
        process_tracker::registry().stop(&process_id).unwrap();
    }

    #[tokio::test]
    async fn start_process_rejects_non_array_args() {
        let action = action_with_inputs(
            "StartProcess",
            vec![
                ("command", Value::from("echo")),
                ("args", Value::from("not an array")),
            ],
        );
        let err = execute_start_process("s1", &action, std::path::Path::new("."))
            .await
            .unwrap_err();
        assert!(err.contains("array"));
    }

    // Not `start_process_fails_clearly_for_a_nonexistent_command` any more —
    // on Windows, `StartProcess` now always launches through `cmd.exe /C`
    // (see `execute_start_process`'s doc comment), and `cmd.exe` itself
    // spawns successfully regardless of whether the *inner* command exists;
    // that failure now happens asynchronously inside the (headless,
    // output-discarded) shell, not as a synchronous spawn error here. A
    // nonexistent `cwd`, by contrast, still fails `spawn()` itself on every
    // platform (the OS validates the working directory before starting
    // anything) — this is the assertion that survived that change.
    #[tokio::test]
    async fn start_process_fails_clearly_when_cwd_does_not_exist() {
        let action = action_with_inputs(
            "StartProcess",
            vec![
                ("command", Value::from("cmd")),
                ("cwd", Value::from("this/dir/does/not/exist")),
            ],
        );
        let err = execute_start_process("s1", &action, std::path::Path::new("."))
            .await
            .unwrap_err();
        assert!(err.contains("failed to start"));
    }

    #[test]
    fn set_env_sets_a_process_env_var() {
        let action = action_with_inputs(
            "SetEnv",
            vec![
                ("key", Value::from("FORGE_WORKFLOW_TEST_VAR")),
                ("value", Value::from("hello")),
            ],
        );
        execute_set_env(&action).unwrap();
        assert_eq!(std::env::var("FORGE_WORKFLOW_TEST_VAR").unwrap(), "hello");
    }

    #[tokio::test]
    async fn log_and_send_email_succeed_with_no_outputs() {
        let ctx = RunContext::new();
        let log_action = action_with_inputs(
            "log",
            vec![
                ("level", Value::from("info")),
                ("message", Value::from("hi")),
            ],
        );
        assert!(execute_log(&log_action, &ctx).await.unwrap().is_empty());

        let email_action = action_with_inputs(
            "sendEmail",
            vec![
                ("to", Value::from("a@b.com")),
                ("subject", Value::from("subj")),
                ("body", Value::from("body")),
            ],
        );
        assert!(
            execute_send_email(&email_action, &ctx)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// Spawns a one-shot raw HTTP mock server that records the request it
    /// received (method/path/body) into `received`, then answers with
    /// `status` and an empty JSON object — used for `notify`'s webhook
    /// POST, where the *outgoing* request shape matters more than the
    /// response.
    async fn mock_webhook_server(
        status: u16,
        received: Arc<Mutex<Option<(String, String, String)>>>,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = socket.read(&mut buf).await.unwrap_or(0);
            let request_text = String::from_utf8_lossy(&buf[..n]).to_string();
            let mut lines = request_text.split("\r\n");
            let request_line = lines.next().unwrap_or_default().to_string();
            let mut parts = request_line.split(' ');
            let method = parts.next().unwrap_or_default().to_string();
            let path = parts.next().unwrap_or_default().to_string();
            let body = request_text
                .split("\r\n\r\n")
                .nth(1)
                .unwrap_or_default()
                .to_string();
            *received.lock().unwrap() = Some((method, path, body));

            let status_line = match status {
                200 => "200 OK",
                _ => "500 Internal Server Error",
            };
            let response = format!(
                "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        });
        format!("http://127.0.0.1:{port}/webhook")
    }

    #[tokio::test]
    async fn notify_posts_a_slack_compatible_webhook_body() {
        let ctx = RunContext::new();
        let received = Arc::new(Mutex::new(None));
        let url = mock_webhook_server(200, received.clone()).await;

        let notify_action = action_with_inputs(
            "notify",
            vec![
                ("url", Value::from(url)),
                ("channel", Value::from("ops")),
                ("message", Value::from("hi")),
            ],
        );
        let outputs = execute_notify(&notify_action, &ctx).await.unwrap();
        assert_eq!(outputs.get("status"), Some(&Value::from(200)));

        let (method, path, body) = received.lock().unwrap().clone().unwrap();
        assert_eq!(method, "POST");
        assert_eq!(path, "/webhook");
        let parsed: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed.get("channel"), Some(&Value::from("ops")));
        assert_eq!(parsed.get("text"), Some(&Value::from("hi")));
    }

    #[tokio::test]
    async fn notify_omits_channel_from_the_body_when_not_set() {
        let ctx = RunContext::new();
        let received = Arc::new(Mutex::new(None));
        let url = mock_webhook_server(200, received.clone()).await;

        let notify_action = action_with_inputs(
            "notify",
            vec![("url", Value::from(url)), ("message", Value::from("hi"))],
        );
        execute_notify(&notify_action, &ctx).await.unwrap();

        let (_, _, body) = received.lock().unwrap().clone().unwrap();
        let parsed: Value = serde_json::from_str(&body).unwrap();
        assert!(parsed.get("channel").is_none());
    }

    #[tokio::test]
    async fn notify_fails_on_a_non_2xx_webhook_response() {
        let ctx = RunContext::new();
        let received = Arc::new(Mutex::new(None));
        let url = mock_webhook_server(500, received.clone()).await;

        let notify_action = action_with_inputs(
            "notify",
            vec![("url", Value::from(url)), ("message", Value::from("hi"))],
        );
        let err = execute_notify(&notify_action, &ctx).await.unwrap_err();
        assert!(err.contains("500"), "{err}");
    }

    #[tokio::test]
    async fn log_message_can_be_a_jsonlogic_expression_referencing_upstream_outputs() {
        // Exactly the `.forge/order-processing.flow.json` pattern that
        // exposed this: `message: {"var": "tryCharge.outputs.error.message"}`
        // instead of a plain string.
        let ctx = RunContext::new();
        ctx.record_outputs(
            "tryCharge",
            serde_json::json!({"error": {"message": "boom"}})
                .as_object()
                .unwrap()
                .clone(),
        );
        let log_action = action_with_inputs(
            "log",
            vec![
                ("level", Value::from("error")),
                (
                    "message",
                    serde_json::json!({"var": "tryCharge.outputs.error.message"}),
                ),
            ],
        );
        assert!(execute_log(&log_action, &ctx).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn notify_channel_and_send_email_recipient_fields_also_accept_expressions() {
        let ctx = RunContext::new();
        ctx.record_outputs(
            "fetchCustomer",
            serde_json::json!({"email": "a@b.com", "tier": "ops"})
                .as_object()
                .unwrap()
                .clone(),
        );

        let received = Arc::new(Mutex::new(None));
        let url = mock_webhook_server(200, received.clone()).await;
        let notify_action = action_with_inputs(
            "notify",
            vec![
                ("url", Value::from(url)),
                (
                    "channel",
                    serde_json::json!({"var": "fetchCustomer.outputs.tier"}),
                ),
                ("message", Value::from("hi")),
            ],
        );
        execute_notify(&notify_action, &ctx).await.unwrap();
        let (_, _, body) = received.lock().unwrap().clone().unwrap();
        let parsed: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed.get("channel"), Some(&Value::from("ops")));

        let email_action = action_with_inputs(
            "sendEmail",
            vec![
                (
                    "to",
                    serde_json::json!({"var": "fetchCustomer.outputs.email"}),
                ),
                ("subject", Value::from("subj")),
                ("body", Value::from("body")),
            ],
        );
        assert!(
            execute_send_email(&email_action, &ctx)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn execute_leaf_reports_unimplemented_and_unknown_types_clearly() {
        let ctx = RunContext::new();
        let root = std::path::Path::new(".");

        let script_action = Action::new("script"); // no "source" input
        let err = execute_leaf("s1", &script_action, &ctx, root)
            .await
            .unwrap_err();
        assert!(err.contains("script"));

        let db_action = Action::new("DbMigration");
        let err = execute_leaf("d1", &db_action, &ctx, root)
            .await
            .unwrap_err();
        assert!(err.contains("not implemented"));

        let unknown = Action::new("TotallyMadeUp");
        let err = execute_leaf("u1", &unknown, &ctx, root).await.unwrap_err();
        assert!(err.contains("unknown action type"));
    }

    fn python_available() -> bool {
        std::process::Command::new("python")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn extension_inference_covers_known_and_unknown_extensions() {
        assert_eq!(
            infer_runtime_from_extension(Path::new("x.py")),
            Some(ScriptRuntime::Python)
        );
        assert_eq!(
            infer_runtime_from_extension(Path::new("x.js")),
            Some(ScriptRuntime::Node)
        );
        assert_eq!(
            infer_runtime_from_extension(Path::new("x.ts")),
            Some(ScriptRuntime::Node)
        );
        assert_eq!(
            infer_runtime_from_extension(Path::new("x.ps1")),
            Some(ScriptRuntime::Powershell)
        );
        assert_eq!(
            infer_runtime_from_extension(Path::new("x.csproj")),
            Some(ScriptRuntime::Dotnet)
        );
        assert_eq!(infer_runtime_from_extension(Path::new("x.exe")), None);
    }

    #[tokio::test]
    async fn script_inline_dotnet_is_rejected_without_spawning_anything() {
        let mut action = Action::new("script");
        action.inputs.insert(
            "source".to_string(),
            serde_json::json!({"kind": "inline", "runtime": "dotnet", "code": "// n/a"}),
        );
        let err = execute_script(&action, Path::new(".")).await.unwrap_err();
        assert!(err.contains("dotnet"));
    }

    #[tokio::test]
    async fn script_file_mode_requires_runtime_when_extension_is_unrecognized() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("thing.xyz"), "whatever").unwrap();
        let mut action = Action::new("script");
        action.inputs.insert(
            "source".to_string(),
            serde_json::json!({"kind": "file", "path": "thing.xyz"}),
        );
        let err = execute_script(&action, dir.path()).await.unwrap_err();
        assert!(err.contains("infer"));
    }

    #[tokio::test]
    async fn script_inline_python_round_trips_stdin_args_to_outputs() {
        if !python_available() {
            eprintln!("skipping: python not on PATH");
            return;
        }
        let code = "import json, sys\ndata = json.loads(sys.stdin.read() or '{}')\nprint(json.dumps({'doubled': data.get('n', 0) * 2}))\n";
        let mut action = Action::new("script");
        action.inputs.insert(
            "source".to_string(),
            serde_json::json!({"kind": "inline", "runtime": "python", "code": code}),
        );
        action
            .inputs
            .insert("args".to_string(), serde_json::json!({"n": 21}));

        let outputs = execute_script(&action, Path::new(".")).await.unwrap();
        assert_eq!(outputs.get("doubled"), Some(&Value::from(42)));
    }

    #[tokio::test]
    async fn script_nonzero_exit_is_failed() {
        if !python_available() {
            eprintln!("skipping: python not on PATH");
            return;
        }
        let mut action = Action::new("script");
        action.inputs.insert(
            "source".to_string(),
            serde_json::json!({"kind": "inline", "runtime": "python", "code": "import sys\nsys.exit(3)\n"}),
        );
        let err = execute_script(&action, Path::new(".")).await.unwrap_err();
        assert!(err.contains("exited with"));
    }

    #[tokio::test]
    async fn script_last_stdout_line_must_be_a_json_object() {
        if !python_available() {
            eprintln!("skipping: python not on PATH");
            return;
        }
        let mut action = Action::new("script");
        action.inputs.insert(
            "source".to_string(),
            serde_json::json!({"kind": "inline", "runtime": "python", "code": "print('not json')\n"}),
        );
        let err = execute_script(&action, Path::new(".")).await.unwrap_err();
        assert!(err.contains("JSON"));
    }

    #[tokio::test]
    async fn script_file_mode_resolves_against_solution_root_and_infers_runtime() {
        if !python_available() {
            eprintln!("skipping: python not on PATH");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("calc.py"), "print('{\"ok\": true}')\n").unwrap();
        let mut action = Action::new("script");
        action.inputs.insert(
            "source".to_string(),
            serde_json::json!({"kind": "file", "path": "calc.py"}),
        );

        let outputs = execute_script(&action, dir.path()).await.unwrap();
        assert_eq!(outputs.get("ok"), Some(&Value::Bool(true)));
    }
}
