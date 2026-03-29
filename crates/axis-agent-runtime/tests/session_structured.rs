//! Structured session detail storage and provider action routing.

use std::collections::BTreeMap;
use std::sync::Arc;

use axis_agent_runtime::adapters::fake::FakeProvider;
use axis_agent_runtime::events::RuntimeEvent;
use axis_agent_runtime::provider::{ProviderRegistry, StartAgentRequest};
use axis_agent_runtime::SessionManager;
use axis_core::agent::{AgentAttention, AgentLifecycle, AgentTransportKind};
use axis_core::agent_history::{
    AgentApprovalDecision, AgentApprovalKind, AgentApprovalRequest, AgentApprovalRequestId,
    AgentApprovalState, AgentSessionCapabilities, AgentTimelineEntry, AgentToolCall,
    AgentToolCallId, AgentToolCallState, AgentTurn, AgentTurnId, AgentTurnRole, AgentTurnState,
};

fn new_manager() -> SessionManager {
    let mut reg = ProviderRegistry::new();
    reg.register("fake", Arc::new(FakeProvider::with_standard_script()));
    SessionManager::new(reg)
}

fn start_session(mgr: &mut SessionManager) -> axis_core::agent::AgentSessionId {
    mgr.start_session(StartAgentRequest {
        cwd: "/tmp/wt".into(),
        provider_profile_id: "fake".into(),
        transport: AgentTransportKind::CliWrapped,
        argv_suffix: vec![],
        env: BTreeMap::new(),
    })
    .unwrap()
}

#[test]
fn start_session_seeds_structured_detail_with_provider_capabilities() {
    let mut mgr = new_manager();
    let id = start_session(&mut mgr);

    let detail = mgr.session_detail(&id).expect("detail should exist");
    assert_eq!(detail.session.id, id);
    assert_eq!(
        detail.capabilities,
        AgentSessionCapabilities {
            turn_input: true,
            tool_calls: true,
            approvals: true,
            resume: true,
            terminal_attachment: false,
        }
    );
    assert_eq!(detail.revision, 0);
    assert_eq!(detail.history_cursor, 0);
    assert!(detail.timeline.is_empty());
}

#[test]
fn apply_events_upserts_structured_timeline_and_pending_approval() {
    let mut mgr = new_manager();
    let id = start_session(&mut mgr);

    mgr.apply_events([
        RuntimeEvent::Turn {
            session_id: id.clone(),
            turn: AgentTurn {
                id: AgentTurnId::new("turn-1"),
                role: AgentTurnRole::Assistant,
                state: AgentTurnState::Completed,
                text: "I need to run a command.".into(),
                created_at_ms: 10,
                completed_at_ms: Some(11),
            },
        },
        RuntimeEvent::ToolCall {
            session_id: id.clone(),
            tool_call: AgentToolCall {
                id: AgentToolCallId::new("tool-1"),
                title: "cargo check".into(),
                state: AgentToolCallState::Running,
                details: "Validating the workspace".into(),
                output: None,
                started_at_ms: Some(12),
                finished_at_ms: None,
            },
        },
        RuntimeEvent::ApprovalRequest {
            session_id: id.clone(),
            approval: AgentApprovalRequest {
                id: AgentApprovalRequestId::new("approval-1"),
                kind: AgentApprovalKind::Command,
                title: "Allow `cargo check`?".into(),
                details: "The agent wants to run a workspace build.".into(),
                state: AgentApprovalState::Pending,
                tool_call_id: Some(AgentToolCallId::new("tool-1")),
                requested_at_ms: 13,
                decision: None,
            },
        },
    ])
    .unwrap();

    let detail = mgr.session_detail(&id).unwrap();
    assert_eq!(detail.history_cursor, 3);
    assert_eq!(
        detail.pending_approval_id,
        Some(AgentApprovalRequestId::new("approval-1"))
    );
    assert!(matches!(
        detail.timeline.as_slice(),
        [
            AgentTimelineEntry::Turn { sequence: 0, .. },
            AgentTimelineEntry::ToolCall { sequence: 1, .. },
            AgentTimelineEntry::ApprovalRequest { sequence: 2, .. }
        ]
    ));

    mgr.apply_events([RuntimeEvent::ApprovalRequest {
        session_id: id.clone(),
        approval: AgentApprovalRequest {
            id: AgentApprovalRequestId::new("approval-1"),
            kind: AgentApprovalKind::Command,
            title: "Allow `cargo check`?".into(),
            details: "The agent wants to run a workspace build.".into(),
            state: AgentApprovalState::Approved,
            tool_call_id: Some(AgentToolCallId::new("tool-1")),
            requested_at_ms: 13,
            decision: Some(AgentApprovalDecision {
                approved: true,
                note: Some("Proceed.".into()),
                decided_at_ms: 14,
            }),
        },
    }])
    .unwrap();

    let detail = mgr.session_detail(&id).unwrap();
    assert_eq!(detail.history_cursor, 3, "updating an existing entry should not append");
    assert_eq!(detail.pending_approval_id, None);
    match &detail.timeline[2] {
        AgentTimelineEntry::ApprovalRequest { sequence, approval } => {
            assert_eq!(*sequence, 2);
            assert_eq!(approval.state, AgentApprovalState::Approved);
            assert_eq!(approval.decision.as_ref().unwrap().note.as_deref(), Some("Proceed."));
        }
        entry => panic!("expected approval entry, got {entry:?}"),
    }
}

#[test]
fn send_turn_respond_approval_and_resume_apply_provider_events() {
    let mut mgr = new_manager();
    let id = start_session(&mut mgr);

    mgr.send_turn(&id, "Continue with a concise summary.")
        .unwrap();
    let detail = mgr.session_detail(&id).unwrap();
    assert_eq!(detail.history_cursor, 1);
    match &detail.timeline[0] {
        AgentTimelineEntry::Turn { turn, .. } => {
            assert_eq!(turn.role, AgentTurnRole::User);
            assert_eq!(turn.text, "Continue with a concise summary.");
        }
        entry => panic!("expected turn entry, got {entry:?}"),
    }
    assert_eq!(mgr.session(&id).unwrap().status_message, "turn submitted");

    mgr.respond_approval(
        &id,
        &AgentApprovalRequestId::new("fake-approval-1"),
        true,
        Some("Approved.".into()),
    )
    .unwrap();
    let detail = mgr.session_detail(&id).unwrap();
    assert_eq!(detail.pending_approval_id, None);
    assert_eq!(detail.history_cursor, 2);
    match &detail.timeline[1] {
        AgentTimelineEntry::ApprovalRequest { approval, .. } => {
            assert_eq!(approval.state, AgentApprovalState::Approved);
            assert_eq!(approval.decision.as_ref().unwrap().approved, true);
        }
        entry => panic!("expected approval entry, got {entry:?}"),
    }

    mgr.resume(&id).unwrap();
    assert_eq!(mgr.session(&id).unwrap().attention, AgentAttention::Working);
    assert_eq!(mgr.session(&id).unwrap().status_message, "resumed");
    assert_eq!(mgr.session(&id).unwrap().lifecycle, AgentLifecycle::Running);
}
