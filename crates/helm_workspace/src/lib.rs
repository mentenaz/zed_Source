//! Read-only GitHub repository Workspace.

mod tab;

pub use tab::{
    AuthorizeRepositoryWorkspace, CloneRepositoryForWorkspace, OpenWorkspaceWorkflowRun,
    WorkspaceTab, open_workspace_tab,
};
