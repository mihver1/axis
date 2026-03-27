use std::collections::HashMap;

use axis_agent_runtime::adapters::codex::CodexProvider;
use axis_agent_runtime::adapters::process_only::ProcessOnlyProvider;
use axis_agent_runtime::{
    provider_base_argv_from_env_or_default, ProviderProfileMetadata, ProviderRegistry,
    SessionManager, StartAgentRequest,
};
use axis_core::agent::{AgentSessionId, AgentSessionRecord, AgentTransportKind};
use axis_core::workdesk::WorkdeskId;
use axis_core::SurfaceId;

const CODEX_PROFILE_ID: &str = "codex";
const CLAUDE_CODE_PROFILE_ID: &str = "claude-code";
const CLAUDE_CODE_CAPABILITY_NOTE: &str = "basic lifecycle only";
const CODEX_BIN_ENV: &str = "AXIS_CODEX_BIN";
const CLAUDE_CODE_BIN_ENV: &str = "AXIS_CLAUDE_CODE_BIN";

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct AgentSurfaceKey {
    workdesk_id: String,
    surface_id: SurfaceId,
}

#[derive(Clone, Debug, Default)]
struct SessionBinding {
    workdesk_id: Option<String>,
    surface_id: Option<SurfaceId>,
}

pub struct DaemonAgentRuntime {
    default_profile_id: String,
    manager: SessionManager,
    surface_to_session: HashMap<AgentSurfaceKey, AgentSessionId>,
    session_bindings: HashMap<AgentSessionId, SessionBinding>,
}

impl DaemonAgentRuntime {
    pub fn new() -> Self {
        let mut registry = ProviderRegistry::new();
        let codex_base_argv =
            provider_base_argv_from_env_or_default(CODEX_BIN_ENV, CODEX_PROFILE_ID);
        registry.register_with_metadata(
            CODEX_PROFILE_ID,
            std::sync::Arc::new(CodexProvider::with_base_argv(codex_base_argv)),
            None::<String>,
        );
        let claude_base_argv =
            provider_base_argv_from_env_or_default(CLAUDE_CODE_BIN_ENV, CLAUDE_CODE_PROFILE_ID);
        registry.register_with_metadata(
            CLAUDE_CODE_PROFILE_ID,
            std::sync::Arc::new(ProcessOnlyProvider::with_base_argv(
                CLAUDE_CODE_PROFILE_ID,
                claude_base_argv,
            )),
            Some(CLAUDE_CODE_CAPABILITY_NOTE),
        );

        Self {
            default_profile_id: CODEX_PROFILE_ID.to_string(),
            manager: SessionManager::new(registry),
            surface_to_session: HashMap::new(),
            session_bindings: HashMap::new(),
        }
    }

    #[allow(dead_code)]
    pub fn revision(&self) -> u64 {
        self.manager.revision()
    }

    pub fn provider_profile(&self, profile_id: &str) -> Option<ProviderProfileMetadata> {
        self.manager.provider_profile(profile_id)
    }

    pub fn start_session(
        &mut self,
        cwd: String,
        provider_profile_id: Option<String>,
        argv_suffix: Vec<String>,
        workdesk_id: Option<WorkdeskId>,
        surface_id: Option<SurfaceId>,
    ) -> Result<AgentSessionRecord, String> {
        let cwd = cwd.trim().to_string();
        if cwd.is_empty() {
            return Err("agent session requires non-empty cwd".to_string());
        }

        let provider_profile_id =
            provider_profile_id.unwrap_or_else(|| self.default_profile_id.clone());
        let binding = SessionBinding {
            workdesk_id: workdesk_id.clone().map(|id| id.0),
            surface_id,
        };

        if let (Some(bound_workdesk_id), Some(bound_surface_id)) =
            (binding.workdesk_id.clone(), binding.surface_id)
        {
            let key = AgentSurfaceKey {
                workdesk_id: bound_workdesk_id,
                surface_id: bound_surface_id,
            };
            if let Some(existing_id) = self.surface_to_session.get(&key) {
                let existing = self
                    .record_for_session(existing_id)
                    .ok_or_else(|| "agent session metadata disappeared".to_string())?;
                if existing.provider_profile_id != provider_profile_id {
                    return Err(format!(
                        "agent surface already runs `{}`",
                        existing.provider_profile_id
                    ));
                }
                return Ok(existing);
            }
        }

        let id = self
            .manager
            .start_session(StartAgentRequest {
                cwd,
                provider_profile_id,
                transport: AgentTransportKind::CliWrapped,
                argv_suffix,
                env: Default::default(),
            })
            .map_err(|error| error.to_string())?;

        if let (Some(bound_workdesk_id), Some(bound_surface_id)) =
            (binding.workdesk_id.clone(), binding.surface_id)
        {
            self.surface_to_session.insert(
                AgentSurfaceKey {
                    workdesk_id: bound_workdesk_id,
                    surface_id: bound_surface_id,
                },
                id.clone(),
            );
        }
        self.session_bindings.insert(id.clone(), binding);
        self.record_for_session(&id)
            .ok_or_else(|| "agent session did not register".to_string())
    }

    pub fn session_for_surface(
        &self,
        workdesk_id: &str,
        surface_id: SurfaceId,
    ) -> Option<AgentSessionRecord> {
        let key = AgentSurfaceKey {
            workdesk_id: workdesk_id.to_string(),
            surface_id,
        };
        let session_id = self.surface_to_session.get(&key)?;
        self.record_for_session(session_id)
    }

    pub fn sessions_snapshot(&self) -> Vec<AgentSessionRecord> {
        self.manager
            .sessions()
            .filter_map(|record| self.record_for_session(&record.id))
            .collect()
    }

    pub fn poll_all(&mut self) -> Result<(), String> {
        let session_ids = self
            .manager
            .sessions()
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();
        for session_id in session_ids {
            self.manager
                .poll_provider(&session_id)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub fn poll_surface(&mut self, workdesk_id: &str, surface_id: SurfaceId) -> Result<(), String> {
        let key = AgentSurfaceKey {
            workdesk_id: workdesk_id.to_string(),
            surface_id,
        };
        let Some(session_id) = self.surface_to_session.get(&key).cloned() else {
            return Ok(());
        };
        self.manager
            .poll_provider(&session_id)
            .map_err(|error| error.to_string())
    }

    pub fn stop_session(&mut self, session_id: &AgentSessionId) -> Result<(), String> {
        let matching_key = self
            .surface_to_session
            .iter()
            .find_map(|(key, existing)| (existing == session_id).then_some(key.clone()));
        self.manager
            .stop_session(session_id)
            .map_err(|error| error.to_string())?;
        self.session_bindings.remove(session_id);
        if let Some(key) = matching_key {
            self.surface_to_session.remove(&key);
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub fn stop_surface(&mut self, workdesk_id: &str, surface_id: SurfaceId) {
        let key = AgentSurfaceKey {
            workdesk_id: workdesk_id.to_string(),
            surface_id,
        };
        let Some(session_id) = self.surface_to_session.remove(&key) else {
            return;
        };
        let _ = self.manager.stop_session(&session_id);
        self.session_bindings.remove(&session_id);
    }

    fn record_for_session(&self, session_id: &AgentSessionId) -> Option<AgentSessionRecord> {
        let mut record = self.manager.session(session_id)?.clone();
        if let Some(binding) = self.session_bindings.get(session_id) {
            record.workdesk_id = binding.workdesk_id.clone();
            record.surface_id = binding.surface_id;
        }
        Some(record)
    }
}

impl Default for DaemonAgentRuntime {
    fn default() -> Self {
        Self::new()
    }
}
