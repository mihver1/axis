//! Session registry, lifecycle/attention transitions, and UI revision counter.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context};
use axis_core::agent::{AgentAttention, AgentLifecycle, AgentSessionId, AgentSessionRecord};
use axis_core::agent_history::{
    AgentApprovalRequest, AgentApprovalRequestId, AgentApprovalState, AgentSessionDetail,
    AgentTimelineEntry, AgentToolCall, AgentTurn,
};

use crate::events::RuntimeEvent;
use crate::provider::{
    validate_start_request, ProviderProfileMetadata, ProviderRegistry, RespondApprovalRequest,
    ResumeRequest, SendTurnRequest, StartAgentRequest,
};

/// Owns agent session records, structured detail, provider lookup, and monotonic revision for UI refresh.
pub struct SessionManager {
    sessions: HashMap<AgentSessionId, AgentSessionDetail>,
    registry: ProviderRegistry,
    revision: u64,
}

impl SessionManager {
    pub fn new(registry: ProviderRegistry) -> Self {
        Self {
            sessions: HashMap::new(),
            registry,
            revision: 0,
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn session(&self, id: &AgentSessionId) -> Option<&AgentSessionRecord> {
        self.sessions.get(id).map(|detail| &detail.session)
    }

    pub fn session_detail(&self, id: &AgentSessionId) -> Option<&AgentSessionDetail> {
        self.sessions.get(id)
    }

    pub fn sessions(&self) -> impl Iterator<Item = &AgentSessionRecord> {
        self.sessions.values().map(|detail| &detail.session)
    }

    pub fn provider_profile(&self, profile_id: &str) -> Option<ProviderProfileMetadata> {
        self.registry.metadata(profile_id)
    }

    pub fn provider_profiles(&self) -> Vec<ProviderProfileMetadata> {
        self.registry.profiles()
    }

    /// Starts a session via the registry-resolved provider and seeds the local record.
    pub fn start_session(&mut self, req: StartAgentRequest) -> anyhow::Result<AgentSessionId> {
        validate_start_request(&req)?;
        let provider = self.registry.require(&req.provider_profile_id)?;
        let started = provider.start(req.clone())?;
        let id = started.session_id.clone();

        if self.sessions.contains_key(&id) {
            return Err(anyhow!("provider returned duplicate session id {}", id.0));
        }

        let record = AgentSessionRecord {
            id: id.clone(),
            provider_profile_id: req.provider_profile_id.clone(),
            transport: req.transport,
            workdesk_id: None,
            surface_id: None,
            cwd: req.cwd,
            lifecycle: AgentLifecycle::Planned,
            attention: AgentAttention::Quiet,
            status_message: String::new(),
        };
        let now = now_ms();
        self.sessions.insert(
            id.clone(),
            AgentSessionDetail {
                session: record,
                capabilities: provider.capabilities(),
                started_at_ms: now,
                updated_at_ms: now,
                completed_at_ms: None,
                revision: 0,
                history_cursor: 0,
                pending_approval_id: None,
                timeline: Vec::new(),
                truncated: false,
            },
        );
        self.bump_revision();
        Ok(id)
    }

    /// Applies provider events, updating lifecycle, attention, status text, and structured timeline entries.
    pub fn apply_events(
        &mut self,
        events: impl IntoIterator<Item = RuntimeEvent>,
    ) -> anyhow::Result<()> {
        for event in events {
            self.apply_event(event)?;
        }
        Ok(())
    }

    /// Polls the provider registered for the session’s profile and applies emitted events.
    pub fn poll_provider(&mut self, session_id: &AgentSessionId) -> anyhow::Result<()> {
        let provider = self.provider_for_session(session_id)?;
        let events = provider.poll_events(session_id)?;
        self.apply_events(events)
    }

    pub fn send_turn(&mut self, session_id: &AgentSessionId, text: &str) -> anyhow::Result<()> {
        let provider = self.provider_for_session(session_id)?;
        let events = provider.send_turn(SendTurnRequest {
            session_id: session_id.clone(),
            text: text.to_string(),
        })?;
        self.apply_events(events)
    }

    pub fn respond_approval(
        &mut self,
        session_id: &AgentSessionId,
        approval_request_id: &AgentApprovalRequestId,
        approved: bool,
        note: Option<String>,
    ) -> anyhow::Result<()> {
        let provider = self.provider_for_session(session_id)?;
        let events = provider.respond_approval(RespondApprovalRequest {
            session_id: session_id.clone(),
            approval_request_id: approval_request_id.clone(),
            approved,
            note,
        })?;
        self.apply_events(events)
    }

    pub fn resume(&mut self, session_id: &AgentSessionId) -> anyhow::Result<()> {
        let provider = self.provider_for_session(session_id)?;
        let events = provider.resume(ResumeRequest {
            session_id: session_id.clone(),
        })?;
        self.apply_events(events)
    }

    /// Stops the provider-backed session, then drops the local record.
    pub fn stop_session(&mut self, session_id: &AgentSessionId) -> anyhow::Result<()> {
        let provider = self.provider_for_session(session_id)?;
        provider.stop(session_id)?;
        self.sessions
            .remove(session_id)
            .ok_or_else(|| anyhow!("session disappeared during stop: {}", session_id.0))?;
        self.bump_revision();
        Ok(())
    }

    /// Local lifecycle transition (e.g. UI or host-driven); bumps revision when the state changes.
    pub fn transition_lifecycle(
        &mut self,
        session_id: &AgentSessionId,
        lifecycle: AgentLifecycle,
    ) -> anyhow::Result<()> {
        self.apply_events([RuntimeEvent::Lifecycle {
            session_id: session_id.clone(),
            lifecycle,
        }])
    }

    /// Local attention transition; bumps revision when the state changes.
    pub fn transition_attention(
        &mut self,
        session_id: &AgentSessionId,
        attention: AgentAttention,
    ) -> anyhow::Result<()> {
        self.apply_events([RuntimeEvent::Attention {
            session_id: session_id.clone(),
            attention,
        }])
    }

    fn apply_event(&mut self, event: RuntimeEvent) -> anyhow::Result<()> {
        match event {
            RuntimeEvent::Lifecycle {
                session_id,
                lifecycle,
            } => {
                let changed = {
                    let detail = self
                        .sessions
                        .get_mut(&session_id)
                        .with_context(|| format!("unknown session {}", session_id.0))?;
                    if detail.session.lifecycle == lifecycle {
                        false
                    } else {
                        detail.session.lifecycle = lifecycle;
                        detail.updated_at_ms = now_ms().or(detail.updated_at_ms);
                        detail.completed_at_ms =
                            is_terminal_lifecycle(lifecycle).then(now_ms).flatten();
                        detail.revision = detail.revision.wrapping_add(1);
                        true
                    }
                };
                if changed {
                    self.bump_revision();
                }
            }
            RuntimeEvent::Attention {
                session_id,
                attention,
            } => {
                let changed = {
                    let detail = self
                        .sessions
                        .get_mut(&session_id)
                        .with_context(|| format!("unknown session {}", session_id.0))?;
                    if detail.session.attention == attention {
                        false
                    } else {
                        detail.session.attention = attention;
                        detail.updated_at_ms = now_ms().or(detail.updated_at_ms);
                        detail.revision = detail.revision.wrapping_add(1);
                        true
                    }
                };
                if changed {
                    self.bump_revision();
                }
            }
            RuntimeEvent::Status {
                session_id,
                message,
            } => {
                let changed = {
                    let detail = self
                        .sessions
                        .get_mut(&session_id)
                        .with_context(|| format!("unknown session {}", session_id.0))?;
                    if detail.session.status_message == message {
                        false
                    } else {
                        detail.session.status_message = message;
                        detail.updated_at_ms = now_ms().or(detail.updated_at_ms);
                        detail.revision = detail.revision.wrapping_add(1);
                        true
                    }
                };
                if changed {
                    self.bump_revision();
                }
            }
            RuntimeEvent::Turn { session_id, turn } => {
                let changed = {
                    let detail = self
                        .sessions
                        .get_mut(&session_id)
                        .with_context(|| format!("unknown session {}", session_id.0))?;
                    let changed = upsert_turn(detail, turn);
                    if changed {
                        detail.updated_at_ms = now_ms().or(detail.updated_at_ms);
                        detail.revision = detail.revision.wrapping_add(1);
                    }
                    changed
                };
                if changed {
                    self.bump_revision();
                }
            }
            RuntimeEvent::ToolCall {
                session_id,
                tool_call,
            } => {
                let changed = {
                    let detail = self
                        .sessions
                        .get_mut(&session_id)
                        .with_context(|| format!("unknown session {}", session_id.0))?;
                    let changed = upsert_tool_call(detail, tool_call);
                    if changed {
                        detail.updated_at_ms = now_ms().or(detail.updated_at_ms);
                        detail.revision = detail.revision.wrapping_add(1);
                    }
                    changed
                };
                if changed {
                    self.bump_revision();
                }
            }
            RuntimeEvent::ApprovalRequest {
                session_id,
                approval,
            } => {
                let changed = {
                    let detail = self
                        .sessions
                        .get_mut(&session_id)
                        .with_context(|| format!("unknown session {}", session_id.0))?;
                    let changed = upsert_approval(detail, approval);
                    if changed {
                        recompute_pending_approval_id(detail);
                        detail.updated_at_ms = now_ms().or(detail.updated_at_ms);
                        detail.revision = detail.revision.wrapping_add(1);
                    }
                    changed
                };
                if changed {
                    self.bump_revision();
                }
            }
        }
        Ok(())
    }

    fn provider_for_session(
        &self,
        session_id: &AgentSessionId,
    ) -> anyhow::Result<std::sync::Arc<dyn crate::provider::AgentProvider>> {
        let profile_id = self
            .session(session_id)
            .map(|s| s.provider_profile_id.clone())
            .with_context(|| format!("unknown session {}", session_id.0))?;
        self.registry.require(&profile_id)
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

fn upsert_turn(detail: &mut AgentSessionDetail, turn: AgentTurn) -> bool {
    if let Some(existing) = detail.timeline.iter_mut().find_map(|entry| match entry {
        AgentTimelineEntry::Turn {
            sequence: _,
            turn: existing,
        } if existing.id == turn.id => Some(existing),
        _ => None,
    }) {
        if *existing == turn {
            return false;
        }
        *existing = turn;
        return true;
    }
    let sequence = detail.history_cursor;
    detail.history_cursor = detail.history_cursor.wrapping_add(1);
    detail
        .timeline
        .push(AgentTimelineEntry::Turn { sequence, turn });
    true
}

fn upsert_tool_call(detail: &mut AgentSessionDetail, tool_call: AgentToolCall) -> bool {
    if let Some(existing) = detail.timeline.iter_mut().find_map(|entry| match entry {
        AgentTimelineEntry::ToolCall {
            sequence: _,
            tool_call: existing,
        } if existing.id == tool_call.id => Some(existing),
        _ => None,
    }) {
        if *existing == tool_call {
            return false;
        }
        *existing = tool_call;
        return true;
    }
    let sequence = detail.history_cursor;
    detail.history_cursor = detail.history_cursor.wrapping_add(1);
    detail.timeline.push(AgentTimelineEntry::ToolCall {
        sequence,
        tool_call,
    });
    true
}

fn upsert_approval(detail: &mut AgentSessionDetail, approval: AgentApprovalRequest) -> bool {
    if let Some(existing) = detail.timeline.iter_mut().find_map(|entry| match entry {
        AgentTimelineEntry::ApprovalRequest {
            sequence: _,
            approval: existing,
        } if existing.id == approval.id => Some(existing),
        _ => None,
    }) {
        if *existing == approval {
            return false;
        }
        *existing = approval;
        return true;
    }
    let sequence = detail.history_cursor;
    detail.history_cursor = detail.history_cursor.wrapping_add(1);
    detail
        .timeline
        .push(AgentTimelineEntry::ApprovalRequest { sequence, approval });
    true
}

fn recompute_pending_approval_id(detail: &mut AgentSessionDetail) {
    detail.pending_approval_id = detail.timeline.iter().rev().find_map(|entry| match entry {
        AgentTimelineEntry::ApprovalRequest { approval, .. }
            if approval.state == AgentApprovalState::Pending =>
        {
            Some(approval.id.clone())
        }
        _ => None,
    });
}

fn is_terminal_lifecycle(lifecycle: AgentLifecycle) -> bool {
    matches!(
        lifecycle,
        AgentLifecycle::Completed | AgentLifecycle::Failed | AgentLifecycle::Cancelled
    )
}

fn now_ms() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axis_core::agent::AgentTransportKind;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use crate::adapters::fake::FakeProvider;
    use crate::provider::ProviderRegistry;

    #[test]
    fn revision_unchanged_when_lifecycle_transition_is_noop() {
        let mut reg = ProviderRegistry::new();
        reg.register("fake", Arc::new(FakeProvider::with_standard_script()));
        let mut mgr = SessionManager::new(reg);
        let id = mgr
            .start_session(StartAgentRequest {
                cwd: "/tmp".into(),
                provider_profile_id: "fake".into(),
                transport: AgentTransportKind::CliWrapped,
                argv_suffix: vec![],
                env: BTreeMap::new(),
            })
            .unwrap();
        let after_start = mgr.revision();
        mgr.transition_lifecycle(&id, AgentLifecycle::Planned)
            .unwrap();
        assert_eq!(mgr.revision(), after_start);
    }
}
