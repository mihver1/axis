//! Serialization round-trips for structured agent timeline/detail records.

use axis_core::agent::{
    AgentAttention, AgentLifecycle, AgentSessionId, AgentSessionRecord, AgentTransportKind,
};
use axis_core::agent_history::{
    AgentApprovalDecision, AgentApprovalKind, AgentApprovalRequest, AgentApprovalRequestId,
    AgentApprovalState, AgentSessionCapabilities, AgentSessionDetail, AgentTimelineEntry,
    AgentToolCall, AgentToolCallId, AgentToolCallState, AgentTurn, AgentTurnId, AgentTurnRole,
    AgentTurnState,
};
use axis_core::automation::{
    AgentGetRequest, AgentRespondApprovalRequest, AgentResumeRequest, AgentSendTurnRequest,
    AutomationRequest,
};
use axis_core::SurfaceId;

fn demo_session_record() -> AgentSessionRecord {
    AgentSessionRecord {
        id: AgentSessionId::new("sess-structured-1"),
        provider_profile_id: "codex".to_string(),
        transport: AgentTransportKind::CliWrapped,
        workdesk_id: Some("desk-1".to_string()),
        surface_id: Some(SurfaceId::new(7)),
        cwd: "/repo/wt".to_string(),
        lifecycle: AgentLifecycle::Waiting,
        attention: AgentAttention::NeedsReview,
        status_message: "approval required".to_string(),
    }
}

#[test]
fn agent_session_detail_round_trips_structured_timeline() {
    let detail = AgentSessionDetail {
        session: demo_session_record(),
        capabilities: AgentSessionCapabilities {
            turn_input: true,
            tool_calls: true,
            approvals: true,
            resume: true,
            terminal_attachment: true,
        },
        started_at_ms: Some(100),
        updated_at_ms: Some(125),
        completed_at_ms: None,
        revision: 4,
        history_cursor: 3,
        pending_approval_id: Some(AgentApprovalRequestId::new("approval-1")),
        timeline: vec![
            AgentTimelineEntry::Turn {
                sequence: 0,
                turn: AgentTurn {
                    id: AgentTurnId::new("turn-1"),
                    role: AgentTurnRole::User,
                    state: AgentTurnState::Completed,
                    text: "Run the formatter.".to_string(),
                    created_at_ms: 100,
                    completed_at_ms: Some(101),
                },
            },
            AgentTimelineEntry::ToolCall {
                sequence: 1,
                tool_call: AgentToolCall {
                    id: AgentToolCallId::new("tool-1"),
                    title: "cargo fmt".to_string(),
                    state: AgentToolCallState::Completed,
                    details: "Formatting workspace".to_string(),
                    output: Some("done".to_string()),
                    started_at_ms: Some(105),
                    finished_at_ms: Some(110),
                },
            },
            AgentTimelineEntry::ApprovalRequest {
                sequence: 2,
                approval: AgentApprovalRequest {
                    id: AgentApprovalRequestId::new("approval-1"),
                    kind: AgentApprovalKind::Command,
                    title: "Allow `git add`?".to_string(),
                    details: "Stage modified files before commit.".to_string(),
                    state: AgentApprovalState::Pending,
                    tool_call_id: Some(AgentToolCallId::new("tool-1")),
                    requested_at_ms: 120,
                    decision: None,
                },
            },
        ],
        truncated: false,
    };

    let json = serde_json::to_value(&detail).unwrap();
    assert_eq!(json["session"]["id"], "sess-structured-1");
    assert_eq!(json["timeline"][0]["kind"], "turn");
    assert_eq!(json["timeline"][1]["tool_call"]["title"], "cargo fmt");
    assert_eq!(json["pending_approval_id"], "approval-1");
    assert_eq!(json["history_cursor"], 3);

    let back: AgentSessionDetail = serde_json::from_value(json).unwrap();
    assert_eq!(back, detail);
}

#[test]
fn approval_decision_round_trips_with_note() {
    let request = AgentApprovalRequest {
        id: AgentApprovalRequestId::new("approval-2"),
        kind: AgentApprovalKind::Patch,
        title: "Apply diff?".to_string(),
        details: "Patch touches tracked files.".to_string(),
        state: AgentApprovalState::Approved,
        tool_call_id: None,
        requested_at_ms: 200,
        decision: Some(AgentApprovalDecision {
            approved: true,
            note: Some("Safe to apply.".to_string()),
            decided_at_ms: 210,
        }),
    };

    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["decision"]["approved"], true);
    assert_eq!(json["decision"]["note"], "Safe to apply.");

    let back: AgentApprovalRequest = serde_json::from_value(json).unwrap();
    assert_eq!(back, request);
}

#[test]
fn automation_request_encodes_agent_structured_actions() {
    let get = AutomationRequest::AgentGet(AgentGetRequest {
        agent_session_id: AgentSessionId::new("sess-1"),
        after_sequence: Some(4),
    });
    let send_turn = AutomationRequest::AgentSendTurn(AgentSendTurnRequest {
        agent_session_id: AgentSessionId::new("sess-1"),
        text: "Continue and explain the diff.".to_string(),
    });
    let approve = AutomationRequest::AgentRespondApproval(AgentRespondApprovalRequest {
        agent_session_id: AgentSessionId::new("sess-1"),
        approval_request_id: AgentApprovalRequestId::new("approval-9"),
        approved: true,
        note: Some("Looks safe.".to_string()),
    });
    let resume = AutomationRequest::AgentResume(AgentResumeRequest {
        agent_session_id: AgentSessionId::new("sess-1"),
    });

    let get_json = serde_json::to_value(&get).unwrap();
    assert_eq!(get_json["method"], "agent.get");
    assert_eq!(get_json["params"]["after_sequence"], 4);

    let turn_json = serde_json::to_value(&send_turn).unwrap();
    assert_eq!(turn_json["method"], "agent.send_turn");
    assert_eq!(
        turn_json["params"]["text"],
        "Continue and explain the diff."
    );

    let approve_json = serde_json::to_value(&approve).unwrap();
    assert_eq!(approve_json["method"], "agent.respond_approval");
    assert_eq!(approve_json["params"]["approval_request_id"], "approval-9");
    assert_eq!(approve_json["params"]["approved"], true);

    let resume_json = serde_json::to_value(&resume).unwrap();
    assert_eq!(resume_json["method"], "agent.resume");
    assert_eq!(resume_json["params"]["agent_session_id"], "sess-1");
}
