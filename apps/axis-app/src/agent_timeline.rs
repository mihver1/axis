use axis_core::agent::AgentLifecycle;
use axis_core::agent_history::{
    AgentApprovalRequest, AgentApprovalRequestId, AgentApprovalState, AgentSessionDetail,
    AgentTimelineEntry, AgentTurnRole,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AgentTimelineEntryView {
    pub sequence: u64,
    pub title: String,
    pub body: String,
    pub state_label: String,
    pub pending: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PendingApprovalView {
    pub id: AgentApprovalRequestId,
    pub title: String,
    pub details: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AgentTimelineViewModel {
    pub can_send_turn: bool,
    pub can_resume: bool,
    pub can_respond_approval: bool,
    pub timeline_entries: Vec<AgentTimelineEntryView>,
    pub pending_approvals: Vec<PendingApprovalView>,
}

pub(crate) fn build_agent_timeline_view_model(
    detail: &AgentSessionDetail,
) -> AgentTimelineViewModel {
    let timeline_entries = detail
        .timeline
        .iter()
        .map(|entry| match entry {
            AgentTimelineEntry::Turn { sequence, turn } => AgentTimelineEntryView {
                sequence: *sequence,
                title: match turn.role {
                    AgentTurnRole::User => "User turn".to_string(),
                    AgentTurnRole::Assistant => "Assistant turn".to_string(),
                    AgentTurnRole::System => "System turn".to_string(),
                },
                body: turn.text.clone(),
                state_label: turn_state_label(turn.state).to_string(),
                pending: false,
            },
            AgentTimelineEntry::ToolCall {
                sequence,
                tool_call,
            } => AgentTimelineEntryView {
                sequence: *sequence,
                title: tool_call.title.clone(),
                body: tool_call
                    .output
                    .clone()
                    .unwrap_or_else(|| tool_call.details.clone()),
                state_label: tool_call_state_label(tool_call.state).to_string(),
                pending: false,
            },
            AgentTimelineEntry::ApprovalRequest {
                sequence,
                approval,
            } => AgentTimelineEntryView {
                sequence: *sequence,
                title: approval.title.clone(),
                body: approval_summary_body(approval),
                state_label: approval_state_label(approval.state).to_string(),
                pending: approval.state == AgentApprovalState::Pending,
            },
        })
        .collect::<Vec<_>>();
    let pending_approvals = detail
        .timeline
        .iter()
        .filter_map(|entry| match entry {
            AgentTimelineEntry::ApprovalRequest { approval, .. }
                if approval.state == AgentApprovalState::Pending =>
            {
                Some(PendingApprovalView {
                    id: approval.id.clone(),
                    title: approval.title.clone(),
                    details: approval.details.clone(),
                })
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    AgentTimelineViewModel {
        can_send_turn: detail.capabilities.turn_input,
        can_resume: detail.capabilities.resume
            && !matches!(
                detail.session.lifecycle,
                AgentLifecycle::Completed | AgentLifecycle::Failed | AgentLifecycle::Cancelled
            ),
        can_respond_approval: detail.capabilities.approvals,
        timeline_entries,
        pending_approvals,
    }
}

fn turn_state_label(state: axis_core::agent_history::AgentTurnState) -> &'static str {
    match state {
        axis_core::agent_history::AgentTurnState::Pending => "pending",
        axis_core::agent_history::AgentTurnState::Streaming => "streaming",
        axis_core::agent_history::AgentTurnState::Completed => "completed",
        axis_core::agent_history::AgentTurnState::Failed => "failed",
        axis_core::agent_history::AgentTurnState::Cancelled => "cancelled",
    }
}

fn tool_call_state_label(state: axis_core::agent_history::AgentToolCallState) -> &'static str {
    match state {
        axis_core::agent_history::AgentToolCallState::Pending => "pending",
        axis_core::agent_history::AgentToolCallState::Running => "running",
        axis_core::agent_history::AgentToolCallState::Completed => "completed",
        axis_core::agent_history::AgentToolCallState::Failed => "failed",
        axis_core::agent_history::AgentToolCallState::Cancelled => "cancelled",
    }
}

fn approval_summary_body(approval: &AgentApprovalRequest) -> String {
    match approval.decision.as_ref().and_then(|decision| decision.note.as_deref()) {
        Some(note) if !note.trim().is_empty() => format!("{}\n{}", approval.details, note),
        _ => approval.details.clone(),
    }
}

fn approval_state_label(state: AgentApprovalState) -> &'static str {
    match state {
        AgentApprovalState::Pending => "pending",
        AgentApprovalState::Approved => "approved",
        AgentApprovalState::Denied => "denied",
        AgentApprovalState::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axis_core::agent::{
        AgentAttention, AgentLifecycle, AgentSessionId, AgentSessionRecord, AgentTransportKind,
    };
    use axis_core::agent_history::{
        AgentApprovalDecision, AgentApprovalKind, AgentSessionCapabilities, AgentTurn, AgentTurnId,
        AgentTurnState,
    };

    fn detail_with_pending_approval() -> AgentSessionDetail {
        AgentSessionDetail {
            session: AgentSessionRecord {
                id: AgentSessionId::new("sess-1"),
                provider_profile_id: "fake".to_string(),
                transport: AgentTransportKind::CliWrapped,
                workdesk_id: Some("desk-1".to_string()),
                surface_id: None,
                cwd: "/repo".to_string(),
                lifecycle: AgentLifecycle::Waiting,
                attention: AgentAttention::NeedsReview,
                status_message: "approval required".to_string(),
            },
            capabilities: AgentSessionCapabilities {
                turn_input: true,
                tool_calls: true,
                approvals: true,
                resume: true,
                terminal_attachment: true,
            },
            started_at_ms: Some(1),
            updated_at_ms: Some(2),
            completed_at_ms: None,
            revision: 3,
            history_cursor: 2,
            pending_approval_id: Some(AgentApprovalRequestId::new("approval-1")),
            timeline: vec![
                AgentTimelineEntry::Turn {
                    sequence: 0,
                    turn: AgentTurn {
                        id: AgentTurnId::new("turn-1"),
                        role: AgentTurnRole::User,
                        state: AgentTurnState::Completed,
                        text: "Continue with the plan.".to_string(),
                        created_at_ms: 10,
                        completed_at_ms: Some(11),
                    },
                },
                AgentTimelineEntry::ApprovalRequest {
                    sequence: 1,
                    approval: AgentApprovalRequest {
                        id: AgentApprovalRequestId::new("approval-1"),
                        kind: AgentApprovalKind::Command,
                        title: "Allow command?".to_string(),
                        details: "run cargo test".to_string(),
                        state: AgentApprovalState::Pending,
                        tool_call_id: None,
                        requested_at_ms: 12,
                        decision: None,
                    },
                },
            ],
            truncated: false,
        }
    }

    #[test]
    fn timeline_model_exposes_pending_approval_and_actions() {
        let model = build_agent_timeline_view_model(&detail_with_pending_approval());

        assert!(model.can_send_turn);
        assert!(model.can_resume);
        assert!(model.can_respond_approval);
        assert_eq!(model.timeline_entries.len(), 2);
        assert_eq!(model.timeline_entries[0].title, "User turn");
        assert_eq!(model.timeline_entries[1].state_label, "pending");
        assert_eq!(model.pending_approvals.len(), 1);
        assert_eq!(model.pending_approvals[0].id, AgentApprovalRequestId::new("approval-1"));
    }

    #[test]
    fn timeline_model_includes_approval_note_when_decided() {
        let mut detail = detail_with_pending_approval();
        let AgentTimelineEntry::ApprovalRequest { approval, .. } = &mut detail.timeline[1] else {
            panic!("expected approval entry");
        };
        approval.state = AgentApprovalState::Approved;
        approval.decision = Some(AgentApprovalDecision {
            approved: true,
            note: Some("Ship it.".to_string()),
            decided_at_ms: 13,
        });
        detail.pending_approval_id = None;

        let model = build_agent_timeline_view_model(&detail);

        assert!(model.pending_approvals.is_empty());
        assert!(model.timeline_entries[1].body.contains("Ship it."));
        assert_eq!(model.timeline_entries[1].state_label, "approved");
    }
}
