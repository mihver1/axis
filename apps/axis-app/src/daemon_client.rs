use axis_core::agent::{AgentSessionId, AgentSessionRecord};
use axis_core::agent_history::{AgentApprovalRequestId, AgentSessionDetail};
use axis_core::automation::{
    AgentGetRequest, AgentRespondApprovalRequest, AgentResumeRequest, AgentSendTurnRequest,
    AutomationRequest, AutomationResponse,
};
use axis_core::paths::daemon_socket_path;
use axis_core::terminal::{
    TerminalSessionId, TerminalSessionRecord, TerminalSurfaceKind, TerminalTranscriptChunk,
};
use axis_core::workdesk::{WorkdeskId, WorkdeskRecord};
use axis_core::review::DeskReviewPayload;
use axis_core::worktree::{WorktreeBinding, WorktreeId};
use axis_core::SurfaceId;
use axis_terminal::TerminalGridSize;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_DAEMON_SOCKET_TIMEOUT_MS: u64 = 5_000;
#[cfg(test)]
const DAEMON_SOCKET_TIMEOUT_MS_ENV: &str = "AXIS_DAEMON_SOCKET_TIMEOUT_MS";

#[derive(Clone, Debug)]
pub(crate) struct DaemonClient {
    socket_path: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct TerminalReadResult {
    pub record: TerminalSessionRecord,
    #[serde(default)]
    pub chunk: Option<TerminalTranscriptChunk>,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct WorktreeBindingResult {
    pub worktree_id: WorktreeId,
    pub binding: WorktreeBinding,
}

impl Default for DaemonClient {
    fn default() -> Self {
        Self {
            socket_path: daemon_socket_path(),
        }
    }
}

impl DaemonClient {
    #[allow(dead_code)]
    pub fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }

    pub fn ensure_terminal(
        &self,
        workdesk_id: &str,
        surface_id: SurfaceId,
        kind: TerminalSurfaceKind,
        title: &str,
        cwd: &str,
        grid: TerminalGridSize,
    ) -> Result<TerminalSessionRecord, String> {
        self.send_typed_request(AutomationRequest::TerminalEnsure {
            workdesk_id: WorkdeskId::new(workdesk_id),
            surface_id,
            kind,
            title: title.to_string(),
            cwd: Some(cwd.to_string()),
            cols: grid.cols,
            rows: grid.rows,
            command: None,
        })
    }

    pub fn read_terminal(
        &self,
        terminal_session_id: &TerminalSessionId,
        offset: u64,
    ) -> Result<TerminalReadResult, String> {
        self.send_typed_request(AutomationRequest::TerminalRead {
            terminal_session_id: terminal_session_id.clone(),
            offset,
        })
    }

    pub fn write_terminal(
        &self,
        terminal_session_id: &TerminalSessionId,
        bytes: &[u8],
    ) -> Result<TerminalSessionRecord, String> {
        self.send_typed_request(AutomationRequest::TerminalWrite {
            terminal_session_id: terminal_session_id.clone(),
            bytes: bytes.to_vec(),
        })
    }

    pub fn resize_terminal(
        &self,
        terminal_session_id: &TerminalSessionId,
        grid: TerminalGridSize,
    ) -> Result<TerminalSessionRecord, String> {
        self.send_typed_request(AutomationRequest::TerminalResize {
            terminal_session_id: terminal_session_id.clone(),
            cols: grid.cols,
            rows: grid.rows,
        })
    }

    pub fn close_terminal(
        &self,
        terminal_session_id: &TerminalSessionId,
    ) -> Result<TerminalSessionRecord, String> {
        self.send_typed_request(AutomationRequest::TerminalClose {
            terminal_session_id: terminal_session_id.clone(),
        })
    }

    pub fn worktree_create_or_attach(
        &self,
        repo_root: String,
        branch: Option<String>,
        attach_path: Option<String>,
    ) -> Result<WorktreeBindingResult, String> {
        self.send_typed_request(AutomationRequest::WorktreeCreateOrAttach {
            repo_root,
            branch,
            attach_path,
        })
    }

    pub fn worktree_status(
        &self,
        worktree_id: &WorktreeId,
    ) -> Result<WorktreeBindingResult, String> {
        self.send_typed_request(AutomationRequest::WorktreeStatus {
            worktree_id: worktree_id.clone(),
        })
    }

    #[allow(dead_code)]
    pub fn list_workdesks(
        &self,
        workspace_root: Option<String>,
    ) -> Result<Vec<WorkdeskRecord>, String> {
        self.send_typed_request(AutomationRequest::WorkdeskList { workspace_root })
    }

    pub fn ensure_workdesk(&self, record: WorkdeskRecord) -> Result<WorkdeskRecord, String> {
        self.send_typed_request(AutomationRequest::WorkdeskEnsure { record })
    }

    pub fn start_agent(
        &self,
        worktree_id: &WorktreeId,
        provider_profile_id: String,
        argv: Vec<String>,
        workdesk_id: Option<WorkdeskId>,
        surface_id: Option<SurfaceId>,
    ) -> Result<AgentSessionRecord, String> {
        self.send_typed_request(AutomationRequest::AgentStart {
            worktree_id: worktree_id.clone(),
            provider_profile_id,
            argv,
            workdesk_id,
            surface_id,
        })
    }

    pub fn stop_agent(
        &self,
        agent_session_id: &AgentSessionId,
    ) -> Result<serde_json::Value, String> {
        self.send_typed_request(AutomationRequest::AgentStop {
            agent_session_id: agent_session_id.clone(),
        })
    }

    pub fn list_agents(
        &self,
        worktree_id: Option<&WorktreeId>,
    ) -> Result<Vec<AgentSessionRecord>, String> {
        self.send_typed_request(AutomationRequest::AgentList {
            worktree_id: worktree_id.cloned(),
        })
    }

    pub fn get_agent(
        &self,
        agent_session_id: &AgentSessionId,
        after_sequence: Option<u64>,
    ) -> Result<AgentSessionDetail, String> {
        self.send_typed_request(AutomationRequest::AgentGet(AgentGetRequest {
            agent_session_id: agent_session_id.clone(),
            after_sequence,
        }))
    }

    pub fn send_agent_turn(
        &self,
        agent_session_id: &AgentSessionId,
        text: &str,
    ) -> Result<AgentSessionDetail, String> {
        self.send_typed_request(AutomationRequest::AgentSendTurn(AgentSendTurnRequest {
            agent_session_id: agent_session_id.clone(),
            text: text.to_string(),
        }))
    }

    pub fn respond_agent_approval(
        &self,
        agent_session_id: &AgentSessionId,
        approval_request_id: &AgentApprovalRequestId,
        approved: bool,
        note: Option<String>,
    ) -> Result<AgentSessionDetail, String> {
        self.send_typed_request(AutomationRequest::AgentRespondApproval(
            AgentRespondApprovalRequest {
                agent_session_id: agent_session_id.clone(),
                approval_request_id: approval_request_id.clone(),
                approved,
                note,
            },
        ))
    }

    pub fn resume_agent(
        &self,
        agent_session_id: &AgentSessionId,
    ) -> Result<AgentSessionDetail, String> {
        self.send_typed_request(AutomationRequest::AgentResume(AgentResumeRequest {
            agent_session_id: agent_session_id.clone(),
        }))
    }

    pub fn desk_review_summary(
        &self,
        worktree_id: &WorktreeId,
    ) -> Result<DeskReviewPayload, String> {
        self.send_typed_request(AutomationRequest::DeskReviewSummary {
            worktree_id: worktree_id.clone(),
        })
    }

    #[allow(dead_code)]
    pub fn attention_next(
        &self,
        workdesk_id: Option<String>,
    ) -> Result<serde_json::Value, String> {
        self.send_typed_request(AutomationRequest::AttentionNext { workdesk_id })
    }

    #[allow(dead_code)]
    pub fn state_current(
        &self,
        workdesk_id: Option<String>,
    ) -> Result<serde_json::Value, String> {
        self.send_typed_request(AutomationRequest::StateCurrent { workdesk_id })
    }

    pub fn gui_heartbeat(
        &self,
        workspace_root: String,
        gui_pid: u32,
    ) -> Result<serde_json::Value, String> {
        self.send_typed_request(AutomationRequest::GuiHeartbeat {
            workspace_root,
            gui_pid,
        })
    }

    #[allow(dead_code)]
    pub fn gui_ensure_running(&self, workspace_root: String) -> Result<serde_json::Value, String> {
        self.send_typed_request(AutomationRequest::GuiEnsureRunning { workspace_root })
    }

    pub fn daemon_health(&self) -> Result<serde_json::Value, String> {
        self.send_typed_request(AutomationRequest::DaemonHealth)
    }

    fn send_typed_request<T: DeserializeOwned>(
        &self,
        request: AutomationRequest,
    ) -> Result<T, String> {
        let response = self.send_request(&request)?;
        if !response.ok {
            return Err(response
                .error
                .unwrap_or_else(|| "daemon automation request failed".to_string()));
        }
        let result = response
            .result
            .ok_or_else(|| "daemon automation request returned no result".to_string())?;
        serde_json::from_value(result).map_err(|error| format!("decode daemon response: {error}"))
    }

    fn send_request(&self, request: &AutomationRequest) -> Result<AutomationResponse, String> {
        let mut stream = UnixStream::connect(&self.socket_path)
            .map_err(|error| format!("connect {}: {error}", self.socket_path.display()))?;
        let timeout = daemon_socket_timeout();
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|error| format!("set daemon read timeout: {error}"))?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(|error| format!("set daemon write timeout: {error}"))?;
        let payload = serde_json::to_vec(request)
            .map_err(|error| format!("serialize daemon request: {error}"))?;
        stream
            .write_all(&payload)
            .map_err(|error| format!("write daemon request: {error}"))?;
        stream
            .write_all(b"\n")
            .map_err(|error| format!("write daemon request newline: {error}"))?;
        stream
            .flush()
            .map_err(|error| format!("flush daemon request: {error}"))?;

        let mut response_line = String::new();
        BufReader::new(stream)
            .read_line(&mut response_line)
            .map_err(|error| format!("read daemon response: {error}"))?;
        serde_json::from_str(response_line.trim())
            .map_err(|error| format!("parse daemon response: {error}"))
    }
}

fn daemon_socket_timeout() -> Duration {
    #[cfg(test)]
    {
        if let Some(timeout_ms) = std::env::var(DAEMON_SOCKET_TIMEOUT_MS_ENV)
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|value| *value > 0)
        {
            return Duration::from_millis(timeout_ms);
        }
    }

    Duration::from_millis(DEFAULT_DAEMON_SOCKET_TIMEOUT_MS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::net::UnixListener;
    use std::sync::{mpsc, Mutex, OnceLock};
    use std::thread;
    use std::time::Instant;

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct EnvVarGuard {
        key: &'static str,
        original: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let original = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.original {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    fn temp_socket_path() -> PathBuf {
        PathBuf::from(format!(
            "/tmp/axis-daemon-client-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos()
        ))
    }

    #[test]
    fn send_request_times_out_when_daemon_stalls() {
        let _env_guard = env_lock()
            .lock()
            .expect("environment test lock should not be poisoned");
        let _timeout_override = EnvVarGuard::set(DAEMON_SOCKET_TIMEOUT_MS_ENV, "50");
        let socket_path = temp_socket_path();
        let listener =
            UnixListener::bind(&socket_path).expect("test daemon listener should bind to socket");
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("test daemon should accept client");
            accepted_tx
                .send(())
                .expect("accept notification should send");
            let _ = release_rx.recv_timeout(Duration::from_secs(2));
            drop(stream);
        });

        let client = DaemonClient::new(socket_path.clone());
        let (result_tx, result_rx) = mpsc::channel();
        let client_thread = thread::spawn(move || {
            let started = Instant::now();
            let result = client.daemon_health();
            let elapsed = started.elapsed();
            result_tx
                .send((elapsed, result))
                .expect("client result should send");
        });

        accepted_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("server should accept client request");
        let outcome = result_rx.recv_timeout(Duration::from_millis(500));

        let _ = release_tx.send(());
        client_thread
            .join()
            .expect("client thread should finish after server closes");
        server
            .join()
            .expect("server thread should finish after release signal");
        let _ = std::fs::remove_file(&socket_path);

        let (elapsed, result) =
            outcome.expect("daemon request should time out instead of blocking indefinitely");
        assert!(
            elapsed < Duration::from_millis(500),
            "daemon request should fail fast, got {elapsed:?}"
        );
        let error = result.expect_err("stalled daemon request should fail");
        assert!(
            error.contains("read daemon response"),
            "expected read timeout error, got {error}"
        );
    }
}
