//! Worktree binding and desk-level review metadata.

use serde::{Deserialize, Serialize};

/// Opaque worktree handle exchanged over automation and UI state.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorktreeId(pub String);

impl WorktreeId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

/// Execution binding for a workdesk: where code runs and how it relates to upstream.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct WorktreeBinding {
    pub root_path: String,
    pub branch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    #[serde(default)]
    pub ahead: u32,
    #[serde(default)]
    pub behind: u32,
    /// True when the worktree has uncommitted or otherwise “dirty” working tree state.
    #[serde(default)]
    pub dirty: bool,
}

/// Compact “what changed / is this desk ready” summary for the review loop.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", default)]
pub struct ReviewSummary {
    #[serde(default, skip_serializing_if = "is_u32_zero")]
    pub files_changed: u32,
    #[serde(default, skip_serializing_if = "is_u32_zero")]
    pub uncommitted_files: u32,
    #[serde(default, skip_serializing_if = "is_false")]
    pub ready_for_review: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_inspected_at_ms: Option<u64>,
}

fn is_u32_zero(n: &u32) -> bool {
    *n == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}
