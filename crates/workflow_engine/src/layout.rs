//! `<name>.flow.layout.json` sidecar — canvas node positions, kept out of
//! `<name>.flow.json` on purpose (same "generated/presentation data doesn't
//! belong in the authored file" reasoning as `history.rs`'s separate
//! `.forge/history/` files) so a `git diff` of a flow's actual behavior
//! doesn't include noise from someone just dragging a node around.
//!
//! This is the sidecar `schema.rs`'s doc comment on `Action::pos` names —
//! that field was a stopgap: before this module existed, `flow.json` had
//! nowhere to put positions at all, and the observed result was every
//! manual arrangement getting silently discarded on the next save/reopen.
//! Embedding `pos` directly fixed the data-loss bug at the cost of the
//! "flow.json stays pure behavior" goal; this module is what finally lets
//! both be true — `workflow_json.rs`'s `save`/`load` no longer touch
//! `Action::pos` at all, routing every position through here instead.
//!
//! Pure data + file I/O, no `gpui` dependency, same as `history.rs`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkflowLayout {
    /// Keyed by a `/`-joined path from the flow's root down to the action
    /// (e.g. `"retryLoop/step1"` for `step1` nested inside a `Foreach`
    /// named `retryLoop`; `"checkStock/then/notify"` for an `If`'s True
    /// branch) rather than the bare action id — ids are only required to
    /// be unique within their own immediate `actions` map (see
    /// `scheduler.rs`'s validation), so the same id can legally appear at
    /// multiple nesting scopes. `If`/`Try` branches use the literal
    /// segments `then`/`else`/`try`/`catch` to disambiguate, matching
    /// `workflow_json.rs`'s own branch-wrapper naming.
    #[serde(default)]
    pub positions: HashMap<String, (f32, f32)>,
}

/// `<name>.flow.layout.json` next to `<name>.flow.json` — `None` if
/// `flow_json_path` doesn't actually end in `.flow.json` (shouldn't happen
/// for a real workflow tab; see `flows_panel.rs`'s own use of the same
/// suffix to recognize workflow files).
pub fn layout_path_for(flow_json_path: &Path) -> Option<PathBuf> {
    let name = flow_json_path.file_name()?.to_str()?;
    let stem = name.strip_suffix(".flow.json")?;
    Some(flow_json_path.with_file_name(format!("{stem}.flow.layout.json")))
}

/// `None` for a missing or corrupt/foreign sidecar — same "don't error on
/// absence" convention as `history::load_history`; the caller's job is to
/// fall back to migrating inline positions or computing a fresh
/// auto-layout, not to treat this as a hard failure.
pub fn load(flow_json_path: &Path) -> Option<WorkflowLayout> {
    let path = layout_path_for(flow_json_path)?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save(flow_json_path: &Path, layout: &WorkflowLayout) -> Result<(), String> {
    let path = layout_path_for(flow_json_path).ok_or_else(|| {
        format!(
            "'{}' doesn't look like a .flow.json path",
            flow_json_path.display()
        )
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("couldn't create {}: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(layout)
        .map_err(|e| format!("couldn't serialize layout: {e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("couldn't write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_path_replaces_flow_json_suffix() {
        let path = Path::new("/ws/.forge/deploy.flow.json");
        let layout_path = layout_path_for(path).unwrap();
        assert_eq!(layout_path, Path::new("/ws/.forge/deploy.flow.layout.json"));
    }

    #[test]
    fn layout_path_none_for_non_flow_json() {
        assert!(layout_path_for(Path::new("/ws/notes.txt")).is_none());
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let flow_path = dir.path().join("demo.flow.json");
        std::fs::write(&flow_path, "{}").unwrap();

        assert!(load(&flow_path).is_none());

        let mut layout = WorkflowLayout::default();
        layout.positions.insert("step1".to_string(), (10.0, 20.0));
        layout
            .positions
            .insert("retryLoop/step2".to_string(), (30.0, 40.0));
        save(&flow_path, &layout).unwrap();

        let loaded = load(&flow_path).unwrap();
        assert_eq!(loaded.positions.get("step1"), Some(&(10.0, 20.0)));
        assert_eq!(loaded.positions.get("retryLoop/step2"), Some(&(30.0, 40.0)));
    }
}
