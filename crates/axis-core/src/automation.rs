//! Shared automation request/response schema for app and CLI over a local control channel.

use crate::agent::AgentSessionId;
use crate::worktree::WorktreeId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Single request envelope: dotted method name plus typed parameters.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all_fields = "snake_case")]
pub enum AutomationRequest {
    #[serde(rename = "worktree.create_or_attach")]
    WorktreeCreateOrAttach {
        repo_root: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attach_path: Option<String>,
    },
    #[serde(rename = "worktree.status")]
    WorktreeStatus {
        worktree_id: WorktreeId,
    },
    #[serde(rename = "agent.start")]
    AgentStart {
        worktree_id: WorktreeId,
        provider_profile_id: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        argv: Vec<String>,
    },
    #[serde(rename = "agent.stop")]
    AgentStop {
        agent_session_id: AgentSessionId,
    },
    #[serde(rename = "agent.list")]
    AgentList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worktree_id: Option<WorktreeId>,
    },
    #[serde(rename = "review.summary")]
    DeskReviewSummary {
        worktree_id: WorktreeId,
    },
    #[serde(rename = "attention.next")]
    AttentionNext {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workdesk_id: Option<String>,
    },
    #[serde(rename = "state.current")]
    StateCurrent {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workdesk_id: Option<String>,
    },
}

/// Minimal success/failure reply; richer results can be added as optional fields later.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AutomationResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl AutomationResponse {
    pub fn success() -> Self {
        Self {
            ok: true,
            result: None,
            error: None,
        }
    }

    pub fn success_with_result(result: Value) -> Self {
        Self {
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            result: None,
            error: Some(message.into()),
        }
    }
}
