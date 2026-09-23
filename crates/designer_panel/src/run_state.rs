//! Shared, thread-safe run state for a designer flow run.
//!
//! `workflow_engine::StatusSink` is a bare `Arc<dyn Fn + Send + Sync>`
//! callback invoked from whatever tokio/gpui thread drives the run, so the
//! per-action status events can't touch the UI thread directly. Both the
//! Designer tab and the "Run Results" tab instead share one
//! [`Arc<Mutex<RunState>>`] — the sink fills it, and a UI-side poll task
//! (see `DesignerPanel::start_run_poll` / `RunResults::start_poll`)
//! snapshots it onto the panel a few times a second. That keeps the engine
//! gpui-free (no change to `workflow_engine`) and the UI reactive enough
//! for a live "which action is running" indicator.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use workflow_engine::RunOutcome;

/// One action's live phase, mirroring `workflow_engine::RunStatus` but
/// owned by this crate so the UI can style it without pulling the engine's
/// enum through `serde`/rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunPhase {
    Running,
    Succeeded,
    Failed,
    Skipped,
}

impl RunPhase {
    pub fn label(self) -> &'static str {
        match self {
            RunPhase::Running => "Running",
            RunPhase::Succeeded => "Succeeded",
            RunPhase::Failed => "Failed",
            RunPhase::Skipped => "Skipped",
        }
    }
}

/// Everything the UI knows about a single action's status during/after a
/// run. Keyed by the action's id — note a `Foreach` body action can be
/// reported `Running`/`Succeeded` multiple times; the map keeps only the
/// latest entry while [`RunLogLine`] keeps every transition's history.
#[derive(Debug, Clone)]
pub struct ActionRun {
    pub name: String,
    pub type_id: String,
    pub phase: RunPhase,
    /// For `Succeeded`: the serialized `outputs` map (empty when the
    /// executor returned no outputs). For `Failed`: the error message.
    /// `None` for `Running`/`Skipped`.
    pub detail: Option<String>,
    pub started_at: Option<Instant>,
    pub finished_at: Option<Instant>,
}

impl ActionRun {
    pub fn elapsed(&self) -> Option<Duration> {
        match (self.started_at, self.finished_at) {
            (Some(start), Some(finish)) => Some(finish.saturating_duration_since(start)),
            (Some(start), None) => Some(start.elapsed()),
            _ => None,
        }
    }
}

/// One entry in the chronological run transcript.
#[derive(Debug, Clone)]
pub struct RunLogLine {
    /// Milliseconds since the run started.
    pub time_ms: u128,
    pub action_id: String,
    pub name: String,
    pub type_id: String,
    pub phase: RunPhase,
    pub detail: Option<String>,
}

/// The full, lock-held state of one run. Written by the `StatusSink`
/// closure (possibly off the UI thread), read by UI poll tasks.
#[derive(Debug, Clone, Default)]
pub struct RunState {
    pub nodes: HashMap<String, ActionRun>,
    pub log: Vec<RunLogLine>,
    pub started_at: Option<Instant>,
    pub finished_at: Option<Instant>,
    pub outcome: Option<RunOutcome>,
    pub error: Option<String>,
    /// Bumped by every mutation so UI poll tasks can cheaply detect "new
    /// events since my last snapshot" without `PartialEq` on `Instant`s.
    pub revision: u64,
}

impl RunState {
    pub fn record(
        &mut self,
        run_start: Instant,
        action_id: &str,
        phase: RunPhase,
        detail: Option<String>,
        name: String,
        type_id: String,
    ) {
        let now = Instant::now();
        let entry = self.nodes.entry(action_id.to_string()).or_insert_with(|| ActionRun {
            name: name.clone(),
            type_id: type_id.clone(),
            phase,
            detail: None,
            started_at: None,
            finished_at: None,
        });
        entry.name = name.clone();
        entry.type_id = type_id.clone();
        entry.phase = phase;
        entry.detail = detail.clone();
        match phase {
            RunPhase::Running => entry.started_at = Some(now),
            RunPhase::Succeeded | RunPhase::Failed | RunPhase::Skipped => {
                entry.finished_at = Some(now);
            }
        }
        self.log.push(RunLogLine {
            time_ms: now.duration_since(run_start).as_millis(),
            action_id: action_id.to_string(),
            name,
            type_id,
            phase,
            detail,
        });
        self.revision += 1;
    }

    /// `(succeeded, failed, skipped)` counts for the summary header.
    pub fn totals(&self) -> (u32, u32, u32) {
        let mut succeeded = 0;
        let mut failed = 0;
        let mut skipped = 0;
        for run in self.nodes.values() {
            match run.phase {
                RunPhase::Succeeded => succeeded += 1,
                RunPhase::Failed => failed += 1,
                RunPhase::Skipped => skipped += 1,
                RunPhase::Running => {}
            }
        }
        (succeeded, failed, skipped)
    }

    pub fn duration(&self) -> Option<Duration> {
        match (self.started_at, self.finished_at) {
            (Some(start), Some(finish)) => Some(finish.saturating_duration_since(start)),
            (Some(start), None) => Some(start.elapsed()),
            _ => None,
        }
    }
}

pub type SharedRunState = Arc<Mutex<RunState>>;

/// Path-keyed registry of the *live* run state, so a run-results tab can
/// find the run it belongs to without the designer handing it a handle. The
/// Designer registers its `SharedRunState` on open and drops it on close;
/// `run_state_for` falls back to a fresh empty state (a "no run yet" tab).
static RUN_STATES: OnceLock<Arc<Mutex<HashMap<PathBuf, SharedRunState>>>> = OnceLock::new();

pub fn register_run_state(path: PathBuf, state: SharedRunState) {
    RUN_STATES
        .get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
        .lock()
        .unwrap()
        .insert(path, state);
}

pub fn unregister_run_state(path: &Path) {
    if let Some(states) = RUN_STATES.get() {
        states.lock().unwrap().remove(path);
    }
}

pub fn run_state_for(path: &Path) -> SharedRunState {
    RUN_STATES
        .get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
        .lock()
        .unwrap()
        .get(path)
        .cloned()
        .unwrap_or_else(|| Arc::new(Mutex::new(RunState::default())))
}