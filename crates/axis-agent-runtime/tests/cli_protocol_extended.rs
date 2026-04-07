//! Extended tests for the AXIS_APPROVAL_REQUEST protocol line and edge cases.

use axis_agent_runtime::cli_protocol::{parse_axis_output_line, AXIS_APPROVAL_REQUEST_PREFIX};
use axis_agent_runtime::RuntimeEvent;
use axis_core::agent::AgentSessionId;
use axis_core::agent_history::{
    AgentApprovalKind, AgentApprovalRequest, AgentApprovalRequestId, AgentApprovalState,
};

#[test]
fn parse_axis_approval_request_line_emits_approval_request_event() {
    let line = format!(
        r#"{}{{"id":"approval-42","kind":"command","title":"Allow rm?","details":"rm -rf /tmp/foo","state":"pending","requested_at_ms":999}}"#,
        AXIS_APPROVAL_REQUEST_PREFIX,
    );
    let session_id = AgentSessionId::new("sess-approvals");
    let events = parse_axis_output_line(&line, &session_id).expect("should parse approval request");

    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        RuntimeEvent::ApprovalRequest {
            session_id: session_id.clone(),
            approval: AgentApprovalRequest {
                id: AgentApprovalRequestId::new("approval-42"),
                kind: AgentApprovalKind::Command,
                title: "Allow rm?".to_string(),
                details: "rm -rf /tmp/foo".to_string(),
                state: AgentApprovalState::Pending,
                tool_call_id: None,
                requested_at_ms: 999,
                decision: None,
            },
        }
    );
}

#[test]
fn parse_axis_approval_request_malformed_json_returns_none() {
    let line = "AXIS_APPROVAL_REQUEST {not valid json}";
    let result = parse_axis_output_line(line, &AgentSessionId::new("sess-x"));
    assert!(result.is_none(), "malformed JSON should return None");
}

#[test]
fn parse_axis_status_still_works_after_approval_request_added() {
    let line = "AXIS_STATUS compiling…";
    let session_id = AgentSessionId::new("sess-status");
    let events = parse_axis_output_line(line, &session_id).expect("AXIS_STATUS should still parse");
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        RuntimeEvent::Status {
            session_id,
            message: "compiling…".to_string(),
        }
    );
}

#[test]
fn parse_axis_output_unknown_prefix_returns_none() {
    let line = "AXIS_UNKNOWN whatever";
    let result = parse_axis_output_line(line, &AgentSessionId::new("sess-y"));
    assert!(result.is_none(), "unknown prefix should return None");
}
