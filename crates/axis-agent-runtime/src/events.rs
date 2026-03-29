//! Events flowing from providers into the session manager.

use axis_core::agent::{AgentAttention, AgentLifecycle, AgentSessionId};
use axis_core::agent_history::{AgentApprovalRequest, AgentToolCall, AgentTurn};

/// Provider-emitted update applied by [`crate::SessionManager`](crate::SessionManager).
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(
    rename_all = "snake_case",
    tag = "kind",
    rename_all_fields = "snake_case"
)]
pub enum RuntimeEvent {
    Lifecycle {
        session_id: AgentSessionId,
        lifecycle: AgentLifecycle,
    },
    Attention {
        session_id: AgentSessionId,
        attention: AgentAttention,
    },
    Status {
        session_id: AgentSessionId,
        message: String,
    },
    Turn {
        session_id: AgentSessionId,
        turn: AgentTurn,
    },
    ToolCall {
        session_id: AgentSessionId,
        tool_call: AgentToolCall,
    },
    ApprovalRequest {
        session_id: AgentSessionId,
        approval: AgentApprovalRequest,
    },
}
