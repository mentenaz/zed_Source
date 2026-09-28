//! §6 of `DOCS/workflow-schema/execution-engine-design.md` — per-action
//! status transitions, threaded through `execute_action` and every
//! container executor so a UI layer can render progress live. Deliberately
//! just a callback (`StatusSink`), not a broadcast channel keyed by
//! `(run_id, action_id)` — this module stays gpui-free (see the parent
//! module's own doc: "pure data/logic, no gpui dependency"); the caller
//! (`ForgeShell`/`DesignerPanel`) decides how to fan updates out to the
//! canvas, and for a single Designer tab running its own flow, a direct
//! callback into an mpsc channel is simpler than AppState-level broadcast
//! infrastructure with no second subscriber to justify it yet.

use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    Succeeded,
    Failed,
    Skipped,
}

/// Cheap to `Clone` (an `Arc` underneath) so every recursive
/// `execute_action`/container-executor call can carry its own copy without
/// threading a reference through every lifetime in `container.rs`.
#[derive(Clone)]
pub struct StatusSink(Option<Arc<dyn Fn(&str, RunStatus, Option<String>) + Send + Sync>>);

impl StatusSink {
    /// For callers that don't care about status — every existing engine
    /// test uses this, so adding live status didn't require touching any
    /// of them beyond passing this in.
    pub fn none() -> Self {
        Self(None)
    }

    pub fn new(f: impl Fn(&str, RunStatus, Option<String>) + Send + Sync + 'static) -> Self {
        Self(Some(Arc::new(f)))
    }

    /// `detail` is the human-readable "what actually happened" a UI log
    /// wants — the serialized `outputs` map for a `Succeeded` leaf, the
    /// error message for `Failed`, `None` for `Running`/`Skipped` (nothing
    /// to show yet).
    pub fn report(&self, action_id: &str, status: RunStatus, detail: Option<String>) {
        if let Some(f) = &self.0 {
            f(action_id, status, detail);
        }
    }
}
