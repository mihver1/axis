//! Shared line protocol for structured CLI-backed providers.

use anyhow::Context;
use axis_core::agent::{AgentAttention, AgentLifecycle, AgentSessionId};
use axis_core::agent_history::{
    AgentApprovalRequest, AgentApprovalRequestId, AgentToolCall, AgentTurn,
};

use crate::events::RuntimeEvent;

const AXIS_EVENT_PREFIX: &str = "AXIS_EVENT ";
const AXIS_CMD_PREFIX: &str = "AXIS_CMD ";
const AXIS_ATTENTION_PREFIX: &str = "AXIS_ATTENTION ";
const AXIS_STATUS_PREFIX: &str = "AXIS_STATUS ";
pub const AXIS_APPROVAL_REQUEST_PREFIX: &str = "AXIS_APPROVAL_REQUEST ";

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "snake_case"
)]
enum AxisCliEvent {
    Lifecycle { lifecycle: AgentLifecycle },
    Attention { attention: AgentAttention },
    Status { message: String },
    Turn { turn: AgentTurn },
    ToolCall { tool_call: AgentToolCall },
    ApprovalRequest { approval: AgentApprovalRequest },
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "snake_case"
)]
pub enum AxisCliCommand {
    SendTurn {
        text: String,
    },
    RespondApproval {
        approval_request_id: AgentApprovalRequestId,
        approved: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    Resume,
}

pub fn parse_axis_output_line(
    line: &str,
    session_id: &AgentSessionId,
) -> Option<Vec<RuntimeEvent>> {
    let trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix(AXIS_EVENT_PREFIX) {
        let event = serde_json::from_str::<AxisCliEvent>(rest).ok()?;
        return Some(axis_cli_event_to_runtime_events(session_id, event));
    }
    if let Some(rest) = trimmed.strip_prefix(AXIS_ATTENTION_PREFIX) {
        let attention = match rest.trim() {
            "needs_review" => AgentAttention::NeedsReview,
            "needs_input" => AgentAttention::NeedsInput,
            _ => return None,
        };
        return Some(vec![
            RuntimeEvent::Lifecycle {
                session_id: session_id.clone(),
                lifecycle: AgentLifecycle::Waiting,
            },
            RuntimeEvent::Attention {
                session_id: session_id.clone(),
                attention,
            },
        ]);
    }
    if let Some(rest) = trimmed.strip_prefix(AXIS_STATUS_PREFIX) {
        return Some(vec![RuntimeEvent::Status {
            session_id: session_id.clone(),
            message: rest.trim().to_string(),
        }]);
    }
    if let Some(rest) = trimmed.strip_prefix(AXIS_APPROVAL_REQUEST_PREFIX) {
        let approval = serde_json::from_str::<AgentApprovalRequest>(rest).ok()?;
        return Some(vec![RuntimeEvent::ApprovalRequest {
            session_id: session_id.clone(),
            approval,
        }]);
    }
    None
}

pub fn encode_axis_command(command: &AxisCliCommand) -> anyhow::Result<String> {
    let json = serde_json::to_string(command).context("serialize axis CLI command")?;
    Ok(format!("{AXIS_CMD_PREFIX}{json}\n"))
}

fn axis_cli_event_to_runtime_events(
    session_id: &AgentSessionId,
    event: AxisCliEvent,
) -> Vec<RuntimeEvent> {
    match event {
        AxisCliEvent::Lifecycle { lifecycle } => vec![RuntimeEvent::Lifecycle {
            session_id: session_id.clone(),
            lifecycle,
        }],
        AxisCliEvent::Attention { attention } => vec![RuntimeEvent::Attention {
            session_id: session_id.clone(),
            attention,
        }],
        AxisCliEvent::Status { message } => vec![RuntimeEvent::Status {
            session_id: session_id.clone(),
            message,
        }],
        AxisCliEvent::Turn { turn } => vec![RuntimeEvent::Turn {
            session_id: session_id.clone(),
            turn,
        }],
        AxisCliEvent::ToolCall { tool_call } => vec![RuntimeEvent::ToolCall {
            session_id: session_id.clone(),
            tool_call,
        }],
        AxisCliEvent::ApprovalRequest { approval } => vec![RuntimeEvent::ApprovalRequest {
            session_id: session_id.clone(),
            approval,
        }],
    }
}
