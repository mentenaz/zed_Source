use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui_flow::FlowState;
use workflow_engine::{ActionMap, ScriptRuntime, ScriptSource, WorkflowDefinition};

pub fn companion_path(
    flow_path: &Path,
    action_path: &[String],
    runtime: ScriptRuntime,
) -> Result<PathBuf, String> {
    let file_name = flow_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid flow path: {}", flow_path.display()))?;
    let suffix = ".flow.json";
    let suffix_start = file_name
        .len()
        .checked_sub(suffix.len())
        .ok_or_else(|| format!("{} doesn't end in .flow.json", flow_path.display()))?;
    if !file_name[suffix_start..].eq_ignore_ascii_case(suffix) {
        return Err(format!("{} doesn't end in .flow.json", flow_path.display()));
    }
    let flow_name = &file_name[..suffix_start];
    let (action_id, scopes) = action_path
        .split_last()
        .ok_or_else(|| "script action path is empty".to_string())?;

    let directory = flow_path.with_file_name(format!("{flow_name}.flow-scripts"));
    let directory = scopes.iter().fold(directory, |path, component| {
        path.join(companion_component(component))
    });
    // `ScriptRuntime::extension` is the engine's single source of truth for the
    // runtime <-> extension mapping, so a companion is named with the same
    // extension the executor will later infer that runtime from.
    let extension = runtime.extension();
    Ok(directory.join(format!("{}.{extension}", companion_component(action_id))))
}

pub fn action_path_for_node(
    definition: &WorkflowDefinition,
    state: &FlowState,
    node_id: &str,
) -> Option<Vec<String>> {
    let target = state
        .nodes
        .iter()
        .find(|node| node.id.as_ref() == node_id)?;
    let mut parent_chain = Vec::new();
    let mut parent_id = target.parent_id.clone();
    while let Some(id) = parent_id {
        let parent = state.nodes.iter().find(|node| node.id == id)?;
        parent_chain.push(id.to_string());
        parent_id = parent.parent_id.clone();
    }
    parent_chain.reverse();

    find_action_path(
        &definition.actions,
        node_id,
        &parent_chain,
        &mut Vec::new(),
        &mut Vec::new(),
    )
}

pub fn source_for_action_path(
    definition: &WorkflowDefinition,
    action_path: &[String],
) -> Option<ScriptSource> {
    let action = find_action_by_path(&definition.actions, action_path)?;
    let source = action.inputs.get("source")?;
    serde_json::from_value(source.clone()).ok()
}

/// What one sync pass did to a single inline script's code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sync {
    /// No companion existed yet, so the flow's inline code became it.
    Created,
    /// The flow's inline code changed since the last sync and was written
    /// through to the (unchanged) companion. This is the case the panel used
    /// to drop on the floor: it created the companion once and then never
    /// touched it again, while every save pulled the companion back over the
    /// inline code - so anything typed into the inspector was silently lost.
    Written,
    /// The companion changed on disk since the last sync and was pulled back
    /// into the flow's inline code.
    Hydrated,
    /// The flow and its companion already agree.
    InSync,
}

/// Stable key for a script action across a sync pass: the action path joined
/// by `/`. Unambiguous because no action id can contain one - every path
/// component is percent-escaped by [`companion_component`] on its way into a
/// companion path.
pub fn action_key(action_path: &[String]) -> String {
    action_path.join("/")
}

/// The companion content last synced for each script action, keyed by
/// [`action_key`].
///
/// This is what makes the two-way sync lossless. Without it there is no way
/// to tell "the user edited the inline code in the inspector" from "the user
/// edited the companion in an external editor", so every rule has to pick a
/// winner unconditionally and one of the two edits dies. With it, the only
/// ambiguous case is *both* sides changing, which [`sync_inline_scripts`]
/// refuses instead of guessing.
#[derive(Default)]
pub struct SyncState {
    last_synced: HashMap<String, String>,
}

impl SyncState {
    /// The content recorded for `action_path` at the last sync, or `None` if
    /// this session hasn't synced it. `None` means *unknown* - not "the flow
    /// is authoritative" - so callers fall back to the documented rule that a
    /// saved companion is the latest source.
    pub fn last_synced(&self, action_path: &[String]) -> Option<&str> {
        self.last_synced
            .get(&action_key(action_path))
            .map(String::as_str)
    }

    /// Records `code` as the content `action_path` and its companion agree on.
    pub fn record(&mut self, action_path: &[String], code: &str) {
        self.last_synced
            .insert(action_key(action_path), code.to_string());
    }
}

/// The `source`-property text for `action_path` in `definition` - the exact
/// inverse of what `workflow_json` does when loading a flow, so a source
/// [`sync_inline_scripts`] hydrated shows up in the inspector spelled the way
/// a freshly loaded one is.
pub fn source_text_for_action_path(
    definition: &WorkflowDefinition,
    action_path: &[String],
) -> Option<String> {
    let action = find_action_by_path(&definition.actions, action_path)?;
    Some(crate::workflow_json::value_to_string(
        action.inputs.get("source")?,
    ))
}

/// Reconciles every inline script in `definition` with its companion file, in
/// both directions, and reports what happened to each action by
/// [`action_key`].
///
/// The rule, per action: whichever side changed since `state` last saw the
/// pair wins, and if *both* sides changed this returns `Err` instead of
/// choosing a loser. With no record for an action (`state` is empty - the
/// first pass after opening the flow) the documented rule applies instead: a
/// saved companion is the latest source, so it hydrates the flow.
///
/// Companion files are only ever created and written for `Inline` sources. A
/// `File` source points at a workspace file the executor reads directly, so
/// it is left entirely alone here - including never deleted, per the design
/// doc's "never delete the original file as a side effect".
pub fn sync_inline_scripts(
    definition: &mut WorkflowDefinition,
    flow_path: &Path,
    state: &mut SyncState,
) -> Result<Vec<(String, Sync)>, String> {
    let mut report = Vec::new();
    sync_action_map(
        &mut definition.actions,
        flow_path,
        &mut Vec::new(),
        state,
        &mut report,
    )?;
    Ok(report)
}

/// Writes the flow's inline code for `action_path` through to its companion,
/// for the inspector's "Open Script" button.
///
/// Unlike [`sync_inline_scripts`] this never hydrates. The caller wants to
/// show the user the companion for the code they can see in the inspector,
/// so pulling the file back into a throwaway definition would be worse than
/// useless: it would record a sync for content the canvas doesn't have, and
/// the next save would "write through" the stale inline code over the user's
/// external edit. A companion that changed on disk since the last sync is
/// therefore reported as a conflict, not silently overwritten.
pub fn write_through(
    definition: &WorkflowDefinition,
    flow_path: &Path,
    action_path: &[String],
    state: &mut SyncState,
) -> Result<Sync, String> {
    let key = action_key(action_path);
    let action = find_action_by_path(&definition.actions, action_path)
        .ok_or_else(|| format!("no script action at {key}"))?;
    let Some(ScriptSource::Inline { runtime, code }) = action
        .inputs
        .get("source")
        .and_then(|value| serde_json::from_value::<ScriptSource>(value.clone()).ok())
    else {
        return Err(format!("script action {key} isn't an Inline script"));
    };

    let path = companion_path(flow_path, action_path, runtime)?;
    let file_code = match std::fs::read_to_string(&path) {
        Ok(file_code) => file_code,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            write_companion(&path, &code)?;
            state.record(action_path, &code);
            return Ok(Sync::Created);
        }
        Err(error) => return Err(format!("couldn't read {}: {error}", path.display())),
    };
    if file_code == code {
        state.record(action_path, &code);
        return Ok(Sync::InSync);
    }
    match state.last_synced(action_path) {
        Some(previous) if previous == file_code => {
            write_companion(&path, &code)?;
            state.record(action_path, &code);
            Ok(Sync::Written)
        }
        _ => Err(conflict_error(&path, action_path)),
    }
}

/// Writes `code` to the companion at `path`, creating its directory and
/// replacing any existing file.
///
/// Write-then-rename rather than `create_new` plus manual cleanup (what this
/// replaced): a failed write leaves the previous companion intact instead of a
/// truncated one, and re-writing an existing companion - the normal case once
/// a script has one - isn't an error condition.
pub fn write_companion(path: &Path, code: &str) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Err(format!("{} has no parent directory", path.display()));
    };
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("couldn't create {}: {error}", parent.display()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid companion path: {}", path.display()))?;
    let staging = parent.join(format!(".{file_name}.staged"));
    if let Err(error) = std::fs::write(&staging, code) {
        let _ = std::fs::remove_file(&staging);
        return Err(format!("couldn't write {}: {error}", staging.display()));
    }
    if let Err(error) = std::fs::rename(&staging, path) {
        let _ = std::fs::remove_file(&staging);
        return Err(format!("couldn't replace {}: {error}", path.display()));
    }
    Ok(())
}

/// The single message for "both sides of this script changed", shared by the
/// sync pass and the write-through path so the advice can't drift apart.
fn conflict_error(path: &Path, action_path: &[String]) -> String {
    format!(
        "{} and the inline code for script {} both changed since the last sync - \
         reconcile them by hand, or delete the companion to go back to inline-only",
        path.display(),
        action_path.join("/")
    )
}

fn find_action_path(
    actions: &ActionMap,
    target_id: &str,
    target_parent_chain: &[String],
    parent_chain: &mut Vec<String>,
    path: &mut Vec<String>,
) -> Option<Vec<String>> {
    for (id, action) in actions {
        if id == target_id && parent_chain.as_slice() == target_parent_chain {
            let mut result = path.clone();
            result.push(id.clone());
            return Some(result);
        }

        path.push(id.clone());
        match action.type_id.as_str() {
            "Foreach" | "Until" => {
                if let Some(body) = &action.actions {
                    parent_chain.push(id.clone());
                    let found =
                        find_action_path(body, target_id, target_parent_chain, parent_chain, path);
                    parent_chain.pop();
                    if found.is_some() {
                        return found;
                    }
                }
            }
            "If" => {
                if let Some(body) = &action.actions {
                    path.push("then".to_string());
                    parent_chain.push(id.clone());
                    parent_chain.push(format!("{id}__branch_then"));
                    let found =
                        find_action_path(body, target_id, target_parent_chain, parent_chain, path);
                    parent_chain.pop();
                    parent_chain.pop();
                    path.pop();
                    if found.is_some() {
                        return found;
                    }
                }
                if let Some(branch) = &action.else_branch {
                    path.push("else".to_string());
                    parent_chain.push(id.clone());
                    parent_chain.push(format!("{id}__branch_else"));
                    let found = find_action_path(
                        &branch.actions,
                        target_id,
                        target_parent_chain,
                        parent_chain,
                        path,
                    );
                    parent_chain.pop();
                    parent_chain.pop();
                    path.pop();
                    if found.is_some() {
                        return found;
                    }
                }
            }
            "Try" => {
                if let Some(body) = &action.actions {
                    path.push("try".to_string());
                    parent_chain.push(id.clone());
                    parent_chain.push(format!("{id}__branch_try"));
                    let found =
                        find_action_path(body, target_id, target_parent_chain, parent_chain, path);
                    parent_chain.pop();
                    parent_chain.pop();
                    path.pop();
                    if found.is_some() {
                        return found;
                    }
                }
                if let Some(branch) = &action.catch {
                    path.push("catch".to_string());
                    parent_chain.push(id.clone());
                    parent_chain.push(format!("{id}__branch_catch"));
                    let found = find_action_path(
                        &branch.actions,
                        target_id,
                        target_parent_chain,
                        parent_chain,
                        path,
                    );
                    parent_chain.pop();
                    parent_chain.pop();
                    path.pop();
                    if found.is_some() {
                        return found;
                    }
                }
            }
            _ => {}
        }
        path.pop();
    }
    None
}

fn sync_action_map(
    actions: &mut ActionMap,
    flow_path: &Path,
    action_path: &mut Vec<String>,
    state: &mut SyncState,
    report: &mut Vec<(String, Sync)>,
) -> Result<(), String> {
    for (id, action) in actions {
        action_path.push(id.clone());
        if action.type_id == "script"
            && let Some(source_value) = action.inputs.get_mut("source")
            && let Ok(ScriptSource::Inline { runtime, code }) =
                serde_json::from_value::<ScriptSource>(source_value.clone())
        {
            let path = companion_path(flow_path, action_path, runtime)?;
            let file_code = match std::fs::read_to_string(&path) {
                Ok(file_code) => Some(file_code),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => {
                    return Err(format!("couldn't read {}: {error}", path.display()));
                }
            };
            // Cloned rather than borrowed: `state` is recorded into below,
            // and a `&str` into it would still be live at that point.
            let previous = state.last_synced(action_path).map(str::to_string);
            let mut effective = code.clone();
            let outcome = match (previous, file_code) {
                // No companion yet: the flow's inline code becomes it.
                (_, None) => {
                    write_companion(&path, &code)?;
                    Sync::Created
                }
                (_, Some(file_code)) if file_code == code => Sync::InSync,
                // The companion is byte-for-byte what we last synced and the
                // flow's inline code moved: the user edited the inline code,
                // so it wins and is written through.
                (Some(previous), Some(file_code)) if file_code == previous => {
                    write_companion(&path, &code)?;
                    Sync::Written
                }
                // The flow's inline code is what we last synced and the
                // companion moved: the user edited the file, so it wins.
                (Some(previous), Some(file_code)) if previous == code => {
                    effective = file_code;
                    Sync::Hydrated
                }
                // No record for this action - the flow was loaded before any
                // edit could be tracked, so the documented rule is all there
                // is: a saved companion is the latest source.
                (None, Some(file_code)) => {
                    effective = file_code;
                    Sync::Hydrated
                }
                // Both sides moved since the last sync. Any winner destroys
                // one of the two edits, so refuse and let the user decide.
                (Some(_), Some(_)) => return Err(conflict_error(&path, action_path)),
            };
            if matches!(outcome, Sync::Hydrated) {
                *source_value = serde_json::to_value(ScriptSource::Inline {
                    runtime,
                    code: effective.clone(),
                })
                .map_err(|error| format!("couldn't serialize script source: {error}"))?;
            }
            state.record(action_path, &effective);
            report.push((action_key(action_path), outcome));
        }

        match action.type_id.as_str() {
            "Foreach" | "Until" => {
                if let Some(body) = &mut action.actions {
                    sync_action_map(body, flow_path, action_path, state, report)?;
                }
            }
            "If" => {
                if let Some(body) = &mut action.actions {
                    action_path.push("then".to_string());
                    sync_action_map(body, flow_path, action_path, state, report)?;
                    action_path.pop();
                }
                if let Some(branch) = &mut action.else_branch {
                    action_path.push("else".to_string());
                    sync_action_map(&mut branch.actions, flow_path, action_path, state, report)?;
                    action_path.pop();
                }
            }
            "Try" => {
                if let Some(body) = &mut action.actions {
                    action_path.push("try".to_string());
                    sync_action_map(body, flow_path, action_path, state, report)?;
                    action_path.pop();
                }
                if let Some(branch) = &mut action.catch {
                    action_path.push("catch".to_string());
                    sync_action_map(&mut branch.actions, flow_path, action_path, state, report)?;
                    action_path.pop();
                }
            }
            _ => {}
        }
        action_path.pop();
    }
    Ok(())
}

pub(crate) fn find_action_by_path<'a>(
    actions: &'a ActionMap,
    path: &[String],
) -> Option<&'a workflow_engine::Action> {
    let (id, remainder) = path.split_first()?;
    let action = actions.get(id)?;
    if remainder.is_empty() {
        return Some(action);
    }
    match action.type_id.as_str() {
        "Foreach" | "Until" => find_action_by_path(action.actions.as_ref()?, remainder),
        "If" => match remainder.first()?.as_str() {
            "then" => find_action_by_path(action.actions.as_ref()?, &remainder[1..]),
            "else" => find_action_by_path(&action.else_branch.as_ref()?.actions, &remainder[1..]),
            _ => None,
        },
        "Try" => match remainder.first()?.as_str() {
            "try" => find_action_by_path(action.actions.as_ref()?, &remainder[1..]),
            "catch" => find_action_by_path(&action.catch.as_ref()?.actions, &remainder[1..]),
            _ => None,
        },
        _ => None,
    }
}

/// Percent-escapes one action-path component for use as a single path
/// segment. Shared with `http_companion` so a body companion lands in the same
/// escaped layout as a script companion — and, more importantly, so an action
/// id containing a separator or `..` can never escape the companion directory.
pub(crate) fn companion_component(component: &str) -> String {
    let mut safe = String::new();
    for byte in component.bytes() {
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' {
            safe.push(byte as char);
        } else {
            safe.push_str(&format!("%{byte:02X}"));
        }
    }
    if safe.is_empty() {
        safe.push_str("%00");
    }
    safe
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use workflow_engine::{Action, ActionMap, Branch, ScriptRuntime, WorkflowDefinition};

    static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

    fn temp_dir() -> PathBuf {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "designer-script-companion-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn inline_script(code: &str) -> Action {
        let mut action = Action::new("script");
        action.inputs.insert("source".into(), inline_source(code));
        action
    }

    fn inline_source(code: &str) -> serde_json::Value {
        serde_json::to_value(ScriptSource::Inline {
            runtime: ScriptRuntime::Python,
            code: code.to_string(),
        })
        .unwrap()
    }

    fn definition() -> WorkflowDefinition {
        let mut nested = ActionMap::new();
        nested.insert("same-id".to_string(), inline_script("nested original"));
        let mut actions = ActionMap::new();
        let mut condition = Action::new("If");
        condition.actions = Some(nested);
        condition.else_branch = Some(Branch {
            actions: {
                let mut branch = ActionMap::new();
                branch.insert("same-id".to_string(), inline_script("else original"));
                branch
            },
        });
        actions.insert("condition".to_string(), condition);
        actions.insert("same-id".to_string(), inline_script("root original"));
        WorkflowDefinition {
            id: "sample".to_string(),
            name: "Sample".to_string(),
            actions,
            outputs: Default::default(),
        }
    }

    #[test]
    fn nested_scripts_get_separate_sibling_companions() {
        let root = temp_dir();
        let flow = root.join("sample.flow.json");
        let root_action = vec!["same-id".to_string()];
        let then_action = vec![
            "condition".to_string(),
            "then".to_string(),
            "same-id".to_string(),
        ];
        let else_action = vec![
            "condition".to_string(),
            "else".to_string(),
            "same-id".to_string(),
        ];
        let root_path = companion_path(&flow, &root_action, ScriptRuntime::Python).unwrap();
        let then_path = companion_path(&flow, &then_action, ScriptRuntime::Python).unwrap();
        let else_path = companion_path(&flow, &else_action, ScriptRuntime::Python).unwrap();
        let slash_id = companion_path(&flow, &["a/b".to_string()], ScriptRuntime::Python).unwrap();
        let escaped_id =
            companion_path(&flow, &["a%2Fb".to_string()], ScriptRuntime::Python).unwrap();
        let uppercase_id =
            companion_path(&flow, &["Same".to_string()], ScriptRuntime::Python).unwrap();
        let lowercase_id =
            companion_path(&flow, &["same".to_string()], ScriptRuntime::Python).unwrap();

        assert_eq!(
            root_path,
            root.join("sample.flow-scripts").join("same-id.py")
        );
        assert_ne!(then_path, else_path);
        assert_ne!(slash_id, escaped_id);
        assert_ne!(uppercase_id, lowercase_id);
        assert!(then_path.starts_with(root.join("sample.flow-scripts")));
        assert!(else_path.starts_with(root.join("sample.flow-scripts")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saved_companion_code_hydrates_inline_source_on_reopen() {
        let root = temp_dir();
        let flow = root.join("sample.flow.json");
        let mut definition = definition();
        let path = companion_path(
            &flow,
            &[
                "condition".to_string(),
                "then".to_string(),
                "same-id".to_string(),
            ],
            ScriptRuntime::Python,
        )
        .unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "print('edited in Zed')").unwrap();

        // A panel that has just opened the flow has no record of what the
        // companion held before, so the documented rule applies: a saved
        // companion is the latest source.
        let mut fresh = SyncState::default();
        sync_inline_scripts(&mut definition, &flow, &mut fresh).unwrap();

        let serialized = serde_json::to_string(&definition).unwrap();
        let mut reopened: WorkflowDefinition = serde_json::from_str(&serialized).unwrap();
        sync_inline_scripts(&mut reopened, &flow, &mut fresh).unwrap();
        let source: ScriptSource = serde_json::from_value(
            reopened.actions["condition"].actions.as_ref().unwrap()["same-id"].inputs["source"]
                .clone(),
        )
        .unwrap();
        assert_eq!(
            source,
            ScriptSource::Inline {
                runtime: ScriptRuntime::Python,
                code: "print('edited in Zed')".to_string(),
            }
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The content of the companion for the nested `then` script in
    /// [`definition`].
    fn nested_companion(root: &Path) -> PathBuf {
        companion_path(
            &root.join("sample.flow.json"),
            &[
                "condition".to_string(),
                "then".to_string(),
                "same-id".to_string(),
            ],
            ScriptRuntime::Python,
        )
        .unwrap()
    }

    fn inline_code(action: &Action) -> String {
        match serde_json::from_value::<ScriptSource>(action.inputs["source"].clone()).unwrap() {
            ScriptSource::Inline { code, .. } => code,
            source => panic!("expected an inline source, got {source:?}"),
        }
    }

    fn nested_script(definition: &WorkflowDefinition) -> &Action {
        &definition.actions["condition"].actions.as_ref().unwrap()["same-id"]
    }

    fn nested_script_mut(definition: &mut WorkflowDefinition) -> &mut Action {
        definition.actions["condition"]
            .actions
            .as_mut()
            .unwrap()
            .get_mut("same-id")
            .unwrap()
    }

    #[test]
    fn the_first_sync_materializes_a_missing_companion() {
        let root = temp_dir();
        let flow = root.join("sample.flow.json");
        let mut definition = definition();
        let mut state = SyncState::default();

        let report = sync_inline_scripts(&mut definition, &flow, &mut state).unwrap();

        assert!(!report.is_empty());
        assert!(report.iter().all(|(_, outcome)| *outcome == Sync::Created));
        assert_eq!(
            std::fs::read_to_string(nested_companion(&root)).unwrap(),
            "nested original"
        );
        // A second pass has nothing left to do: the file and the flow agree.
        let report = sync_inline_scripts(&mut definition, &flow, &mut state).unwrap();
        assert!(report.iter().all(|(_, outcome)| *outcome == Sync::InSync));
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The regression that motivated the two-way sync: an edit typed into
    /// the inspector used to be dropped by the very next save, because the
    /// companion (written once, at creation) always won.
    #[test]
    fn an_inline_edit_is_written_through_to_an_unchanged_companion() {
        let root = temp_dir();
        let flow = root.join("sample.flow.json");
        let mut definition = definition();
        let mut state = SyncState::default();
        sync_inline_scripts(&mut definition, &flow, &mut state).unwrap();

        // The user edits the inline code in the inspector and saves.
        let code = "print('edited in the panel')";
        nested_script_mut(&mut definition)
            .inputs
            .insert("source".into(), inline_source(code));
        let report = sync_inline_scripts(&mut definition, &flow, &mut state).unwrap();

        assert!(report.contains(&("condition/then/same-id".to_string(), Sync::Written)));
        assert_eq!(
            std::fs::read_to_string(nested_companion(&root)).unwrap(),
            code
        );
        // The flow's own copy is untouched by a write-through.
        assert_eq!(inline_code(nested_script(&definition)), code);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_external_edit_still_hydrates_an_untouched_flow() {
        let root = temp_dir();
        let flow = root.join("sample.flow.json");
        let mut definition = definition();
        let mut state = SyncState::default();
        sync_inline_scripts(&mut definition, &flow, &mut state).unwrap();

        let code = "print('edited in an external editor')";
        std::fs::write(nested_companion(&root), code).unwrap();
        let report = sync_inline_scripts(&mut definition, &flow, &mut state).unwrap();

        assert!(report.contains(&("condition/then/same-id".to_string(), Sync::Hydrated)));
        assert_eq!(inline_code(nested_script(&definition)), code);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn editing_both_sides_is_refused_instead_of_picking_a_winner() {
        let root = temp_dir();
        let flow = root.join("sample.flow.json");
        let mut definition = definition();
        let mut state = SyncState::default();
        sync_inline_scripts(&mut definition, &flow, &mut state).unwrap();

        nested_script_mut(&mut definition)
            .inputs
            .insert("source".into(), inline_source("print('panel')"));
        std::fs::write(nested_companion(&root), "print('external')").unwrap();

        let error = sync_inline_scripts(&mut definition, &flow, &mut state).unwrap_err();

        assert!(
            error.contains("both changed since the last sync"),
            "{error}"
        );
        // Neither side was touched on the way out.
        assert_eq!(
            std::fs::read_to_string(nested_companion(&root)).unwrap(),
            "print('external')"
        );
        assert_eq!(inline_code(nested_script(&definition)), "print('panel')");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_deleted_companion_keeps_the_inline_code() {
        let root = temp_dir();
        let flow = root.join("sample.flow.json");
        let mut definition = definition();
        let mut state = SyncState::default();
        sync_inline_scripts(&mut definition, &flow, &mut state).unwrap();

        // The user reverts to a single-file flow by deleting the companion.
        std::fs::remove_file(nested_companion(&root)).unwrap();
        let report = sync_inline_scripts(&mut definition, &flow, &mut state).unwrap();

        assert!(report.contains(&("condition/then/same-id".to_string(), Sync::Created)));
        assert_eq!(inline_code(nested_script(&definition)), "nested original");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn write_through_creates_then_refreshes_a_companion() {
        let root = temp_dir();
        let flow = root.join("sample.flow.json");
        let mut definition = definition();
        let action_path = vec![
            "condition".to_string(),
            "then".to_string(),
            "same-id".to_string(),
        ];
        let mut state = SyncState::default();

        assert_eq!(
            write_through(&definition, &flow, &action_path, &mut state).unwrap(),
            Sync::Created
        );
        assert_eq!(
            std::fs::read_to_string(nested_companion(&root)).unwrap(),
            "nested original"
        );
        assert_eq!(
            write_through(&definition, &flow, &action_path, &mut state).unwrap(),
            Sync::InSync
        );

        nested_script_mut(&mut definition)
            .inputs
            .insert("source".into(), inline_source("print('panel')"));
        assert_eq!(
            write_through(&definition, &flow, &action_path, &mut state).unwrap(),
            Sync::Written
        );
        assert_eq!(
            std::fs::read_to_string(nested_companion(&root)).unwrap(),
            "print('panel')"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn write_through_refuses_to_clobber_an_external_edit() {
        let root = temp_dir();
        let flow = root.join("sample.flow.json");
        let mut definition = definition();
        let action_path = vec![
            "condition".to_string(),
            "then".to_string(),
            "same-id".to_string(),
        ];
        let mut state = SyncState::default();
        sync_inline_scripts(&mut definition, &flow, &mut state).unwrap();

        nested_script_mut(&mut definition)
            .inputs
            .insert("source".into(), inline_source("print('panel')"));
        std::fs::write(nested_companion(&root), "print('external')").unwrap();

        let error = write_through(&definition, &flow, &action_path, &mut state).unwrap_err();

        assert!(
            error.contains("both changed since the last sync"),
            "{error}"
        );
        assert_eq!(
            std::fs::read_to_string(nested_companion(&root)).unwrap(),
            "print('external')"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_file_source_is_left_completely_alone() {
        let root = temp_dir();
        let flow = root.join("sample.flow.json");
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("scripts").join("task.sh");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(&script, "echo hi\n").unwrap();

        let mut definition = definition();
        definition
            .actions
            .get_mut("same-id")
            .unwrap()
            .inputs
            .insert(
                "source".into(),
                serde_json::to_value(ScriptSource::File {
                    path: "scripts/task.sh".to_string(),
                    runtime: None,
                })
                .unwrap(),
            );
        let mut state = SyncState::default();

        let report = sync_inline_scripts(&mut definition, &flow, &mut state).unwrap();

        // No companion for the File action (the directory itself exists -
        // the definition's other two inline scripts legitimately made one),
        // and the workspace file is neither read nor written.
        assert!(report.iter().all(|(key, _)| key != "same-id"));
        assert!(!root.join("sample.flow-scripts").join("same-id.py").exists());
        assert_eq!(std::fs::read_to_string(&script).unwrap(), "echo hi\n");
        assert_eq!(
            std::fs::read_to_string(nested_companion(&root)).unwrap(),
            "nested original"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolves_companion_address_for_a_nested_canvas_node() {
        let definition = definition();
        let state = FlowState::new(
            vec![
                gpui_flow::FlowNode::new("condition", 0.0, 0.0).node_type("If"),
                gpui_flow::FlowNode::new("condition__branch_then", 0.0, 0.0)
                    .node_type(crate::workflow_json::BRANCH_THEN)
                    .parent("condition"),
                gpui_flow::FlowNode::new("same-id", 0.0, 0.0).parent("condition__branch_then"),
            ],
            Vec::new(),
        );

        let path = action_path_for_node(&definition, &state, "same-id").unwrap();
        assert_eq!(
            path,
            vec![
                "condition".to_string(),
                "then".to_string(),
                "same-id".to_string()
            ]
        );
        assert!(matches!(
            source_for_action_path(&definition, &path),
            Some(ScriptSource::Inline { ref code, .. }) if code == "nested original"
        ));
    }

    #[test]
    fn companion_path_rejects_invalid_flow_name() {
        assert!(
            companion_path(
                Path::new("workflow.json"),
                &["action".to_string()],
                ScriptRuntime::Python
            )
            .is_err()
        );
    }
}
