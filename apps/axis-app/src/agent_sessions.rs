//! Maps workdesks and surfaces to `axis-agent-runtime` sessions for agent panes.

use std::collections::HashMap;
use std::sync::Mutex;

use axis_agent_runtime::adapters::codex::CodexProvider;
use axis_agent_runtime::adapters::process_only::ProcessOnlyProvider;
use axis_agent_runtime::{
    ProviderProfileMetadata, ProviderRegistry, SessionManager, StartAgentRequest,
};
use axis_core::agent::{AgentAttention, AgentSessionId, AgentSessionRecord, AgentTransportKind};
use axis_core::SurfaceId;
use axis_terminal::{TerminalAgentMetadata, TerminalSession};

const CODEX_PROFILE_ID: &str = "codex";
const CLAUDE_CODE_PROFILE_ID: &str = "claude-code";
const CLAUDE_CODE_CAPABILITY_NOTE: &str = "basic lifecycle only";
const CODEX_BIN_ENV: &str = "AXIS_CODEX_BIN";
const CLAUDE_CODE_BIN_ENV: &str = "AXIS_CLAUDE_CODE_BIN";

/// Shared runtime: one [`SessionManager`], codex provider, desk cwd hints, surface → session map.
pub struct AgentRuntimeBridge {
    inner: Mutex<BridgeInner>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct SurfaceRuntimeKey {
    workdesk_runtime_id: u64,
    surface_id: SurfaceId,
}

struct BridgeInner {
    default_profile_id: String,
    manager: SessionManager,
    desk_cwd: HashMap<u64, String>,
    surface_to_session: HashMap<SurfaceRuntimeKey, AgentSessionId>,
}

impl AgentRuntimeBridge {
    pub fn new() -> Self {
        let mut registry = ProviderRegistry::new();
        let codex_base_argv = provider_base_argv_from_bin_override(
            provider_bin_override(CODEX_BIN_ENV).as_deref(),
            CODEX_PROFILE_ID,
        );
        registry.register_with_metadata(
            CODEX_PROFILE_ID,
            std::sync::Arc::new(CodexProvider::with_base_argv(codex_base_argv)),
            None::<String>,
        );
        let claude_base_argv = provider_base_argv_from_bin_override(
            provider_bin_override(CLAUDE_CODE_BIN_ENV).as_deref(),
            CLAUDE_CODE_PROFILE_ID,
        );
        registry.register_with_metadata(
            CLAUDE_CODE_PROFILE_ID,
            std::sync::Arc::new(ProcessOnlyProvider::with_base_argv(
                CLAUDE_CODE_PROFILE_ID,
                claude_base_argv,
            )),
            Some(CLAUDE_CODE_CAPABILITY_NOTE),
        );
        Self::with_registry(CODEX_PROFILE_ID, registry)
    }

    pub(crate) fn with_registry(
        default_profile_id: impl Into<String>,
        registry: ProviderRegistry,
    ) -> Self {
        Self {
            inner: Mutex::new(BridgeInner {
                default_profile_id: default_profile_id.into(),
                manager: SessionManager::new(registry),
                desk_cwd: HashMap::new(),
                surface_to_session: HashMap::new(),
            }),
        }
    }

    pub fn revision(&self) -> u64 {
        self.inner
            .lock()
            .map(|g| g.manager.revision())
            .unwrap_or(0)
    }

    fn key(workdesk_runtime_id: u64, surface_id: SurfaceId) -> SurfaceRuntimeKey {
        SurfaceRuntimeKey {
            workdesk_runtime_id,
            surface_id,
        }
    }

    pub fn set_desk_cwd(&self, workdesk_runtime_id: u64, cwd: String) {
        if let Ok(mut g) = self.inner.lock() {
            g.desk_cwd.insert(workdesk_runtime_id, cwd);
        }
    }

    fn resolve_cwd(&self, workdesk_runtime_id: u64, fallback: &str) -> String {
        let g = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => return fallback.to_string(),
        };
        g.desk_cwd
            .get(&workdesk_runtime_id)
            .cloned()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| fallback.to_string())
    }

    pub(crate) fn has_session_for_surface(
        &self,
        workdesk_runtime_id: u64,
        surface_id: SurfaceId,
    ) -> bool {
        self.inner
            .lock()
            .ok()
            .is_some_and(|g| {
                g.surface_to_session
                    .contains_key(&Self::key(workdesk_runtime_id, surface_id))
            })
    }

    fn start_agent_for_surface_inner(
        &self,
        workdesk_runtime_id: u64,
        surface_id: SurfaceId,
        cwd_fallback: &str,
        terminal: &TerminalSession,
        provider_profile_id: String,
        argv_suffix: Vec<String>,
    ) -> Result<AgentSessionId, String> {
        let cwd = self.resolve_cwd(workdesk_runtime_id, cwd_fallback);
        let cwd = cwd.trim().to_string();
        if cwd.is_empty() {
            return Err("agent session requires non-empty cwd".to_string());
        }
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| format!("agent runtime lock poisoned: {e}"))?;
        let req = StartAgentRequest {
            cwd,
            provider_profile_id,
            transport: AgentTransportKind::CliWrapped,
            argv_suffix,
            env: Default::default(),
        };
        let id = guard
            .manager
            .start_session(req)
            .map_err(|e| e.to_string())?;
        guard
            .surface_to_session
            .insert(Self::key(workdesk_runtime_id, surface_id), id.clone());
        terminal.set_agent_metadata(Some(TerminalAgentMetadata {
            session_id: id.clone(),
        }));
        Ok(id)
    }

    /// Start a provider session for a new agent surface; attaches metadata to the terminal session.
    pub fn start_agent_for_surface(
        &self,
        workdesk_runtime_id: u64,
        surface_id: SurfaceId,
        cwd_fallback: &str,
        terminal: &TerminalSession,
    ) -> Result<AgentSessionId, String> {
        let default_profile_id = self
            .inner
            .lock()
            .map_err(|e| format!("agent runtime lock poisoned: {e}"))?
            .default_profile_id
            .clone();
        self.start_agent_for_surface_inner(
            workdesk_runtime_id,
            surface_id,
            cwd_fallback,
            terminal,
            default_profile_id,
            vec![],
        )
    }

    pub(crate) fn start_agent_for_surface_with_profile(
        &self,
        workdesk_runtime_id: u64,
        surface_id: SurfaceId,
        cwd_fallback: &str,
        terminal: &TerminalSession,
        provider_profile_id: &str,
        argv_suffix: Vec<String>,
    ) -> Result<AgentSessionId, String> {
        self.start_agent_for_surface_inner(
            workdesk_runtime_id,
            surface_id,
            cwd_fallback,
            terminal,
            provider_profile_id.to_string(),
            argv_suffix,
        )
    }

    pub fn attention_for_surface(
        &self,
        workdesk_runtime_id: u64,
        surface_id: SurfaceId,
    ) -> Option<AgentAttention> {
        let guard = self.inner.lock().ok()?;
        let sid = guard
            .surface_to_session
            .get(&Self::key(workdesk_runtime_id, surface_id))?;
        guard.manager.session(sid).map(|r| r.attention)
    }

    fn record_for_key(guard: &BridgeInner, key: SurfaceRuntimeKey) -> Option<AgentSessionRecord> {
        let sid = guard.surface_to_session.get(&key)?;
        let mut record = guard.manager.session(sid)?.clone();
        record.workdesk_id = Some(key.workdesk_runtime_id.to_string());
        record.surface_id = Some(key.surface_id);
        Some(record)
    }

    pub(crate) fn session_for_surface(
        &self,
        workdesk_runtime_id: u64,
        surface_id: SurfaceId,
    ) -> Option<AgentSessionRecord> {
        let guard = self.inner.lock().ok()?;
        Self::record_for_key(&guard, Self::key(workdesk_runtime_id, surface_id))
    }

    pub(crate) fn sessions_snapshot(&self) -> Vec<AgentSessionRecord> {
        let Ok(guard) = self.inner.lock() else {
            return Vec::new();
        };
        guard
            .surface_to_session
            .keys()
            .copied()
            .filter_map(|key| Self::record_for_key(&guard, key))
            .collect()
    }

    pub(crate) fn provider_profile(&self, profile_id: &str) -> Option<ProviderProfileMetadata> {
        self.inner
            .lock()
            .ok()
            .and_then(|guard| guard.manager.provider_profile(profile_id))
    }

    pub fn poll_surface(
        &self,
        workdesk_runtime_id: u64,
        surface_id: SurfaceId,
    ) -> Result<(), String> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| format!("agent runtime lock poisoned: {e}"))?;
        let Some(sid) = guard
            .surface_to_session
            .get(&Self::key(workdesk_runtime_id, surface_id))
            .cloned()
        else {
            return Ok(());
        };
        guard
            .manager
            .poll_provider(&sid)
            .map_err(|e| e.to_string())
    }

    pub(crate) fn stop_session(&self, session_id: &AgentSessionId) -> Result<(), String> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| format!("agent runtime lock poisoned: {e}"))?;
        let matching_key = guard
            .surface_to_session
            .iter()
            .find_map(|(key, existing)| (existing == session_id).then_some(*key));
        guard
            .manager
            .stop_session(session_id)
            .map_err(|e| e.to_string())?;
        if let Some(key) = matching_key {
            guard.surface_to_session.remove(&key);
        }
        Ok(())
    }

    pub fn stop_surface(&self, workdesk_runtime_id: u64, surface_id: SurfaceId) {
        let Ok(mut guard) = self.inner.lock() else {
            return;
        };
        let Some(sid) = guard
            .surface_to_session
            .remove(&Self::key(workdesk_runtime_id, surface_id))
        else {
            return;
        };
        let _ = guard.manager.stop_session(&sid);
    }
}

impl Default for AgentRuntimeBridge {
    fn default() -> Self {
        Self::new()
    }
}

fn provider_bin_override(env_name: &str) -> Option<String> {
    std::env::var(env_name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn provider_base_argv_from_bin_override(
    bin_override: Option<&str>,
    default_binary: &str,
) -> Vec<String> {
    vec![bin_override.unwrap_or(default_binary).to_string()]
}

#[cfg(test)]
mod tests {
    use super::provider_base_argv_from_bin_override;

    #[test]
    fn provider_base_argv_prefers_override_binary() {
        assert_eq!(
            provider_base_argv_from_bin_override(Some("/tmp/codex-demo"), "codex"),
            vec!["/tmp/codex-demo".to_string()]
        );
    }

    #[test]
    fn provider_base_argv_falls_back_to_default_binary() {
        assert_eq!(
            provider_base_argv_from_bin_override(None, "claude-code"),
            vec!["claude-code".to_string()]
        );
    }
}
