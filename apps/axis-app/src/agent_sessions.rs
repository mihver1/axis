//! Maps workdesks and surfaces to `axis-agent-runtime` sessions for agent panes.

use crate::daemon_client::DaemonClient;
use crate::remote_terminals::RemoteTerminalSession;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

use axis_agent_runtime::adapters::codex::CodexProvider;
use axis_agent_runtime::adapters::process_only::ProcessOnlyProvider;
use axis_agent_runtime::{
    resolve_provider_command_from_env_or_default, resolve_provider_command_from_env_or_default_for_cwd,
    ProviderProfileMetadata, ProviderRegistry, SessionManager, StartAgentRequest,
};
use axis_core::agent::{AgentAttention, AgentSessionId, AgentSessionRecord, AgentTransportKind};
use axis_core::workdesk::WorkdeskId;
use axis_core::worktree::WorktreeId;
use axis_core::SurfaceId;
use axis_terminal::TerminalAgentMetadata;

const CODEX_PROFILE_ID: &str = "codex";
const CLAUDE_CODE_PROFILE_ID: &str = "claude-code";
const CLAUDE_CODE_CAPABILITY_NOTE: &str = "basic lifecycle only";
const CODEX_BIN_ENV: &str = "AXIS_CODEX_BIN";
const CLAUDE_CODE_BIN_ENV: &str = "AXIS_CLAUDE_CODE_BIN";

/// UI-facing snapshot of a registered provider profile and whether its CLI appears launchable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderProfileOption {
    pub profile_id: String,
    pub capability_note: Option<String>,
    pub available: bool,
    pub unavailable_reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProviderOptionCommandSource {
    env_name: &'static str,
    default_binary: &'static str,
}

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
    daemon: DaemonClient,
    daemon_records: HashMap<AgentSessionId, AgentSessionRecord>,
    daemon_revision: u64,
    desk_cwd: HashMap<u64, String>,
    surface_to_session: HashMap<SurfaceRuntimeKey, AgentSessionId>,
    provider_options: Vec<ProviderProfileOption>,
    provider_option_command_sources: HashMap<String, ProviderOptionCommandSource>,
}

impl AgentRuntimeBridge {
    pub fn new() -> Self {
        let mut registry = ProviderRegistry::new();
        let codex_resolution =
            resolve_provider_command_from_env_or_default(CODEX_BIN_ENV, CODEX_PROFILE_ID);
        let claude_resolution = resolve_provider_command_from_env_or_default(
            CLAUDE_CODE_BIN_ENV,
            CLAUDE_CODE_PROFILE_ID,
        );
        registry.register_with_metadata(
            CODEX_PROFILE_ID,
            std::sync::Arc::new(CodexProvider::with_base_argv(codex_resolution.argv.clone())),
            None::<String>,
        );
        registry.register_with_metadata(
            CLAUDE_CODE_PROFILE_ID,
            std::sync::Arc::new(ProcessOnlyProvider::with_base_argv(
                CLAUDE_CODE_PROFILE_ID,
                claude_resolution.argv.clone(),
            )),
            Some(CLAUDE_CODE_CAPABILITY_NOTE),
        );
        let provider_options = vec![
            ProviderProfileOption {
                profile_id: CODEX_PROFILE_ID.to_string(),
                capability_note: None,
                available: codex_resolution.available,
                unavailable_reason: codex_resolution.unavailable_reason.clone(),
            },
            ProviderProfileOption {
                profile_id: CLAUDE_CODE_PROFILE_ID.to_string(),
                capability_note: Some(CLAUDE_CODE_CAPABILITY_NOTE.to_string()),
                available: claude_resolution.available,
                unavailable_reason: claude_resolution.unavailable_reason.clone(),
            },
        ];
        let provider_option_command_sources = HashMap::from([
            (
                CODEX_PROFILE_ID.to_string(),
                ProviderOptionCommandSource {
                    env_name: CODEX_BIN_ENV,
                    default_binary: CODEX_PROFILE_ID,
                },
            ),
            (
                CLAUDE_CODE_PROFILE_ID.to_string(),
                ProviderOptionCommandSource {
                    env_name: CLAUDE_CODE_BIN_ENV,
                    default_binary: CLAUDE_CODE_PROFILE_ID,
                },
            ),
        ]);
        Self::with_inner(
            CODEX_PROFILE_ID.to_string(),
            registry,
            provider_options,
            provider_option_command_sources,
        )
    }

    fn with_inner(
        default_profile_id: String,
        registry: ProviderRegistry,
        provider_options: Vec<ProviderProfileOption>,
        provider_option_command_sources: HashMap<String, ProviderOptionCommandSource>,
    ) -> Self {
        Self {
            inner: Mutex::new(BridgeInner {
                default_profile_id,
                manager: SessionManager::new(registry),
                daemon: DaemonClient::default(),
                daemon_records: HashMap::new(),
                daemon_revision: 0,
                desk_cwd: HashMap::new(),
                surface_to_session: HashMap::new(),
                provider_options,
                provider_option_command_sources,
            }),
        }
    }

    #[cfg(test)]
    fn provider_options_from_registry(registry: &ProviderRegistry) -> Vec<ProviderProfileOption> {
        registry
            .profiles()
            .into_iter()
            .map(|profile| ProviderProfileOption {
                profile_id: profile.profile_id,
                capability_note: profile.capability_note,
                available: true,
                unavailable_reason: None,
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn with_registry(
        default_profile_id: impl Into<String>,
        registry: ProviderRegistry,
    ) -> Self {
        let provider_options = Self::provider_options_from_registry(&registry);
        Self::with_inner(
            default_profile_id.into(),
            registry,
            provider_options,
            HashMap::new(),
        )
    }

    /// Returns a snapshot of known provider profiles and CLI availability for UI (e.g. picker).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn provider_options(&self) -> Vec<ProviderProfileOption> {
        self.inner
            .lock()
            .map(|g| g.provider_options.clone())
            .unwrap_or_default()
    }

    pub fn provider_options_for_cwd(&self, cwd: &str) -> Vec<ProviderProfileOption> {
        let Ok(guard) = self.inner.lock() else {
            return Vec::new();
        };
        let selected_cwd = (!cwd.trim().is_empty()).then(|| Path::new(cwd));
        guard
            .provider_options
            .iter()
            .cloned()
            .map(|mut option| {
                let Some(command_source) = guard
                    .provider_option_command_sources
                    .get(option.profile_id.as_str())
                else {
                    return option;
                };
                let resolution = resolve_provider_command_from_env_or_default_for_cwd(
                    command_source.env_name,
                    command_source.default_binary,
                    selected_cwd,
                );
                option.available = resolution.available;
                option.unavailable_reason = resolution.unavailable_reason;
                option
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn with_registry_and_options(
        default_profile_id: impl Into<String>,
        registry: ProviderRegistry,
        provider_options: Vec<ProviderProfileOption>,
    ) -> Self {
        Self::with_inner(
            default_profile_id.into(),
            registry,
            provider_options,
            HashMap::new(),
        )
    }

    pub fn revision(&self) -> u64 {
        self.inner
            .lock()
            .map(|g| g.manager.revision().max(g.daemon_revision))
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
        self.inner.lock().ok().is_some_and(|g| {
            g.surface_to_session
                .contains_key(&Self::key(workdesk_runtime_id, surface_id))
        })
    }

    fn start_agent_for_surface_inner(
        &self,
        workdesk_runtime_id: u64,
        workdesk_id: &str,
        surface_id: SurfaceId,
        cwd_fallback: &str,
        terminal: &RemoteTerminalSession,
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
        let key = Self::key(workdesk_runtime_id, surface_id);
        if let Ok(record) = guard.daemon.start_agent(
            &WorktreeId::new(cwd.clone()),
            provider_profile_id.clone(),
            argv_suffix.clone(),
            Some(WorkdeskId::new(workdesk_id)),
            Some(surface_id),
        ) {
            let id = record.id.clone();
            guard.surface_to_session.insert(key, id.clone());
            guard.daemon_records.insert(id.clone(), record);
            guard.daemon_revision = guard.daemon_revision.wrapping_add(1);
            terminal.set_agent_metadata(Some(TerminalAgentMetadata {
                session_id: id.clone(),
            }));
            return Ok(id);
        }

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
        guard.surface_to_session.insert(key, id.clone());
        terminal.set_agent_metadata(Some(TerminalAgentMetadata {
            session_id: id.clone(),
        }));
        Ok(id)
    }

    /// Start a provider session for a new agent surface; attaches metadata to the terminal session.
    pub fn start_agent_for_surface(
        &self,
        workdesk_runtime_id: u64,
        workdesk_id: &str,
        surface_id: SurfaceId,
        cwd_fallback: &str,
        terminal: &RemoteTerminalSession,
    ) -> Result<AgentSessionId, String> {
        let default_profile_id = self
            .inner
            .lock()
            .map_err(|e| format!("agent runtime lock poisoned: {e}"))?
            .default_profile_id
            .clone();
        self.start_agent_for_surface_inner(
            workdesk_runtime_id,
            workdesk_id,
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
        workdesk_id: &str,
        surface_id: SurfaceId,
        cwd_fallback: &str,
        terminal: &RemoteTerminalSession,
        provider_profile_id: &str,
        argv_suffix: Vec<String>,
    ) -> Result<AgentSessionId, String> {
        self.start_agent_for_surface_inner(
            workdesk_runtime_id,
            workdesk_id,
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
        guard
            .daemon_records
            .get(sid)
            .map(|record| record.attention)
            .or_else(|| guard.manager.session(sid).map(|r| r.attention))
    }

    fn record_for_key(guard: &BridgeInner, key: SurfaceRuntimeKey) -> Option<AgentSessionRecord> {
        let sid = guard.surface_to_session.get(&key)?;
        if let Some(record) = guard.daemon_records.get(sid) {
            return Some(record.clone());
        }
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
        let mut sessions = guard
            .surface_to_session
            .keys()
            .copied()
            .filter_map(|key| Self::record_for_key(&guard, key))
            .map(|record| (record.id.clone(), record))
            .collect::<HashMap<_, _>>();
        for record in guard.daemon_records.values() {
            sessions
                .entry(record.id.clone())
                .or_insert_with(|| record.clone());
        }
        sessions.into_values().collect()
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
        if guard.daemon_records.contains_key(&sid) {
            let sessions = guard.daemon.list_agents(None)?;
            guard.daemon_records = sessions
                .into_iter()
                .map(|record| (record.id.clone(), record))
                .collect();
            let daemon_ids = guard.daemon_records.keys().cloned().collect::<HashSet<_>>();
            let local_ids = guard
                .manager
                .sessions()
                .map(|record| record.id.clone())
                .collect::<HashSet<_>>();
            guard.surface_to_session.retain(|_, existing| {
                daemon_ids.contains(existing) || local_ids.contains(existing)
            });
            guard.daemon_revision = guard.daemon_revision.wrapping_add(1);
            return Ok(());
        }
        guard.manager.poll_provider(&sid).map_err(|e| e.to_string())
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
        if guard.daemon_records.contains_key(session_id) {
            guard.daemon.stop_agent(session_id)?;
            guard.daemon_records.remove(session_id);
            if let Some(key) = matching_key {
                guard.surface_to_session.remove(&key);
            }
            guard.daemon_revision = guard.daemon_revision.wrapping_add(1);
            return Ok(());
        }
        if guard.manager.session(session_id).is_none() {
            guard.daemon.stop_agent(session_id)?;
            guard.daemon_records.remove(session_id);
            if let Some(key) = matching_key {
                guard.surface_to_session.remove(&key);
            }
            guard.daemon_revision = guard.daemon_revision.wrapping_add(1);
            return Ok(());
        }
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
        if guard.daemon_records.contains_key(&sid) {
            let _ = guard.daemon.stop_agent(&sid);
            guard.daemon_records.remove(&sid);
            guard.daemon_revision = guard.daemon_revision.wrapping_add(1);
            return;
        }
        if guard.manager.session(&sid).is_none() && guard.daemon.stop_agent(&sid).is_ok() {
            guard.daemon_records.remove(&sid);
            guard.daemon_revision = guard.daemon_revision.wrapping_add(1);
            return;
        }
        let _ = guard.manager.stop_session(&sid);
    }
}

impl Default for AgentRuntimeBridge {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axis_agent_runtime::adapters::fake::FakeProvider;
    use std::ffi::OsString;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    #[test]
    fn provider_options_include_profiles_registered_via_with_registry() {
        let mut registry = ProviderRegistry::new();
        registry.register_with_metadata(
            "alpha",
            Arc::new(FakeProvider::with_standard_script()),
            Some("alpha note"),
        );
        registry.register(
            "beta",
            Arc::new(FakeProvider::with_standard_script()),
        );

        let bridge = AgentRuntimeBridge::with_registry("alpha", registry);

        assert_eq!(
            bridge.provider_options(),
            vec![
                ProviderProfileOption {
                    profile_id: "alpha".to_string(),
                    capability_note: Some("alpha note".to_string()),
                    available: true,
                    unavailable_reason: None,
                },
                ProviderProfileOption {
                    profile_id: "beta".to_string(),
                    capability_note: None,
                    available: true,
                    unavailable_reason: None,
                },
            ]
        );
    }

    #[test]
    fn provider_options_keep_unavailable_profiles_visible() {
        let mut registry = ProviderRegistry::new();
        registry.register(
            "p1",
            Arc::new(FakeProvider::with_standard_script()),
        );
        registry.register(
            "p2",
            Arc::new(FakeProvider::with_standard_script()),
        );
        let provider_options = vec![
            ProviderProfileOption {
                profile_id: "p1".to_string(),
                capability_note: None,
                available: false,
                unavailable_reason: Some("not installed".to_string()),
            },
            ProviderProfileOption {
                profile_id: "p2".to_string(),
                capability_note: Some("note".to_string()),
                available: false,
                unavailable_reason: Some("missing".to_string()),
            },
        ];
        let bridge =
            AgentRuntimeBridge::with_registry_and_options("p1", registry, provider_options.clone());
        assert_eq!(bridge.provider_options(), provider_options);
    }

    #[test]
    fn provider_options_for_cwd_recompute_relative_override_paths_per_desk() {
        let available_dir = temp_dir("provider-options-available");
        let missing_dir = temp_dir("provider-options-missing");
        let tool_dir = available_dir.join("tools");
        std::fs::create_dir_all(&tool_dir).expect("tool directory should be created");
        create_executable(&tool_dir, "codex");
        let _guard = EnvVarGuard::set(CODEX_BIN_ENV, Some("./tools/codex"));

        let bridge = AgentRuntimeBridge::new();

        let available = bridge
            .provider_options_for_cwd(&available_dir.to_string_lossy())
            .into_iter()
            .find(|option| option.profile_id == CODEX_PROFILE_ID)
            .expect("codex option should exist");
        let missing = bridge
            .provider_options_for_cwd(&missing_dir.to_string_lossy())
            .into_iter()
            .find(|option| option.profile_id == CODEX_PROFILE_ID)
            .expect("codex option should exist");

        assert!(available.available);
        assert_eq!(available.unavailable_reason, None);
        assert!(!missing.available);
        assert_eq!(
            missing.unavailable_reason.as_deref(),
            Some("Configured path is not executable")
        );

        let _ = std::fs::remove_dir_all(available_dir);
        let _ = std::fs::remove_dir_all(missing_dir);
    }

    fn temp_dir(label: &str) -> PathBuf {
        let unique = format!(
            "axis-agent-sessions-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be available")
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&dir).expect("temp dir should be created");
        dir
    }

    fn create_executable(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("script should be written");
        #[cfg(unix)]
        {
            let mut permissions = std::fs::metadata(&path)
                .expect("metadata should load")
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions).expect("permissions should be set");
        }
        path
    }

    struct EnvVarGuard {
        key: &'static str,
        original: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: Option<&str>) -> Self {
            let original = std::env::var_os(key);
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}
