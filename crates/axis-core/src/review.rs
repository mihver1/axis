//! Desk-level structured review payloads shared across daemon and app.

use serde::{Deserialize, Serialize};

use crate::worktree::{ReviewSummary, WorktreeId};

/// Full desk review snapshot for a worktree, suitable for automation transport.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DeskReviewPayload {
    pub worktree_id: WorktreeId,
    pub summary: ReviewSummary,
    pub files: Vec<ReviewFileDiff>,
    pub truncated: bool,
}

/// High-level classification of a changed file in a review payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewFileChangeKind {
    Added,
    Deleted,
    Modified,
    Renamed,
}

/// One file entry with line counts and parsed hunks.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ReviewFileDiff {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub change_kind: ReviewFileChangeKind,
    pub added_lines: u32,
    pub removed_lines: u32,
    pub truncated: bool,
    pub hunks: Vec<ReviewHunk>,
}

/// A single unified-diff hunk with optional navigation anchor on the new side.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ReviewHunk {
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_new_line: Option<u32>,
    pub truncated: bool,
    pub lines: Vec<ReviewLine>,
}

/// One rendered line in a hunk (context, removal, or addition).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ReviewLine {
    pub kind: ReviewLineKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_line: Option<u32>,
    pub jumpable: bool,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewLineKind {
    Context,
    Removal,
    Addition,
    Metadata,
}

impl ReviewLine {
    pub fn context(
        old_line: Option<u32>,
        new_line: Option<u32>,
        jumpable: bool,
        text: impl Into<String>,
    ) -> Self {
        Self {
            kind: ReviewLineKind::Context,
            old_line,
            new_line,
            jumpable,
            text: text.into(),
        }
    }

    pub fn removed(
        old_line: Option<u32>,
        new_line: Option<u32>,
        jumpable: bool,
        text: impl Into<String>,
    ) -> Self {
        Self {
            kind: ReviewLineKind::Removal,
            old_line,
            new_line,
            jumpable,
            text: text.into(),
        }
    }

    pub fn added(
        old_line: Option<u32>,
        new_line: Option<u32>,
        jumpable: bool,
        text: impl Into<String>,
    ) -> Self {
        Self {
            kind: ReviewLineKind::Addition,
            old_line,
            new_line,
            jumpable,
            text: text.into(),
        }
    }

    pub fn metadata(text: impl Into<String>) -> Self {
        Self {
            kind: ReviewLineKind::Metadata,
            old_line: None,
            new_line: None,
            jumpable: false,
            text: text.into(),
        }
    }
}
