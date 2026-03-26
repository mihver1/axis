//! Deterministic provider that emits a fixed script without subprocesses.

use std::collections::HashMap;
use std::sync::Mutex;

use axis_core::agent::{AgentAttention, AgentLifecycle, AgentSessionId};

use crate::events::RuntimeEvent;
use crate::provider::{AgentProvider, StartAgentRequest, StartedSession};

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
    sessions: HashMap<AgentSessionId, (usize, Vec<ScriptStep>)>,
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
    fn start(&self, _req: StartAgentRequest) -> anyhow::Result<StartedSession> {
        let mut g = self
            .inner
            .lock()
            .map_err(|e| anyhow::anyhow!("fake provider lock poisoned: {e}"))?;
        let id = AgentSessionId::new(format!("fake-session-{}", g.next_id));
        g.next_id += 1;
        let script = g.template.clone();
        g.sessions.insert(id.clone(), (0, script));
        Ok(StartedSession { session_id: id })
    }

    fn poll_events(&self, session_id: &AgentSessionId) -> anyhow::Result<Vec<RuntimeEvent>> {
        let mut g = self
            .inner
            .lock()
            .map_err(|e| anyhow::anyhow!("fake provider lock poisoned: {e}"))?;
        let (cursor, steps) = g
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| anyhow::anyhow!("unknown fake session {}", session_id.0))?;
        if *cursor >= steps.len() {
            return Ok(vec![]);
        }
        let step = steps[*cursor].clone();
        *cursor += 1;
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

    fn stop(&self, session_id: &AgentSessionId) -> anyhow::Result<()> {
        let mut g = self
            .inner
            .lock()
            .map_err(|e| anyhow::anyhow!("fake provider lock poisoned: {e}"))?;
        g.sessions
            .remove(session_id)
            .ok_or_else(|| anyhow::anyhow!("unknown fake session {}", session_id.0))?;
        Ok(())
    }
}
