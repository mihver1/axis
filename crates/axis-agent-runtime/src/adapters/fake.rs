//! Deterministic provider that emits a fixed script without subprocesses.

use std::collections::HashMap;
use parking_lot::Mutex;

use axis_core::agent::{AgentAttention, AgentLifecycle, AgentSessionId};
use axis_core::agent_history::{
    AgentApprovalDecision, AgentApprovalKind, AgentApprovalRequest, AgentApprovalState,
    AgentSessionCapabilities, AgentTurn, AgentTurnId, AgentTurnRole, AgentTurnState,
};

use crate::events::RuntimeEvent;
use crate::provider::{
    AgentProvider, RespondApprovalRequest, ResumeRequest, SendTurnRequest, StartAgentRequest,
    StartedSession,
};

#[derive(Clone, Debug, Eq, PartialEq)]
enum ScriptStep {
    Lifecycle(AgentLifecycle),
    Attention(AgentAttention),
}

/// Build a custom [`FakeProvider`] script from integration tests without pulling in internal types.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FakeScriptStep {
    Lifecycle(AgentLifecycle),
    Attention(AgentAttention),
}

impl From<FakeScriptStep> for ScriptStep {
    fn from(step: FakeScriptStep) -> Self {
        match step {
            FakeScriptStep::Lifecycle(l) => ScriptStep::Lifecycle(l),
            FakeScriptStep::Attention(a) => ScriptStep::Attention(a),
        }
    }
}

/// Emits one scripted [`RuntimeEvent`] per [`AgentProvider::poll_events`] until exhausted.
pub struct FakeProvider {
    inner: Mutex<FakeInner>,
}

struct FakeInner {
    next_id: u64,
    template: Vec<ScriptStep>,
    sessions: HashMap<AgentSessionId, FakeSessionState>,
}

struct FakeSessionState {
    cursor: usize,
    steps: Vec<ScriptStep>,
    next_turn_id: u64,
    next_timestamp_ms: u64,
}

impl FakeProvider {
    /// `starting` → `running` → `needs_review` (attention) → `completed`.
    pub fn with_standard_script() -> Self {
        Self::with_script(vec![
            ScriptStep::Lifecycle(AgentLifecycle::Starting),
            ScriptStep::Lifecycle(AgentLifecycle::Running),
            ScriptStep::Attention(AgentAttention::NeedsReview),
            ScriptStep::Lifecycle(AgentLifecycle::Completed),
        ])
    }

    /// Custom event sequence for tests or embedded demos (same semantics as the standard script builder).
    pub fn with_steps(steps: Vec<FakeScriptStep>) -> Self {
        Self::with_script(steps.into_iter().map(Into::into).collect())
    }

    fn with_script(template: Vec<ScriptStep>) -> Self {
        Self {
            inner: Mutex::new(FakeInner {
                next_id: 1,
                template,
                sessions: HashMap::new(),
            }),
        }
    }
}

impl AgentProvider for FakeProvider {
    fn capabilities(&self) -> AgentSessionCapabilities {
        AgentSessionCapabilities {
            turn_input: true,
            tool_calls: true,
            approvals: true,
            resume: true,
            terminal_attachment: false,
        }
    }

    fn start(&self, _req: StartAgentRequest) -> anyhow::Result<StartedSession> {
        let mut g = self.inner.lock();
        let id = AgentSessionId::new(format!("fake-session-{}", g.next_id));
        g.next_id += 1;
        let script = g.template.clone();
        g.sessions.insert(
            id.clone(),
            FakeSessionState {
                cursor: 0,
                steps: script,
                next_turn_id: 1,
                next_timestamp_ms: 1,
            },
        );
        Ok(StartedSession { session_id: id })
    }

    fn poll_events(&self, session_id: &AgentSessionId) -> anyhow::Result<Vec<RuntimeEvent>> {
        let mut g = self.inner.lock();
        let state = g
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| anyhow::anyhow!("unknown fake session {}", session_id.0))?;
        if state.cursor >= state.steps.len() {
            return Ok(vec![]);
        }
        let step = state.steps[state.cursor].clone();
        state.cursor += 1;
        let event = match step {
            ScriptStep::Lifecycle(lifecycle) => RuntimeEvent::Lifecycle {
                session_id: session_id.clone(),
                lifecycle,
            },
            ScriptStep::Attention(attention) => RuntimeEvent::Attention {
                session_id: session_id.clone(),
                attention,
            },
        };
        Ok(vec![event])
    }

    fn send_turn(&self, req: SendTurnRequest) -> anyhow::Result<Vec<RuntimeEvent>> {
        let mut g = self.inner.lock();
        let state = g
            .sessions
            .get_mut(&req.session_id)
            .ok_or_else(|| anyhow::anyhow!("unknown fake session {}", req.session_id.0))?;
        let turn_id = state.next_turn_id;
        state.next_turn_id += 1;
        let created_at_ms = next_fake_timestamp(state);
        let completed_at_ms = next_fake_timestamp(state);
        Ok(vec![
            RuntimeEvent::Turn {
                session_id: req.session_id.clone(),
                turn: AgentTurn {
                    id: AgentTurnId::new(format!("fake-turn-{turn_id}")),
                    role: AgentTurnRole::User,
                    state: AgentTurnState::Completed,
                    text: req.text,
                    created_at_ms,
                    completed_at_ms: Some(completed_at_ms),
                },
            },
            RuntimeEvent::Status {
                session_id: req.session_id,
                message: "turn submitted".to_string(),
            },
        ])
    }

    fn respond_approval(&self, req: RespondApprovalRequest) -> anyhow::Result<Vec<RuntimeEvent>> {
        let mut g = self.inner.lock();
        let state = g
            .sessions
            .get_mut(&req.session_id)
            .ok_or_else(|| anyhow::anyhow!("unknown fake session {}", req.session_id.0))?;
        let requested_at_ms = next_fake_timestamp(state);
        let decided_at_ms = next_fake_timestamp(state);
        Ok(vec![RuntimeEvent::ApprovalRequest {
            session_id: req.session_id,
            approval: AgentApprovalRequest {
                id: req.approval_request_id,
                kind: AgentApprovalKind::Generic,
                title: "Synthetic fake approval".to_string(),
                details: "Deterministic approval emitted by the fake provider.".to_string(),
                state: if req.approved {
                    AgentApprovalState::Approved
                } else {
                    AgentApprovalState::Denied
                },
                tool_call_id: None,
                requested_at_ms,
                decision: Some(AgentApprovalDecision {
                    approved: req.approved,
                    note: req.note,
                    decided_at_ms,
                }),
            },
        }])
    }

    fn resume(&self, req: ResumeRequest) -> anyhow::Result<Vec<RuntimeEvent>> {
        let g = self.inner.lock();
        if !g.sessions.contains_key(&req.session_id) {
            return Err(anyhow::anyhow!("unknown fake session {}", req.session_id.0));
        }
        Ok(vec![
            RuntimeEvent::Lifecycle {
                session_id: req.session_id.clone(),
                lifecycle: AgentLifecycle::Running,
            },
            RuntimeEvent::Attention {
                session_id: req.session_id.clone(),
                attention: AgentAttention::Working,
            },
            RuntimeEvent::Status {
                session_id: req.session_id,
                message: "resumed".to_string(),
            },
        ])
    }

    fn stop(&self, session_id: &AgentSessionId) -> anyhow::Result<()> {
        let mut g = self.inner.lock();
        g.sessions
            .remove(session_id)
            .ok_or_else(|| anyhow::anyhow!("unknown fake session {}", session_id.0))?;
        Ok(())
    }
}

fn next_fake_timestamp(state: &mut FakeSessionState) -> u64 {
    let next = state.next_timestamp_ms;
    state.next_timestamp_ms = state.next_timestamp_ms.wrapping_add(1);
    next
}
