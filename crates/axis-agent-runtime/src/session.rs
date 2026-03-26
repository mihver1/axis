//! Session registry, lifecycle/attention transitions, and UI revision counter.

use std::collections::HashMap;

use anyhow::{anyhow, Context};
use axis_core::agent::{
    AgentAttention, AgentLifecycle, AgentSessionId, AgentSessionRecord,
};

use crate::events::RuntimeEvent;
use crate::provider::{
    validate_start_request, ProviderProfileMetadata, ProviderRegistry, StartAgentRequest,
};

/// Owns agent session records, provider lookup, and monotonic revision for UI refresh.
pub struct SessionManager {
    sessions: HashMap<AgentSessionId, AgentSessionRecord>,
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
        self.sessions.get(id)
    }

    pub fn sessions(&self) -> impl Iterator<Item = &AgentSessionRecord> {
        self.sessions.values()
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
            return Err(anyhow!(
                "provider returned duplicate session id {}",
                id.0
            ));
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
        self.sessions.insert(id.clone(), record);
        self.bump_revision();
        Ok(id)
    }

    /// Applies provider events, updating lifecycle, attention, and status text.
    pub fn apply_events(
        &mut self,
        events: impl IntoIterator<Item = RuntimeEvent>,
    ) -> anyhow::Result<()> {
        for event in events {
            match event {
                RuntimeEvent::Lifecycle {
                    session_id,
                    lifecycle,
                } => {
                    let record = self
                        .sessions
                        .get_mut(&session_id)
                        .with_context(|| format!("unknown session {}", session_id.0))?;
                    record.lifecycle = lifecycle;
                    self.bump_revision();
                }
                RuntimeEvent::Attention {
                    session_id,
                    attention,
                } => {
                    let record = self
                        .sessions
                        .get_mut(&session_id)
                        .with_context(|| format!("unknown session {}", session_id.0))?;
                    record.attention = attention;
                    self.bump_revision();
                }
                RuntimeEvent::Status {
                    session_id,
                    message,
                } => {
                    let record = self
                        .sessions
                        .get_mut(&session_id)
                        .with_context(|| format!("unknown session {}", session_id.0))?;
                    record.status_message = message;
                    self.bump_revision();
                }
            }
        }
        Ok(())
    }

    /// Polls the provider registered for the session’s profile and applies emitted events.
    pub fn poll_provider(&mut self, session_id: &AgentSessionId) -> anyhow::Result<()> {
        let profile_id = self
            .session(session_id)
            .map(|s| s.provider_profile_id.clone())
            .with_context(|| format!("unknown session {}", session_id.0))?;
        let provider = self.registry.require(&profile_id)?;
        let events = provider.poll_events(session_id)?;
        self.apply_events(events)
    }

    /// Stops the provider-backed session, then drops the local record.
    pub fn stop_session(&mut self, session_id: &AgentSessionId) -> anyhow::Result<()> {
        let profile_id = self
            .session(session_id)
            .map(|s| s.provider_profile_id.clone())
            .with_context(|| format!("unknown session {}", session_id.0))?;
        let provider = self.registry.require(&profile_id)?;
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
        let record = self
            .sessions
            .get_mut(session_id)
            .with_context(|| format!("unknown session {}", session_id.0))?;
        if record.lifecycle == lifecycle {
            return Ok(());
        }
        record.lifecycle = lifecycle;
        self.bump_revision();
        Ok(())
    }

    /// Local attention transition; bumps revision when the state changes.
    pub fn transition_attention(
        &mut self,
        session_id: &AgentSessionId,
        attention: AgentAttention,
    ) -> anyhow::Result<()> {
        let record = self
            .sessions
            .get_mut(session_id)
            .with_context(|| format!("unknown session {}", session_id.0))?;
        if record.attention == attention {
            return Ok(());
        }
        record.attention = attention;
        self.bump_revision();
        Ok(())
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
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
        reg.register(
            "fake",
            Arc::new(FakeProvider::with_standard_script()),
        );
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
