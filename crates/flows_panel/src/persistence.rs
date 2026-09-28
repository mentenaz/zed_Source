//! Flow persistence: per-workspace record of which flow the Designer had
//! open, plus per-project run history in the scoped key-value store. Mirrors
//! the `EditorDb`/`KeyValueStore` split the rest of the codebase uses — a
//! SQL domain for the one structured row, the scoped `kv_store` for the JSON
//! run-history blobs (`history:<flow_id>` under the workspace-root
//! namespace, capped at `MAX_HISTORY_ENTRIES`).

use db::kvp::KeyValueStore;
use db::sqlez::domain::Domain;
use db::sqlez_macros::sql;
use db::{AppDatabase, query};
use workflow_engine::history::{RunHistoryEntry, push_entry};
use workspace::{WorkspaceDb, WorkspaceId};

/// Per-workspace record of which `.flow.json` the Designer had open, so a
/// re-opened workspace restores the canvas to the same file.
pub struct DesignerDb(db::sqlez::thread_safe_connection::ThreadSafeConnection);

impl Domain for DesignerDb {
    const NAME: &str = stringify!(DesignerDb);

    //     designer_flows(
    //       workspace_id: i64,  -- FK -> workspaces
    //       flow_path: String,  -- relative path of the open flow (.flow.json)
    //     )
    const MIGRATIONS: &[&str] = &[sql!(
        CREATE TABLE designer_flows(
            workspace_id INTEGER NOT NULL PRIMARY KEY,
            flow_path TEXT NOT NULL,
            FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id)
            ON DELETE CASCADE
            ON UPDATE CASCADE
        ) STRICT;
    )];
}

db::static_connection!(DesignerDb, [WorkspaceDb]);

impl DesignerDb {
    // The flow path the Designer had open in `workspace_id`, if any.
    query! {
        pub fn flow_path(workspace_id: WorkspaceId) -> Result<Option<String>> {
            SELECT flow_path FROM designer_flows WHERE workspace_id = (?)
        }
    }

    query! {
        pub async fn save_flow_path(workspace_id: WorkspaceId, flow_path: String) -> Result<()> {
            INSERT OR REPLACE INTO designer_flows(workspace_id, flow_path) VALUES (?, ?)
        }
    }

    query! {
        pub async fn delete_by_workspace(workspace_id: WorkspaceId) -> Result<()> {
            DELETE FROM designer_flows WHERE workspace_id = (?)
        }
    }
}

/// The scoped-kv key under which `flow_id`'s run history is stored.
pub fn history_key(flow_id: &str) -> String {
    format!("history:{flow_id}")
}

/// Reads the retained run history for `flow_id`, oldest-first. Empty (not
/// an error) for a flow that never ran, a flow whose id changed, or a
/// corrupt stored blob — history is diagnostic, not load-bearing.
pub fn read_flow_history(
    store: &KeyValueStore,
    namespace: &str,
    flow_id: &str,
) -> anyhow::Result<Vec<RunHistoryEntry>> {
    let Some(raw) = store.scoped(namespace).read(&history_key(flow_id))? else {
        return Ok(Vec::new());
    };
    Ok(serde_json::from_str(&raw).unwrap_or_default())
}

/// Appends `entry` to `flow_id`'s retained history (capped at
/// [`MAX_HISTORY_ENTRIES`], oldest dropped first) and writes the list back.
pub async fn append_flow_history(
    store: &KeyValueStore,
    namespace: &str,
    flow_id: &str,
    entry: RunHistoryEntry,
) -> anyhow::Result<()> {
    let mut entries = read_flow_history(store, namespace, flow_id)?;
    push_entry(&mut entries, entry);
    let json = serde_json::to_string(&entries)?;
    store
        .scoped(namespace)
        .write(history_key(flow_id), json)
        .await
}

/// Fires-and-forgets an [`append_flow_history`] on the background thread
/// pool — the run-finish path calls this once a run completes, and doesn't
/// need to wait for the write.
pub fn write_flow_history(
    db: &AppDatabase,
    cx: &mut db::gpui::App,
    namespace: String,
    flow_id: String,
    entry: RunHistoryEntry,
) {
    let store = KeyValueStore::from_app_db(db);
    db::write_and_log(cx, move || async move {
        let mut entries = read_flow_history(&store, &namespace, &flow_id)?;
        push_entry(&mut entries, entry);
        let json = serde_json::to_string(&entries)?;
        store
            .scoped(&namespace)
            .write(history_key(&flow_id), json)
            .await
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use workflow_engine::schema::RunOutcome;

    fn entry(id: &str, outcome: RunOutcome) -> RunHistoryEntry {
        RunHistoryEntry {
            id: id.to_string(),
            time: "2026-09-11T00:00:00Z".to_string(),
            outcome,
            actions: Vec::new(),
        }
    }

    // Opens the full app database (AppMigrator runs every inventory-registered
    // domain in dependency order), then wraps its connection as a DesignerDb —
    // the same shape DesignerDb::global produces on the per-App database.
    async fn open_designer_db() -> (DesignerDb, AppDatabase) {
        let app_db = AppDatabase::test_new();
        (DesignerDb(app_db.0.clone()), app_db)
    }

    async fn seed_workspace(db: &DesignerDb, workspace_id: WorkspaceId) {
        db.write(move |connection| {
            connection.exec_bound::<i64>("INSERT INTO workspaces(workspace_id) VALUES (?)")?(
                i64::from(workspace_id),
            )
            .unwrap();
            anyhow::Ok::<()>(())
        })
        .await
        .unwrap();
    }

    #[gpui::test]
    async fn designer_db_round_trips_the_open_flow_path() {
        let (db, _) = open_designer_db().await;
        let workspace_id = WorkspaceId::from_i64(1);
        seed_workspace(&db, workspace_id).await;

        assert_eq!(db.flow_path(workspace_id).unwrap(), None);

        db.save_flow_path(
            workspace_id,
            ".zed/workflows/order-processing.flow.json".to_string(),
        )
        .await
        .unwrap();
        assert_eq!(
            db.flow_path(workspace_id).unwrap().as_deref(),
            Some(".zed/workflows/order-processing.flow.json")
        );

        // Saving again replaces (INSERT OR REPLACE).
        db.save_flow_path(
            workspace_id,
            ".zed/workflows/shipping.flow.json".to_string(),
        )
        .await
        .unwrap();
        assert_eq!(
            db.flow_path(workspace_id).unwrap().as_deref(),
            Some(".zed/workflows/shipping.flow.json")
        );

        db.delete_by_workspace(workspace_id).await.unwrap();
        assert_eq!(db.flow_path(workspace_id).unwrap(), None);
    }

    #[gpui::test]
    async fn history_appends_and_stays_capped_at_max_entries() {
        let (db, app_db) = open_designer_db().await;
        let store = KeyValueStore::from_app_db(&app_db);
        let namespace = "E:/project-root";
        let flow_id = "order-processing";
        assert!(db.flow_path(WorkspaceId::from_i64(1)).unwrap().is_none());

        assert!(
            read_flow_history(&store, namespace, flow_id)
                .unwrap()
                .is_empty()
        );

        for i in 0..(workflow_engine::history::MAX_HISTORY_ENTRIES + 5) {
            append_flow_history(
                &store,
                namespace,
                flow_id,
                entry(&format!("run{i}"), RunOutcome::Succeeded),
            )
            .await
            .unwrap();
        }

        let entries = read_flow_history(&store, namespace, flow_id).unwrap();
        assert_eq!(entries.len(), workflow_engine::history::MAX_HISTORY_ENTRIES);
        // The oldest 5 were dropped — the first entry remaining is run5.
        assert_eq!(entries[0].id, "run5");
    }

    #[gpui::test]
    async fn history_namespaces_do_not_collide() {
        let (_, app_db) = open_designer_db().await;
        let store = KeyValueStore::from_app_db(&app_db);

        append_flow_history(
            &store,
            "E:/project-a",
            "order-processing",
            entry("run1", RunOutcome::Succeeded),
        )
        .await
        .unwrap();

        assert_eq!(
            read_flow_history(&store, "E:/project-a", "order-processing")
                .unwrap()
                .len(),
            1
        );
        assert!(
            read_flow_history(&store, "E:/project-b", "order-processing")
                .unwrap()
                .is_empty()
        );
    }
}
