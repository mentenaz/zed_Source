use anyhow::Result;
use db::{
    query,
    sqlez::{domain::Domain, statement::Statement, thread_safe_connection::ThreadSafeConnection},
    sqlez_macros::sql,
};
use workspace::{ItemId, WorkspaceDb, WorkspaceId};

pub struct DesignerDb(ThreadSafeConnection);

impl Domain for DesignerDb {
    const NAME: &str = stringify!(DesignerDb);

    // Migration steps are immutable once applied — Zed checks the stored SQL
    // text of every already-run step against what's in this array on
    // startup, and refuses to open the database (falling back to an
    // in-memory one, breaking every panel's persisted state for that
    // session) if step 0's text no longer matches what an earlier build
    // already applied. Step 0 below is that earlier shape (workspace_id-only
    // PK, single flow per workspace) — never edit it again. Step 1 replaces
    // it with the shape the panel actually needs (one row per open item, not
    // per workspace). The table had no meaningful data yet, so a drop +
    // recreate is simpler and safe here; a real schema change with data to
    // preserve should copy-transform like `workspace_id`/`workspaces_2` in
    // `workspace::persistence` instead.
    const MIGRATIONS: &[&str] = &[
        sql!(
            CREATE TABLE designer_flows (
                workspace_id INTEGER NOT NULL PRIMARY KEY,
                flow_path TEXT NOT NULL,
                FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id)
                    ON DELETE CASCADE ON UPDATE CASCADE
            ) STRICT;
        ),
        sql!(
            DROP TABLE designer_flows;
            CREATE TABLE designer_flows (
                workspace_id INTEGER,
                item_id INTEGER,
                path TEXT NOT NULL,
                PRIMARY KEY(workspace_id, item_id),
                FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id)
                    ON DELETE CASCADE
            ) STRICT;
        ),
    ];
}

db::static_connection!(DesignerDb, [WorkspaceDb]);

impl DesignerDb {
    pub async fn save_flow(
        &self,
        item_id: ItemId,
        workspace_id: WorkspaceId,
        path: String,
    ) -> Result<()> {
        self.write(move |conn| {
            let mut statement = Statement::prepare(
                conn,
                "INSERT INTO designer_flows(workspace_id, item_id, path)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(workspace_id, item_id) DO UPDATE SET path = excluded.path",
            )?;
            let mut next = statement.bind(&workspace_id, 1)?;
            next = statement.bind(&item_id, next)?;
            statement.bind(&path, next)?;
            statement.exec()
        })
        .await
    }

    query! {
        pub fn flow_path(item_id: ItemId, workspace_id: WorkspaceId) -> Result<Option<String>> {
            SELECT path
            FROM designer_flows
            WHERE item_id = ? AND workspace_id = ?
        }
    }

    pub async fn delete_unloaded(
        &self,
        workspace_id: WorkspaceId,
        alive_items: Vec<ItemId>,
    ) -> Result<()> {
        self.write(move |conn| {
            if alive_items.is_empty() {
                let mut statement =
                    Statement::prepare(conn, "DELETE FROM designer_flows WHERE workspace_id = ?1")?;
                statement.bind(&workspace_id, 1)?;
                return statement.exec();
            }

            let placeholders = (0..alive_items.len())
                .map(|index| format!("?{}", index + 2))
                .collect::<Vec<_>>()
                .join(", ");
            let query = format!(
                "DELETE FROM designer_flows
                 WHERE workspace_id = ?1 AND item_id NOT IN ({placeholders})"
            );
            let mut statement = Statement::prepare(conn, &query)?;
            let mut next = statement.bind(&workspace_id, 1)?;
            for item_id in &alive_items {
                next = statement.bind(item_id, next)?;
            }
            statement.exec()
        })
        .await
    }
}
