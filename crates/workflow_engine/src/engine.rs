//! `RunContext` — §2 of `DOCS/workflow-schema/execution-engine-design.md`.
//! The JSONLogic evaluation context threaded through the scheduler (§3,
//! `scheduler.rs`, fully built): a flat `{actionId: {"outputs": {...}}}` map
//! built up incrementally as actions complete, plus (§1) a reserved `$item`
//! slot bound only while a `Foreach` iteration's body is running.
//!
//! Deliberately doesn't know about `Action`/`ActionMap`/the scheduler at
//! all — same "data, not code" separation the registry already follows.
//! This is pure JSONLogic plumbing; scheduling and per-action-type
//! execution are separate, later pieces of the same design doc.

use std::sync::{Arc, RwLock};

use datalogic_rs::Engine;
use serde_json::{Map, Value};

/// Reserved data-context key for a `Foreach` iteration's loop variable —
/// see the design doc §1's "Locked" note. Not a valid action id (action ids
/// must start with a letter per the schema's `actionId` pattern), so this
/// can never collide with a real action's own entry.
const ITEM_KEY: &str = "$item";

/// One per flow execution, cheaply `Clone`d to produce nested-scope
/// variants (§2) — the `Arc<RwLock<_>>` data layer is what's actually
/// shared or forked, not this struct itself.
#[derive(Clone)]
pub struct RunContext {
    /// Action id -> `{"outputs": {...}}`, written once per action as it
    /// completes and read by every JSONLogic evaluation from this point
    /// on. `Arc<RwLock<_>>` so concurrently-running sibling actions (§3's
    /// scheduler runs everything runAfter-eligible at once) can all read
    /// it while only the action that just finished writes to it.
    data: Arc<RwLock<Map<String, Value>>>,
    /// This context's own `$item` binding, if any — see `with_item`.
    /// Deliberately *not* stored in `data` itself: `data` is shared with
    /// `child()`-derived contexts of the *previous* iteration, and if
    /// `$item` lived there, clearing/overwriting it for iteration N+1
    /// would need extra bookkeeping to undo. Keeping it as a plain field
    /// on `RunContext` (cheap to `Clone`) sidesteps that entirely.
    item: Option<Value>,
    engine: Arc<Engine>,
}

impl RunContext {
    pub fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(Map::new())),
            item: None,
            engine: Arc::new(Engine::new()),
        }
    }

    /// A snapshot for one JSONLogic evaluation — a clone, not a live view,
    /// so an expression sees a consistent picture even if another action
    /// finishes mid-evaluation (design doc §2).
    pub fn snapshot(&self) -> Value {
        let mut map = self.data.read().unwrap().clone();
        if let Some(item) = &self.item {
            map.insert(ITEM_KEY.to_string(), item.clone());
        }
        Value::Object(map)
    }

    /// Records a just-finished action's outputs, visible to every
    /// evaluation from this point on (including ones already in flight
    /// that haven't yet called `snapshot`, since sibling actions read
    /// through the same `Arc<RwLock<_>>`).
    pub fn record_outputs(&self, action_id: &str, outputs: Map<String, Value>) {
        let mut entry = Map::new();
        entry.insert("outputs".to_string(), Value::Object(outputs));
        self.data
            .write()
            .unwrap()
            .insert(action_id.to_string(), Value::Object(entry));
    }

    /// Evaluate one JSONLogic expression (`foreach`/`until`/`expression`,
    /// or any `inputs` field of `registry::FieldKind::Expression`) against
    /// the current snapshot.
    ///
    /// **Discovered empirically, not documented anywhere upstream**: this
    /// `datalogic-rs` build parses *any* JSON object appearing anywhere in
    /// the rule tree as an operator call — including one meant as a
    /// literal data value, and even wrapped in the nominal `"val"` escape
    /// operator. `{"sku": "ITEM-1"}` embedded as data (e.g. inside an
    /// `"or"` fallback array) fails with `"Invalid operator: sku"`, not a
    /// parse error naming the real problem. Numbers, strings, bools, and
    /// arrays of those are fine as literals; a bare JSON object never is.
    /// Build literal structured data by composing an object from `outputs`
    /// that already exist as real actions' results, not by writing an
    /// object literal directly into an expression.
    pub fn evaluate(&self, expr: &Value) -> Result<Value, String> {
        let data = self.snapshot();
        self.engine
            .eval_into::<Value, _, _>(expr, &data)
            .map_err(|e| e.to_string())
    }

    /// §2: a `Foreach` body's per-iteration child layer. Starts as a copy
    /// of `self`'s current data (so the body can read everything that's
    /// resolved so far, including ancestors' siblings), but writes made
    /// through the child (i.e. actions inside the loop body) land only in
    /// the child's own layer — never propagating back to `self` or to a
    /// sibling iteration's own `child()`. Without this, re-running the same
    /// body every iteration would let iteration 2's actions silently
    /// overwrite iteration 1's outputs under the same ids in one shared
    /// map.
    pub fn child(&self) -> Self {
        let snapshot = self.data.read().unwrap().clone();
        Self {
            data: Arc::new(RwLock::new(snapshot)),
            item: None,
            engine: self.engine.clone(),
        }
    }

    /// §1: bind `$item` for one `Foreach` iteration's body. Shares the same
    /// data layer as `self` (only the reserved `$item` slot differs), so
    /// this is cheap to call once per iteration — pair it with `child()`
    /// once per `Foreach` (not once per iteration) and call `with_item` on
    /// that child for each item in the evaluated `foreach` array.
    pub fn with_item(&self, item: Value) -> Self {
        Self {
            item: Some(item),
            ..self.clone()
        }
    }
}

impl Default for RunContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_reads_back_outputs_by_action_id() {
        let ctx = RunContext::new();
        ctx.record_outputs(
            "fetchOrder",
            serde_json::json!({"total": 42})
                .as_object()
                .unwrap()
                .clone(),
        );

        let result = ctx
            .evaluate(&serde_json::json!({"var": "fetchOrder.outputs.total"}))
            .unwrap();
        assert_eq!(result, Value::from(42));
    }

    #[test]
    fn missing_var_evaluates_to_null_not_an_error() {
        let ctx = RunContext::new();
        let result = ctx
            .evaluate(&serde_json::json!({"var": "neverRan.outputs.x"}))
            .unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn item_is_only_visible_when_bound() {
        let ctx = RunContext::new();
        // Not bound yet — $item resolves to null, not an error.
        let before = ctx
            .evaluate(&serde_json::json!({"var": "$item.sku"}))
            .unwrap();
        assert_eq!(before, Value::Null);

        let bound = ctx.with_item(serde_json::json!({"sku": "ABC"}));
        let during = bound
            .evaluate(&serde_json::json!({"var": "$item.sku"}))
            .unwrap();
        assert_eq!(during, Value::from("ABC"));

        // The original context (sibling scope) was never mutated.
        let after = ctx
            .evaluate(&serde_json::json!({"var": "$item.sku"}))
            .unwrap();
        assert_eq!(after, Value::Null);
    }

    #[test]
    fn child_sees_parent_data_but_writes_stay_local() {
        let parent = RunContext::new();
        parent.record_outputs(
            "setup",
            serde_json::json!({"ready": true})
                .as_object()
                .unwrap()
                .clone(),
        );

        let child = parent.child();
        // Child can read what the parent already resolved before the fork.
        let seen = child
            .evaluate(&serde_json::json!({"var": "setup.outputs.ready"}))
            .unwrap();
        assert_eq!(seen, Value::from(true));

        // Child writes (an action inside the Foreach body finishing) don't
        // leak back to the parent or to a sibling iteration's own child.
        child.record_outputs(
            "processItem",
            serde_json::json!({"result": "ok"})
                .as_object()
                .unwrap()
                .clone(),
        );
        let sibling = parent.child();
        let leaked = sibling
            .evaluate(&serde_json::json!({"var": "processItem.outputs.result"}))
            .unwrap();
        assert_eq!(leaked, Value::Null);

        let parent_leaked = parent
            .evaluate(&serde_json::json!({"var": "processItem.outputs.result"}))
            .unwrap();
        assert_eq!(parent_leaked, Value::Null);
    }

    #[test]
    fn nested_containers_share_the_parent_context_directly() {
        // §2: `If`/`Until`/`Try` don't fork — they evaluate straight
        // against the same `RunContext` their caller holds (no `child()`
        // call in their executors, once those are written), so this test
        // just documents that `RunContext` itself imposes no such
        // isolation unless `child()` is explicitly used — the scheduler
        // is what decides per container type.
        let ctx = RunContext::new();
        ctx.record_outputs(
            "a",
            serde_json::json!({"x": 1}).as_object().unwrap().clone(),
        );
        let same_scope = ctx.clone();
        same_scope.record_outputs(
            "b",
            serde_json::json!({"y": 2}).as_object().unwrap().clone(),
        );

        let seen = ctx
            .evaluate(&serde_json::json!({"var": "b.outputs.y"}))
            .unwrap();
        assert_eq!(seen, Value::from(2));
    }
}
