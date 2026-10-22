//! Read-only GitHub repository Workspace.

mod ref_list;
mod tab;

pub use tab::{
    AuthorizeRepositoryWorkspace, CloneRepositoryForWorkspace, OpenWorkspaceWorkflowRun,
    WorkspaceTab, open_workspace_tab,
};
