//! Structured agent timeline/detail records shared across runtime, UI, and automation.

use serde::{Deserialize, Serialize};

use crate::agent::AgentSessionRecord;

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentTurnId(pub String);

impl AgentTurnId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentToolCallId(pub String);

impl AgentToolCallId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentApprovalRequestId(pub String);

impl AgentApprovalRequestId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AgentSessionCapabilities {
    #[serde(default)]
    pub turn_input: bool,
    #[serde(default)]
    pub tool_calls: bool,
    #[serde(default)]
    pub approvals: bool,
    #[serde(default)]
    pub resume: bool,
    #[serde(default)]
    pub terminal_attachment: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTurnRole {
    User,
    Assistant,
    System,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTurnState {
    Pending,
    Streaming,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AgentTurn {
    pub id: AgentTurnId,
    pub role: AgentTurnRole,
    pub state: AgentTurnState,
    pub text: String,
    pub created_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentToolCallState {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AgentToolCall {
    pub id: AgentToolCallId,
    pub title: String,
    pub state: AgentToolCallState,
    pub details: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentApprovalKind {
    ToolCall,
    Command,
    Patch,
    Generic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentApprovalState {
    Pending,
    Approved,
    Denied,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AgentApprovalDecision {
    pub approved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub decided_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AgentApprovalRequest {
    pub id: AgentApprovalRequestId,
    pub kind: AgentApprovalKind,
    pub title: String,
    pub details: String,
    pub state: AgentApprovalState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<AgentToolCallId>,
    pub requested_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<AgentApprovalDecision>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", rename_all_fields = "snake_case")]
pub enum AgentTimelineEntry {
    Turn { sequence: u64, turn: AgentTurn },
    ToolCall {
        sequence: u64,
        tool_call: AgentToolCall,
    },
    ApprovalRequest {
        sequence: u64,
        approval: AgentApprovalRequest,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AgentSessionDetail {
    pub session: AgentSessionRecord,
    #[serde(default)]
    pub capabilities: AgentSessionCapabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at_ms: Option<u64>,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub history_cursor: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_approval_id: Option<AgentApprovalRequestId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timeline: Vec<AgentTimelineEntry>,
    #[serde(default)]
    pub truncated: bool,
}
