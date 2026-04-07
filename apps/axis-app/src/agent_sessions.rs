//! Maps workdesks and surfaces to `axis-agent-runtime` sessions for agent panes.

use crate::daemon_client::DaemonClient;
use crate::remote_terminals::RemoteTerminalSession;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use parking_lot::Mutex;

use axis_agent_runtime::adapters::codex::CodexProvider;
use axis_agent_runtime::adapters::cursor::CursorProvider;
use axis_agent_runtime::adapters::process_only::ProcessOnlyProvider;
use axis_agent_runtime::{
    resolve_provider_command_from_env_or_default, resolve_provider_command_from_env_or_default_for_cwd,
    AgentError, ProviderProfileMetadata, ProviderRegistry, SessionManager, StartAgentRequest,
};
use axis_core::agent::{AgentAttention, AgentLifecycle, AgentSessionId, AgentSessionRecord, AgentTransportKind};
use axis_core::agent_history::{AgentApprovalRequestId, AgentSessionDetail, AgentTimelineEntry};
use axis_core::workdesk::WorkdeskId;
use axis_core::worktree::WorktreeId;
use axis_core::SurfaceId;
use axis_terminal::TerminalAgentMetadata;

const CODEX_PROFILE_ID: &str = "codex";
const CLAUDE_CODE_PROFILE_ID: &str = "claude-code";
const CURSOR_PROFILE_ID: &str = "cursor";
const CLAUDE_CODE_CAPABILITY_NOTE: &str = "basic lifecycle only";
const CODEX_BIN_ENV: &str = "AXIS_CODEX_BIN";
const CLAUDE_CODE_BIN_ENV: &str = "AXIS_CLAUDE_CODE_BIN";
const CURSOR_BIN_ENV: &str = "AXIS_CURSOR_BIN";

/// Current connectivity status with the axisd daemon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DaemonStatus {
    /// Daemon is reachable and responding.
    Connected,
    /// Daemon was configured but is not currently reachable.
    Disconnected,
    /// No daemon is configured (local-only mode).
    LocalOnly,
}

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
    daemon_details: HashMap<AgentSessionId, AgentSessionDetail>,
    daemon_revision: u64,
    desk_cwd: HashMap<u64, String>,
    surface_to_session: HashMap<SurfaceRuntimeKey, AgentSessionId>,
    provider_options: Vec<ProviderProfileOption>,
    provider_option_command_sources: HashMap<String, ProviderOptionCommandSource>,
    /// Tracks when sessions entered terminal state (Completed/Failed/Cancelled)
    terminal_since: HashMap<AgentSessionId, std::time::Instant>,
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
        let cursor_resolution =
            resolve_provider_command_from_env_or_default(CURSOR_BIN_ENV, CURSOR_PROFILE_ID);
        registry.register_with_metadata(
            CURSOR_PROFILE_ID,
            std::sync::Arc::new(CursorProvider::with_base_argv(
                cursor_resolution.argv.clone(),
            )),
            None::<String>,
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
            ProviderProfileOption {
                profile_id: CURSOR_PROFILE_ID.to_string(),
                capability_note: None,
                available: cursor_resolution.available,
                unavailable_reason: cursor_resolution.unavailable_reason.clone(),
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
            (
                CURSOR_PROFILE_ID.to_string(),
                ProviderOptionCommandSource {
                    env_name: CURSOR_BIN_ENV,
                    default_binary: CURSOR_PROFILE_ID,
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
                daemon_details: HashMap::new(),
                daemon_revision: 0,
                desk_cwd: HashMap::new(),
                surface_to_session: HashMap::new(),
                provider_options,
                provider_option_command_sources,
                terminal_since: HashMap::new(),
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
        self.inner.lock().provider_options.clone()
    }

    pub fn provider_options_for_cwd(&self, cwd: &str) -> Vec<ProviderProfileOption> {
        let guard = self.inner.lock();
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
        let g = self.inner.lock();
        g.manager.revision().max(g.daemon_revision)
    }

    /// Check if the daemon is currently reachable by attempting a lightweight call.
    /// Returns `true` if the daemon responds successfully.
    pub fn check_daemon_health(&self) -> bool {
        let guard = self.inner.lock();
        guard.daemon.daemon_health().is_ok()
    }

    /// Return the current daemon connectivity status.
    pub fn daemon_status(&self) -> DaemonStatus {
        let guard = self.inner.lock();
        // If daemon socket path doesn't exist, treat as LocalOnly.
        if !guard.daemon.socket_path_exists() {
            return DaemonStatus::LocalOnly;
        }
        if guard.daemon.daemon_health().is_ok() {
            DaemonStatus::Connected
        } else {
            DaemonStatus::Disconnected
        }
    }

    /// Re-fetch all daemon sessions. Call when daemon comes back after a disconnect.
    /// Clears stale daemon records and refreshes from the live daemon.
    pub fn resync_daemon_sessions(&self) {
        let mut guard = self.inner.lock();
        match guard.daemon.list_agents(None) {
            Ok(sessions) => {
                let daemon_ids: HashSet<AgentSessionId> =
                    sessions.iter().map(|r| r.id.clone()).collect();
                guard.daemon_records = sessions
                    .into_iter()
                    .map(|record| (record.id.clone(), record))
                    .collect();
                guard
                    .daemon_details
                    .retain(|session_id, _| daemon_ids.contains(session_id));
                let local_ids = guard
                    .manager
                    .sessions()
                    .map(|record| record.id.clone())
                    .collect::<HashSet<_>>();
                guard.surface_to_session.retain(|_, existing| {
                    daemon_ids.contains(existing) || local_ids.contains(existing)
                });
                guard.daemon_revision = guard.daemon_revision.wrapping_add(1);
            }
            Err(_) => {
                // Daemon still not available; leave existing cached state intact.
            }
        }
    }

    fn key(workdesk_runtime_id: u64, surface_id: SurfaceId) -> SurfaceRuntimeKey {
        SurfaceRuntimeKey {
            workdesk_runtime_id,
            surface_id,
        }
    }

    pub fn set_desk_cwd(&self, workdesk_runtime_id: u64, cwd: String) {
        self.inner.lock().desk_cwd.insert(workdesk_runtime_id, cwd);
    }

    fn resolve_cwd(&self, workdesk_runtime_id: u64, fallback: &str) -> String {
        let g = self.inner.lock();
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
            .surface_to_session
            .contains_key(&Self::key(workdesk_runtime_id, surface_id))
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
        let mut guard = self.inner.lock();
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
            workdesk_id: Some(workdesk_runtime_id.to_string()),
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
        let default_profile_id = self.inner.lock().default_profile_id.clone();
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
        let guard = self.inner.lock();
        let sid = guard
            .surface_to_session
            .get(&Self::key(workdesk_runtime_id, surface_id))?;
        let inferred_attention = guard
            .daemon_details
            .get(sid)
            .filter(|detail| detail.pending_approval_id.is_some())
            .map(|_| AgentAttention::NeedsReview)
            .or_else(|| {
                guard
                    .manager
                    .session_detail(sid)
                    .filter(|detail| detail.pending_approval_id.is_some())
                    .map(|_| AgentAttention::NeedsReview)
            });
        inferred_attention.or_else(|| {
            guard
                .daemon_records
                .get(sid)
                .map(|record| record.attention)
                .or_else(|| guard.manager.session(sid).map(|r| r.attention))
        })
    }

    fn record_for_key(guard: &BridgeInner, key: SurfaceRuntimeKey) -> Option<AgentSessionRecord> {
        let sid = guard.surface_to_session.get(&key)?;
        if let Some(record) = guard.daemon_records.get(sid) {
            return Some(record.clone());
        }
        let mut record = guard.manager.session(sid)?.clone();
        record.surface_id = Some(key.surface_id);
        Some(record)
    }

    pub(crate) fn session_for_surface(
        &self,
        workdesk_runtime_id: u64,
        surface_id: SurfaceId,
    ) -> Option<AgentSessionRecord> {
        let guard = self.inner.lock();
        Self::record_for_key(&guard, Self::key(workdesk_runtime_id, surface_id))
    }

    pub(crate) fn sessions_snapshot(&self) -> Vec<AgentSessionRecord> {
        let guard = self.inner.lock();
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
        self.inner.lock().manager.provider_profile(profile_id)
    }

    /// Poll all active local sessions and refresh daemon session state.
    /// Returns the number of local sessions polled.
    pub fn poll_all_active_sessions(&self) -> usize {
        let mut guard = self.inner.lock();
        let all_session_ids: Vec<AgentSessionId> = guard
            .surface_to_session
            .values()
            .cloned()
            .collect();

        // Refresh daemon sessions once if any are registered.
        let has_daemon = all_session_ids
            .iter()
            .any(|id| guard.daemon_records.contains_key(id));
        if has_daemon {
            if let Ok(sessions) = guard.daemon.list_agents(None) {
                let daemon_ids: HashSet<AgentSessionId> =
                    sessions.iter().map(|r| r.id.clone()).collect();
                guard.daemon_records = sessions
                    .into_iter()
                    .map(|record| (record.id.clone(), record))
                    .collect();
                guard
                    .daemon_details
                    .retain(|session_id, _| daemon_ids.contains(session_id));
                let local_ids = guard
                    .manager
                    .sessions()
                    .map(|record| record.id.clone())
                    .collect::<HashSet<_>>();
                guard.surface_to_session.retain(|_, existing| {
                    daemon_ids.contains(existing) || local_ids.contains(existing)
                });
                guard.daemon_revision = guard.daemon_revision.wrapping_add(1);
            }
        }

        let mut polled = 0;
        for session_id in &all_session_ids {
            if guard.daemon_records.contains_key(session_id) {
                continue;
            }
            if let Some(session) = guard.manager.session(session_id) {
                if matches!(
                    session.lifecycle,
                    AgentLifecycle::Completed | AgentLifecycle::Failed | AgentLifecycle::Cancelled
                ) {
                    continue;
                }
            }
            let _ = guard.manager.poll_provider(session_id);
            polled += 1;

            // After polling a session, check if it became terminal
            if let Some(session) = guard.manager.session(session_id) {
                match session.lifecycle {
                    AgentLifecycle::Completed | AgentLifecycle::Failed | AgentLifecycle::Cancelled => {
                        guard.terminal_since
                            .entry(session_id.clone())
                            .or_insert_with(std::time::Instant::now);
                    }
                    _ => {}
                }
            }
        }
        polled
    }

    /// Remove sessions that have been in terminal state for longer than max_age.
    pub fn prune_expired_sessions(&self, max_age: std::time::Duration) {
        let mut guard = self.inner.lock();
        let now = std::time::Instant::now();
        let expired: Vec<AgentSessionId> = guard
            .terminal_since
            .iter()
            .filter(|(_, since)| now.duration_since(**since) >= max_age)
            .map(|(id, _)| id.clone())
            .collect();

        for session_id in &expired {
            guard.terminal_since.remove(session_id);
            guard.surface_to_session.retain(|_, sid| sid != session_id);
            let _ = guard.manager.stop_session(session_id);
        }
    }

    pub fn poll_surface(
        &self,
        workdesk_runtime_id: u64,
        surface_id: SurfaceId,
    ) -> Result<(), String> {
        let mut guard = self.inner.lock();
        let Some(sid) = guard
            .surface_to_session
            .get(&Self::key(workdesk_runtime_id, surface_id))
            .cloned()
        else {
            return Ok(());
        };
        if guard.daemon_records.contains_key(&sid) {
            let sessions = match guard.daemon.list_agents(None) {
                Ok(s) => s,
                Err(_) => return Ok(()), // Daemon unavailable, skip gracefully
            };
            guard.daemon_records = sessions
                .into_iter()
                .map(|record| (record.id.clone(), record))
                .collect();
            let daemon_ids = guard.daemon_records.keys().cloned().collect::<HashSet<_>>();
            guard
                .daemon_details
                .retain(|session_id, _| daemon_ids.contains(session_id));
            let local_ids = guard
                .manager
                .sessions()
                .map(|record| record.id.clone())
                .collect::<HashSet<_>>();
            guard.surface_to_session.retain(|_, existing| {
                daemon_ids.contains(existing) || local_ids.contains(existing)
            });
            guard.daemon_revision = guard.daemon_revision.wrapping_add(1);
            if daemon_ids.contains(&sid) {
                match guard.daemon.get_agent(&sid, None) {
                    Ok(detail) => {
                        cache_daemon_detail(&mut guard, detail);
                    }
                    Err(_) => {
                        // Session disappeared between list and get — remove from cache
                        guard.daemon_records.remove(&sid);
                        guard.daemon_details.remove(&sid);
                    }
                }
            }
            return Ok(());
        }
        guard.manager.poll_provider(&sid).map_err(|e| e.to_string())
    }

    pub(crate) fn stop_session(&self, session_id: &AgentSessionId) -> Result<(), String> {
        let mut guard = self.inner.lock();
        let matching_key = guard
            .surface_to_session
            .iter()
            .find_map(|(key, existing)| (existing == session_id).then_some(*key));
        if guard.daemon_records.contains_key(session_id) {
            guard.daemon.stop_agent(session_id)?;
            guard.daemon_records.remove(session_id);
            guard.daemon_details.remove(session_id);
            if let Some(key) = matching_key {
                guard.surface_to_session.remove(&key);
            }
            guard.daemon_revision = guard.daemon_revision.wrapping_add(1);
            return Ok(());
        }
        if guard.manager.session(session_id).is_none() {
            guard.daemon.stop_agent(session_id)?;
            guard.daemon_records.remove(session_id);
            guard.daemon_details.remove(session_id);
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
        let mut guard = self.inner.lock();
        let Some(sid) = guard
            .surface_to_session
            .remove(&Self::key(workdesk_runtime_id, surface_id))
        else {
            return;
        };
        if guard.daemon_records.contains_key(&sid) {
            let _ = guard.daemon.stop_agent(&sid);
            guard.daemon_records.remove(&sid);
            guard.daemon_details.remove(&sid);
            guard.daemon_revision = guard.daemon_revision.wrapping_add(1);
            return;
        }
        if guard.manager.session(&sid).is_none() && guard.daemon.stop_agent(&sid).is_ok() {
            guard.daemon_records.remove(&sid);
            guard.daemon_details.remove(&sid);
            guard.daemon_revision = guard.daemon_revision.wrapping_add(1);
            return;
        }
        let _ = guard.manager.stop_session(&sid);
    }

    pub(crate) fn session_detail(
        &self,
        agent_session_id: &AgentSessionId,
        after_sequence: Option<u64>,
    ) -> Result<AgentSessionDetail, String> {
        let mut guard = self.inner.lock();
        if let Some(detail) = Self::local_detail_for_session(&guard, agent_session_id) {
            return Ok(filter_detail_after_sequence(detail, after_sequence));
        }
        let detail = guard.daemon.get_agent(agent_session_id, None)?;
        cache_daemon_detail(&mut guard, detail.clone());
        Ok(filter_detail_after_sequence(detail, after_sequence))
    }

    pub(crate) fn send_turn(
        &self,
        agent_session_id: &AgentSessionId,
        text: &str,
    ) -> Result<AgentSessionDetail, String> {
        let mut guard = self.inner.lock();
        if guard.manager.session(agent_session_id).is_some() {
            guard
                .manager
                .send_turn(agent_session_id, text)
                .map_err(|error| {
                    if let Some(agent_err) = error.downcast_ref::<AgentError>() {
                        agent_err.to_string()
                    } else {
                        error.to_string()
                    }
                })?;
            return Self::local_detail_for_session(&guard, agent_session_id)
                .ok_or_else(|| format!("unknown session {}", agent_session_id.0));
        }
        let detail = guard.daemon.send_agent_turn(agent_session_id, text)?;
        cache_daemon_detail(&mut guard, detail.clone());
        Ok(detail)
    }

    pub(crate) fn respond_approval(
        &self,
        agent_session_id: &AgentSessionId,
        approval_request_id: &AgentApprovalRequestId,
        approved: bool,
        note: Option<String>,
    ) -> Result<AgentSessionDetail, String> {
        let mut guard = self.inner.lock();
        if guard.manager.session(agent_session_id).is_some() {
            guard
                .manager
                .respond_approval(agent_session_id, approval_request_id, approved, note)
                .map_err(|error| {
                    if let Some(agent_err) = error.downcast_ref::<AgentError>() {
                        agent_err.to_string()
                    } else {
                        error.to_string()
                    }
                })?;
            return Self::local_detail_for_session(&guard, agent_session_id)
                .ok_or_else(|| format!("unknown session {}", agent_session_id.0));
        }
        let detail = guard.daemon.respond_agent_approval(
            agent_session_id,
            approval_request_id,
            approved,
            note,
        )?;
        cache_daemon_detail(&mut guard, detail.clone());
        Ok(detail)
    }

    pub(crate) fn resume(
        &self,
        agent_session_id: &AgentSessionId,
    ) -> Result<AgentSessionDetail, String> {
        let mut guard = self.inner.lock();
        if guard.manager.session(agent_session_id).is_some() {
            guard
                .manager
                .resume(agent_session_id)
                .map_err(|error| {
                    if let Some(agent_err) = error.downcast_ref::<AgentError>() {
                        agent_err.to_string()
                    } else {
                        error.to_string()
                    }
                })?;
            return Self::local_detail_for_session(&guard, agent_session_id)
                .ok_or_else(|| format!("unknown session {}", agent_session_id.0));
        }
        let detail = guard.daemon.resume_agent(agent_session_id)?;
        cache_daemon_detail(&mut guard, detail.clone());
        Ok(detail)
    }

    fn local_detail_for_session(
        guard: &BridgeInner,
        agent_session_id: &AgentSessionId,
    ) -> Option<AgentSessionDetail> {
        let mut detail = guard.manager.session_detail(agent_session_id)?.clone();
        if let Some((key, _)) = guard
            .surface_to_session
            .iter()
            .find(|(_, existing)| *existing == agent_session_id)
        {
            detail.session.surface_id = Some(key.surface_id);
        }
        Some(detail)
    }
}

impl Default for AgentRuntimeBridge {
    fn default() -> Self {
        Self::new()
    }
}

fn cache_daemon_detail(guard: &mut BridgeInner, detail: AgentSessionDetail) {
    guard
        .daemon_records
        .insert(detail.session.id.clone(), detail.session.clone());
    guard
        .daemon_details
        .insert(detail.session.id.clone(), detail);
    guard.daemon_revision = guard.daemon_revision.wrapping_add(1);
}

fn filter_detail_after_sequence(
    mut detail: AgentSessionDetail,
    after_sequence: Option<u64>,
) -> AgentSessionDetail {
    if let Some(after_sequence) = after_sequence {
        detail
            .timeline
            .retain(|entry| timeline_entry_sequence(entry) >= after_sequence);
    }
    detail
}

fn timeline_entry_sequence(entry: &AgentTimelineEntry) -> u64 {
    match entry {
        AgentTimelineEntry::Turn { sequence, .. }
        | AgentTimelineEntry::ToolCall { sequence, .. }
        | AgentTimelineEntry::ApprovalRequest { sequence, .. } => *sequence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axis_agent_runtime::adapters::fake::FakeProvider;
    use axis_core::agent::{
        AgentAttention, AgentLifecycle, AgentSessionRecord, AgentTransportKind,
    };
    use axis_core::agent_history::{AgentApprovalRequestId, AgentSessionDetail};
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
        registry.register("beta", Arc::new(FakeProvider::with_standard_script()));

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
        registry.register("p1", Arc::new(FakeProvider::with_standard_script()));
        registry.register("p2", Arc::new(FakeProvider::with_standard_script()));
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

    #[test]
    fn pending_approval_detail_elevates_surface_attention() {
        let bridge = AgentRuntimeBridge::with_registry("fake", ProviderRegistry::new());
        let runtime_id = 7;
        let surface_id = SurfaceId::new(11);
        let session_id = AgentSessionId::new("daemon-session-1");
        let record = AgentSessionRecord {
            id: session_id.clone(),
            provider_profile_id: "fake".to_string(),
            transport: AgentTransportKind::CliWrapped,
            workdesk_id: Some(runtime_id.to_string()),
            surface_id: Some(surface_id),
            cwd: "/repo".to_string(),
            lifecycle: AgentLifecycle::Waiting,
            attention: AgentAttention::Quiet,
            status_message: "waiting for approval".to_string(),
        };
        let detail = AgentSessionDetail {
            session: record.clone(),
            capabilities: Default::default(),
            started_at_ms: Some(1),
            updated_at_ms: Some(2),
            completed_at_ms: None,
            revision: 1,
            history_cursor: 0,
            pending_approval_id: Some(AgentApprovalRequestId::new("approval-1")),
            timeline: Vec::new(),
            truncated: false,
        };
        let mut guard = bridge.inner.lock();
        guard
            .surface_to_session
            .insert(AgentRuntimeBridge::key(runtime_id, surface_id), session_id.clone());
        guard.daemon_records.insert(session_id.clone(), record);
        guard.daemon_details.insert(session_id, detail);
        drop(guard);

        assert_eq!(
            bridge.attention_for_surface(runtime_id, surface_id),
            Some(AgentAttention::NeedsReview)
        );
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
