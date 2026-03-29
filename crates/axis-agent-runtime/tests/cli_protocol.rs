//! Shared CLI protocol for structured provider events and commands.

use axis_agent_runtime::cli_protocol::{
    encode_axis_command, parse_axis_output_line, AxisCliCommand,
};
use axis_agent_runtime::RuntimeEvent;
use axis_core::agent::{AgentAttention, AgentLifecycle, AgentSessionId};
use axis_core::agent_history::{
    AgentApprovalKind, AgentApprovalRequest, AgentApprovalRequestId, AgentApprovalState,
};

#[test]
fn parse_axis_event_line_decodes_structured_approval_request() {
    let events = parse_axis_output_line(
        r#"AXIS_EVENT {"kind":"approval_request","approval":{"id":"approval-1","kind":"command","title":"Allow command?","details":"run cargo test","state":"pending","requested_at_ms":10}}"#,
        &AgentSessionId::new("sess-1"),
    )
    .expect("structured line should parse");

    assert_eq!(
        events,
        vec![RuntimeEvent::ApprovalRequest {
            session_id: AgentSessionId::new("sess-1"),
            approval: AgentApprovalRequest {
                id: AgentApprovalRequestId::new("approval-1"),
                kind: AgentApprovalKind::Command,
                title: "Allow command?".to_string(),
                details: "run cargo test".to_string(),
                state: AgentApprovalState::Pending,
                tool_call_id: None,
                requested_at_ms: 10,
                decision: None,
            },
        }]
    );
}

#[test]
fn parse_axis_output_line_keeps_legacy_attention_marker_compatibility() {
    let events = parse_axis_output_line(
        "AXIS_ATTENTION needs_review",
        &AgentSessionId::new("sess-legacy"),
    )
    .expect("legacy marker should parse");

    assert_eq!(
        events,
        vec![
            RuntimeEvent::Lifecycle {
                session_id: AgentSessionId::new("sess-legacy"),
                lifecycle: AgentLifecycle::Waiting,
            },
            RuntimeEvent::Attention {
                session_id: AgentSessionId::new("sess-legacy"),
                attention: AgentAttention::NeedsReview,
            },
        ]
    );
}

#[test]
fn encode_axis_command_serializes_send_turn_as_json_line() {
    let line = encode_axis_command(&AxisCliCommand::SendTurn {
        text: "Continue with the fix.".to_string(),
    })
    .unwrap();

    assert!(line.starts_with("AXIS_CMD "));
    assert!(line.ends_with('\n'));
    assert!(line.contains(r#""kind":"send_turn""#));
    assert!(line.contains(r#""text":"Continue with the fix.""#));
}
