//! Provider contract for starting agent sessions and polling runtime events.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use anyhow::{anyhow, Context};
use axis_core::agent::{AgentSessionId, AgentTransportKind};

use crate::events::RuntimeEvent;

/// Inputs required to spawn a session through a named provider profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartAgentRequest {
    pub cwd: String,
    pub provider_profile_id: String,
    pub transport: AgentTransportKind,
    /// Extra argv segments after the provider executable (e.g. CLI flags).
    pub argv_suffix: Vec<String>,
    /// Environment entries merged onto the child process environment.
    pub env: BTreeMap<String, String>,
}

/// Handle returned after a provider accepts a start request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartedSession {
    pub session_id: AgentSessionId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderProfileMetadata {
    pub profile_id: String,
    pub capability_note: Option<String>,
}

/// Pluggable agent backend (CLI-wrapped, native ACP, or test doubles).
///
/// **Threading:** [`start`](Self::start), [`poll_events`](Self::poll_events), and [`stop`](Self::stop) may block
/// on I/O or child processes. Hosts should not invoke them directly on a UI thread; use a background task or
/// executor instead.
pub trait AgentProvider: Send + Sync {
    fn start(&self, req: StartAgentRequest) -> anyhow::Result<StartedSession>;

    fn poll_events(&self, session_id: &AgentSessionId) -> anyhow::Result<Vec<RuntimeEvent>>;

    /// Tears down the provider-side session (signals subprocess, closes channels, etc.).
    fn stop(&self, session_id: &AgentSessionId) -> anyhow::Result<()>;
}

/// Maps provider profile ids to provider implementations.
#[derive(Clone, Default)]
pub struct ProviderRegistry {
    providers: HashMap<String, RegisteredProvider>,
}

#[derive(Clone)]
struct RegisteredProvider {
    provider: Arc<dyn AgentProvider>,
    metadata: ProviderProfileMetadata,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, profile_id: impl Into<String>, provider: Arc<dyn AgentProvider>) {
        self.register_with_metadata(profile_id, provider, None::<String>);
    }

    pub fn register_with_metadata(
        &mut self,
        profile_id: impl Into<String>,
        provider: Arc<dyn AgentProvider>,
        capability_note: Option<impl Into<String>>,
    ) {
        let profile_id = profile_id.into();
        self.providers.insert(
            profile_id.clone(),
            RegisteredProvider {
                provider,
                metadata: ProviderProfileMetadata {
                    profile_id,
                    capability_note: capability_note.map(Into::into),
                },
            },
        );
    }

    pub fn get(&self, profile_id: &str) -> Option<Arc<dyn AgentProvider>> {
        self.providers.get(profile_id).map(|entry| entry.provider.clone())
    }

    pub fn metadata(&self, profile_id: &str) -> Option<ProviderProfileMetadata> {
        self.providers
            .get(profile_id)
            .map(|entry| entry.metadata.clone())
    }

    pub fn profiles(&self) -> Vec<ProviderProfileMetadata> {
        let mut profiles = self
            .providers
            .values()
            .map(|entry| entry.metadata.clone())
            .collect::<Vec<_>>();
        profiles.sort_by(|left, right| left.profile_id.cmp(&right.profile_id));
        profiles
    }

    pub fn require(&self, profile_id: &str) -> anyhow::Result<Arc<dyn AgentProvider>> {
        self.get(profile_id)
            .with_context(|| format!("unknown provider profile id {profile_id:?}"))
    }
}

pub(crate) fn validate_start_request(req: &StartAgentRequest) -> anyhow::Result<()> {
    if req.cwd.is_empty() {
        return Err(anyhow!("cwd must not be empty"));
    }
    if req.provider_profile_id.is_empty() {
        return Err(anyhow!("provider_profile_id must not be empty"));
    }
    Ok(())
}
