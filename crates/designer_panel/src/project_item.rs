//! `FlowFile` — the minimal `project::ProjectItem` that makes a `.flow.json`
//! path resolve to [`super::DesignerPanel`] instead of the default text
//! editor when opened through the normal project-open paths (project panel
//! double-click, quick-open, `Workspace::open_path`, ...).
//!
//! Deliberately thin: it carries just enough identity (`project_path`,
//! `entry_id`) for `workspace::ProjectItem`'s bookkeeping — `DesignerPanel`
//! already reads/writes the file directly via `std::fs`, so this doesn't
//! wrap a `language::Buffer` or do any content loading of its own.
//!
//! `ProjectItemRegistry` (`workspace.rs`) tries every registered project
//! item type **most-recently-registered first** (see its own doc comment:
//! "starting from the project item that was added last"), so registering
//! this from `designer_panel::init` doesn't require touching the `project`
//! crate's own file-open resolution at all — `try_open` returning `None`
//! for anything not ending in `.flow.json` is what keeps every other file
//! type routing to its usual handler.

use std::path::PathBuf;

use gpui::{App, AppContext as _, Entity, Task};
use project::{Project, ProjectEntryId, ProjectPath, WorktreeId};

pub struct FlowFile {
    project_path: ProjectPath,
    abs_path: Option<PathBuf>,
    entry_id: Option<ProjectEntryId>,
}

impl FlowFile {
    pub fn abs_path(&self) -> Option<PathBuf> {
        self.abs_path.clone()
    }

    pub fn worktree_id(&self) -> WorktreeId {
        self.project_path.worktree_id
    }

    /// Whether `path` names a flow file, i.e. ends in `.flow.json`
    /// (case-insensitive — matched by suffix, not `Path::extension()`, which
    /// only splits on the last `.`). Same convention `flows_panel` /
    /// `workflow_engine` use; excluding `.flow.layout.json` (the layout
    /// sidecar) is what lets it keep opening as plain JSON text instead of a
    /// canvas.
    pub fn matches(path: &std::path::Path) -> bool {
        path.to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".flow.json")
    }
}

impl project::ProjectItem for FlowFile {
    fn try_open(
        project: &Entity<Project>,
        path: &ProjectPath,
        cx: &mut App,
    ) -> Option<Task<anyhow::Result<Entity<Self>>>> {
        if !Self::matches(&path.path.as_std_path()) {
            return None;
        }

        let project_ref = project.read(cx);
        let abs_path = project_ref.absolute_path(path, cx);

        if project_ref.is_local() {
            // Local project: require the file to actually exist on disk.
            // Deliberately *not* gated on the worktree having indexed an
            // `Entry` for it — a `.flow.json` that is gitignored, brand-new,
            // or not yet scanned would otherwise have `entry_for_path`
            // return `None` and silently fall through to the text editor.
            let Some(abs_path) = abs_path.as_ref() else {
                return None;
            };
            if !abs_path.is_file() {
                return None;
            }
        } else {
            // Remote/collab project: there's no local filesystem to stat,
            // so fall back to the worktree's indexed entry.
            let entry = project_ref.entry_for_path(path, cx)?;
            if !entry.is_file() {
                return None;
            }
        }

        let entry_id = project_ref.entry_for_path(path, cx).map(|entry| entry.id);
        let project_path = path.clone();

        Some(Task::ready(Ok(cx.new(|_| FlowFile {
            project_path,
            abs_path,
            entry_id,
        }))))
    }

    fn entry_id(&self, _cx: &App) -> Option<ProjectEntryId> {
        self.entry_id
    }

    fn project_path(&self, _cx: &App) -> Option<ProjectPath> {
        Some(self.project_path.clone())
    }

    fn is_dirty(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::FlowFile;
    use std::path::Path;

    #[test]
    fn matches_only_flow_json_files() {
        assert!(FlowFile::matches(Path::new("workflow.flow.json")));
        assert!(FlowFile::matches(Path::new("nested/dir/Work.Flow.JSON")));
        assert!(FlowFile::matches(Path::new("FLOW.JSON")));

        // The layout sidecar and every other JSON stay with the text editor.
        assert!(!FlowFile::matches(Path::new("workflow.flow.layout.json")));
        assert!(!FlowFile::matches(Path::new("workflow.json")));
        assert!(!FlowFile::matches(Path::new("flow.json")));
        assert!(!FlowFile::matches(Path::new("aflow.json")));
        assert!(!FlowFile::matches(Path::new("workflow.flow.json.bak")));
    }
}
