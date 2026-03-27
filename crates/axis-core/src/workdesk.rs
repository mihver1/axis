//! Stable workdesk identifiers and daemon-facing workdesk records.

use crate::worktree::WorktreeBinding;
use serde::{Deserialize, Serialize};

/// Stable identifier for a workdesk across GUI restarts.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkdeskId(pub String);

impl WorkdeskId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

/// Shared template identity for daemon-side workdesk records.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkdeskTemplateKind {
    ShellDesk,
    Implementation,
    Debug,
    AgentReview,
}

/// Portable workdesk metadata shared between GUI, CLI, and daemon.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct WorkdeskRecord {
    pub workdesk_id: WorkdeskId,
    pub workspace_root: String,
    pub name: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<WorkdeskTemplateKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_binding: Option<WorktreeBinding>,
}
