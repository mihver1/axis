use crate::agent_runtime::DaemonAgentRuntime;
use crate::gui_launcher::GuiLauncher;
use crate::persistence::{load_registry, save_registry};
use crate::registry::{DaemonRegistry, TerminalRegistry};
use crate::transcript_store::TranscriptStore;
use anyhow::Context;
use axis_agent_runtime::WorktreeService;
use axis_core::automation::{AutomationRequest, AutomationResponse};
use axis_core::worktree::ReviewSummary;
use axis_terminal::TerminalGridSize;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::{fs::PermissionsExt, net::UnixListener};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[allow(dead_code)]
#[derive(Debug)]
pub struct RunningDaemon {
    socket_path: PathBuf,
    shutdown_tx: Option<mpsc::Sender<()>>,
    join_handle: Option<JoinHandle<()>>,
}

impl Drop for RunningDaemon {
    fn drop(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        if let Some(join_handle) = self.join_handle.take() {
            let _ = join_handle.join();
        }
        let _ = fs::remove_file(&self.socket_path);
    }
}

#[derive(Debug, Deserialize)]
struct SocketAutomationRequest {
    #[serde(default)]
    id: Option<Value>,
    #[serde(flatten)]
    request: AutomationRequest,
}

#[derive(Debug, Serialize)]
struct SocketAutomationResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(flatten)]
    response: AutomationResponse,
}

#[derive(Debug)]
struct DecodedRequest {
    id: Option<Value>,
    request: AutomationRequest,
    wrapped: bool,
}

#[derive(Clone, Debug)]
struct GuiHeartbeatRecord {
    gui_pid: u32,
    last_seen_at_ms: u64,
}

const PENDING_GUI_PID: u32 = 0;

struct DaemonState {
    workdesks: DaemonRegistry,
    terminals: TerminalRegistry,
    agent_runtime: DaemonAgentRuntime,
    gui_heartbeats: HashMap<String, GuiHeartbeatRecord>,
    gui_launcher: GuiLauncher,
}

#[allow(dead_code)]
pub fn run_forever(socket_path: PathBuf, data_dir: PathBuf) -> anyhow::Result<()> {
    let state = Arc::new(Mutex::new(DaemonState {
        workdesks: load_registry(&data_dir)?,
        terminals: TerminalRegistry::new(TranscriptStore::new(data_dir.join("transcripts"))),
        agent_runtime: DaemonAgentRuntime::new(),
        gui_heartbeats: HashMap::new(),
        gui_launcher: GuiLauncher::default(),
    }));
    let listener = bind_listener(&socket_path)?;
    serve(
        listener,
        socket_path,
        data_dir,
        state,
        None::<mpsc::Receiver<()>>,
    )
}

#[allow(dead_code)]
pub fn start_background_daemon(
    socket_path: PathBuf,
    data_dir: PathBuf,
) -> anyhow::Result<RunningDaemon> {
    let state = Arc::new(Mutex::new(DaemonState {
        workdesks: load_registry(&data_dir)?,
        terminals: TerminalRegistry::new(TranscriptStore::new(data_dir.join("transcripts"))),
        agent_runtime: DaemonAgentRuntime::new(),
        gui_heartbeats: HashMap::new(),
        gui_launcher: GuiLauncher::default(),
    }));
    let listener = bind_listener(&socket_path)?;
    listener
        .set_nonblocking(true)
        .with_context(|| format!("set nonblocking {}", socket_path.display()))?;

    let (shutdown_tx, shutdown_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel();
    let thread_socket_path = socket_path.clone();
    let thread_data_dir = data_dir.clone();
    let join_handle = thread::spawn(move || {
        let _ = ready_tx.send(());
        let _ = serve(
            listener,
            thread_socket_path,
            thread_data_dir,
            state,
            Some(shutdown_rx),
        );
    });
    ready_rx
        .recv_timeout(Duration::from_secs(1))
        .context("daemon thread did not become ready")?;

    Ok(RunningDaemon {
        socket_path,
        shutdown_tx: Some(shutdown_tx),
        join_handle: Some(join_handle),
    })
}

fn bind_listener(socket_path: &Path) -> anyhow::Result<UnixListener> {
    let Some(socket_dir) = socket_path.parent() else {
        anyhow::bail!("invalid daemon socket path")
    };
    fs::create_dir_all(socket_dir).with_context(|| format!("create {}", socket_dir.display()))?;

    if socket_path.exists() {
        match std::os::unix::net::UnixStream::connect(socket_path) {
            Ok(_) => anyhow::bail!("bind {}: daemon already running", socket_path.display()),
            Err(_) => {
                fs::remove_file(socket_path)
                    .with_context(|| format!("remove stale {}", socket_path.display()))?;
            }
        }
    }

    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("bind {}", socket_path.display()))?;
    fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("chmod {}", socket_path.display()))?;
    Ok(listener)
}

fn serve(
    listener: UnixListener,
    socket_path: PathBuf,
    data_dir: PathBuf,
    state: Arc<Mutex<DaemonState>>,
    shutdown_rx: Option<mpsc::Receiver<()>>,
) -> anyhow::Result<()> {
    loop {
        if shutdown_requested(&shutdown_rx) {
            break;
        }

        match listener.accept() {
            Ok((stream, _addr)) => {
                let thread_socket_path = socket_path.clone();
                let thread_data_dir = data_dir.clone();
                let thread_state = Arc::clone(&state);
                thread::spawn(move || {
                    let mut stream = stream;
                    let response_payload = handle_stream(
                        &mut stream,
                        &thread_socket_path,
                        &thread_data_dir,
                        &thread_state,
                    )
                    .unwrap_or_else(|error| {
                        serde_json::to_vec(&AutomationResponse::failure(error.to_string())).unwrap()
                    });
                    let _ = stream.write_all(&response_payload);
                    let _ = stream.write_all(b"\n");
                    let _ = stream.flush();
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                return Err(error).with_context(|| format!("accept {}", socket_path.display()))
            }
        }
    }

    Ok(())
}

fn shutdown_requested(shutdown_rx: &Option<mpsc::Receiver<()>>) -> bool {
    shutdown_rx
        .as_ref()
        .and_then(|rx| rx.try_recv().ok())
        .is_some()
}

fn handle_stream(
    stream: &mut std::os::unix::net::UnixStream,
    socket_path: &Path,
    data_dir: &Path,
    state: &Arc<Mutex<DaemonState>>,
) -> anyhow::Result<Vec<u8>> {
    let mut line = String::new();
    {
        let mut reader = BufReader::new(stream);
        reader.read_line(&mut line)?;
    }

    let trimmed = line.trim();
    if trimmed.is_empty() {
        return serde_json::to_vec(&AutomationResponse::failure(format!(
            "empty automation request sent to {}",
            socket_path.display()
        )))
        .context("serialize empty-request response");
    }

    let decoded = decode_request(trimmed)?;
    let response = handle_request(decoded.request, socket_path, data_dir, state)?;
    encode_response(decoded.id, decoded.wrapped, response)
}

fn decode_request(payload: &str) -> anyhow::Result<DecodedRequest> {
    if let Ok(request) = serde_json::from_str::<SocketAutomationRequest>(payload) {
        return Ok(DecodedRequest {
            id: request.id,
            request: request.request,
            wrapped: true,
        });
    }

    let request = serde_json::from_str::<AutomationRequest>(payload)
        .with_context(|| "parse automation request".to_string())?;
    Ok(DecodedRequest {
        id: None,
        request,
        wrapped: false,
    })
}

fn encode_response(
    id: Option<Value>,
    wrapped: bool,
    response: AutomationResponse,
) -> anyhow::Result<Vec<u8>> {
    if wrapped {
        serde_json::to_vec(&SocketAutomationResponse { id, response })
            .context("serialize socket response")
    } else {
        serde_json::to_vec(&response).context("serialize automation response")
    }
}

fn handle_request(
    request: AutomationRequest,
    socket_path: &Path,
    data_dir: &Path,
    state: &Arc<Mutex<DaemonState>>,
) -> anyhow::Result<AutomationResponse> {
    let response = match request {
        AutomationRequest::DaemonHealth => {
            let workdesk_count = state
                .lock()
                .map_err(|error| anyhow::anyhow!("registry lock poisoned: {error}"))?
                .workdesks
                .workdesk_count();
            AutomationResponse::success_with_result(json!({
                "status": "ok",
                "socket_path": socket_path.display().to_string(),
                "data_dir": data_dir.display().to_string(),
                "workdesk_count": workdesk_count,
            }))
        }
        AutomationRequest::WorktreeCreateOrAttach {
            repo_root,
            branch,
            attach_path,
        } => {
            let binding = match (attach_path, branch) {
                (Some(path), base_branch) => WorktreeService::attach(&path, base_branch)?,
                (None, Some(branch)) => {
                    let repo_binding = WorktreeService::attach(&repo_root, None)?;
                    let worktree_path = default_worktree_path(&repo_root, &branch)?;
                    if worktree_path.exists() {
                        WorktreeService::attach(&worktree_path, Some(repo_binding.branch))?
                    } else {
                        WorktreeService::create_worktree(
                            &repo_root,
                            &worktree_path,
                            &branch,
                            &repo_binding.branch,
                        )?
                    }
                }
                (None, None) => WorktreeService::attach(&repo_root, None)?,
            };
            AutomationResponse::success_with_result(json!({
                "worktree_id": binding.root_path,
                "binding": binding,
            }))
        }
        AutomationRequest::WorktreeStatus { worktree_id } => {
            let binding = WorktreeService::attach(&worktree_id.0, None)?;
            AutomationResponse::success_with_result(json!({
                "worktree_id": worktree_id,
                "binding": binding,
            }))
        }
        AutomationRequest::WorkdeskList { workspace_root } => {
            let records = state
                .lock()
                .map_err(|error| anyhow::anyhow!("registry lock poisoned: {error}"))?
                .workdesks
                .list_workdesks(workspace_root.as_deref());
            AutomationResponse::success_with_result(serde_json::to_value(records)?)
        }
        AutomationRequest::WorkdeskEnsure { record } => {
            let result = {
                let mut guard = state
                    .lock()
                    .map_err(|error| anyhow::anyhow!("registry lock poisoned: {error}"))?;
                let record = guard.workdesks.ensure_workdesk(record);
                save_registry(data_dir, &guard.workdesks)?;
                record
            };
            AutomationResponse::success_with_result(serde_json::to_value(result)?)
        }
        AutomationRequest::AgentStart {
            worktree_id,
            provider_profile_id,
            argv,
            workdesk_id,
            surface_id,
        } => {
            let record = state
                .lock()
                .map_err(|error| anyhow::anyhow!("registry lock poisoned: {error}"))?
                .agent_runtime
                .start_session(
                    worktree_id.0.clone(),
                    Some(provider_profile_id),
                    argv,
                    workdesk_id,
                    surface_id,
                )
                .map_err(anyhow::Error::msg)?;
            AutomationResponse::success_with_result(serde_json::to_value(record)?)
        }
        AutomationRequest::AgentStop { agent_session_id } => {
            state
                .lock()
                .map_err(|error| anyhow::anyhow!("registry lock poisoned: {error}"))?
                .agent_runtime
                .stop_session(&agent_session_id)
                .map_err(anyhow::Error::msg)?;
            AutomationResponse::success_with_result(json!({
                "agent_session_id": agent_session_id,
                "stopped": true,
            }))
        }
        AutomationRequest::AgentList { worktree_id } => {
            let mut guard = state
                .lock()
                .map_err(|error| anyhow::anyhow!("registry lock poisoned: {error}"))?;
            guard.agent_runtime.poll_all().map_err(anyhow::Error::msg)?;
            let sessions = guard
                .agent_runtime
                .sessions_snapshot()
                .into_iter()
                .filter(|record| {
                    worktree_id
                        .as_ref()
                        .map_or(true, |filter| record.cwd == filter.0)
                })
                .collect::<Vec<_>>();
            AutomationResponse::success_with_result(serde_json::to_value(sessions)?)
        }
        AutomationRequest::DeskReviewSummary { worktree_id } => {
            let binding = WorktreeService::attach(&worktree_id.0, None)?;
            let changed_files = binding
                .base_branch
                .as_deref()
                .map(|base_branch| {
                    WorktreeService::changed_files_since_base(&binding.root_path, base_branch)
                })
                .transpose()?
                .unwrap_or_default();
            let uncommitted_files = WorktreeService::uncommitted_changed_files(&binding.root_path)?;
            let summary = ReviewSummary {
                files_changed: changed_files.len() as u32,
                uncommitted_files: uncommitted_files.len() as u32,
                ready_for_review: !changed_files.is_empty() || !uncommitted_files.is_empty(),
                last_inspected_at_ms: Some(unix_time_ms()),
            };
            AutomationResponse::success_with_result(json!({
                "worktree_id": worktree_id,
                "summary": summary,
                "changed_files": changed_files,
                "uncommitted_files": uncommitted_files,
            }))
        }
        AutomationRequest::TerminalEnsure {
            workdesk_id,
            surface_id,
            kind,
            title,
            cwd,
            cols,
            rows,
        } => {
            let record = state
                .lock()
                .map_err(|error| anyhow::anyhow!("registry lock poisoned: {error}"))?
                .terminals
                .ensure_session(
                    &workdesk_id,
                    surface_id,
                    kind,
                    title,
                    cwd,
                    TerminalGridSize::new(cols, rows),
                )?;
            AutomationResponse::success_with_result(serde_json::to_value(record)?)
        }
        AutomationRequest::TerminalRead {
            terminal_session_id,
            offset,
        } => {
            let (record, chunk) = state
                .lock()
                .map_err(|error| anyhow::anyhow!("registry lock poisoned: {error}"))?
                .terminals
                .read_from(&terminal_session_id, offset)?;
            AutomationResponse::success_with_result(json!({
                "record": record,
                "chunk": chunk,
            }))
        }
        AutomationRequest::TerminalWrite {
            terminal_session_id,
            bytes,
        } => {
            let record = state
                .lock()
                .map_err(|error| anyhow::anyhow!("registry lock poisoned: {error}"))?
                .terminals
                .write_bytes(&terminal_session_id, &bytes)?;
            AutomationResponse::success_with_result(serde_json::to_value(record)?)
        }
        AutomationRequest::TerminalResize {
            terminal_session_id,
            cols,
            rows,
        } => {
            let record = state
                .lock()
                .map_err(|error| anyhow::anyhow!("registry lock poisoned: {error}"))?
                .terminals
                .resize(&terminal_session_id, TerminalGridSize::new(cols, rows))?;
            AutomationResponse::success_with_result(serde_json::to_value(record)?)
        }
        AutomationRequest::TerminalClose {
            terminal_session_id,
        } => {
            let record = state
                .lock()
                .map_err(|error| anyhow::anyhow!("registry lock poisoned: {error}"))?
                .terminals
                .close(&terminal_session_id)?;
            AutomationResponse::success_with_result(serde_json::to_value(record)?)
        }
        AutomationRequest::GuiHeartbeat {
            workspace_root,
            gui_pid,
        } => {
            let now_ms = unix_time_ms();
            state
                .lock()
                .map_err(|error| anyhow::anyhow!("registry lock poisoned: {error}"))?
                .gui_heartbeats
                .insert(
                    workspace_root.clone(),
                    GuiHeartbeatRecord {
                        gui_pid,
                        last_seen_at_ms: now_ms,
                    },
                );
            AutomationResponse::success_with_result(json!({
                "workspace_root": workspace_root,
                "gui_pid": gui_pid,
                "last_seen_at_ms": now_ms,
            }))
        }
        AutomationRequest::GuiEnsureRunning { workspace_root } => {
            let mut guard = state
                .lock()
                .map_err(|error| anyhow::anyhow!("registry lock poisoned: {error}"))?;
            let now_ms = unix_time_ms();
            let heartbeat = guard.gui_heartbeats.get(&workspace_root).cloned();
            if gui_heartbeat_is_fresh(heartbeat.as_ref(), now_ms) {
                AutomationResponse::success_with_result(json!({
                    "workspace_root": workspace_root,
                    "launched": false,
                    "app_bin": guard.gui_launcher.app_bin().display().to_string(),
                    "gui_pid": heartbeat.as_ref().map(|record| record.gui_pid),
                    "last_seen_at_ms": heartbeat.as_ref().map(|record| record.last_seen_at_ms),
                }))
            } else {
                guard.gui_launcher.launch(&workspace_root)?;
                let heartbeat = GuiHeartbeatRecord {
                    gui_pid: PENDING_GUI_PID,
                    last_seen_at_ms: now_ms,
                };
                guard
                    .gui_heartbeats
                    .insert(workspace_root.clone(), heartbeat.clone());
                AutomationResponse::success_with_result(json!({
                    "workspace_root": workspace_root,
                    "launched": true,
                    "app_bin": guard.gui_launcher.app_bin().display().to_string(),
                    "gui_pid": heartbeat.gui_pid,
                    "last_seen_at_ms": heartbeat.last_seen_at_ms,
                }))
            }
        }
        _ => AutomationResponse::failure("unsupported axisd automation request"),
    };

    Ok(response)
}

fn sanitize_branch_slug(branch: &str) -> String {
    let slug = branch
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "worktree".to_string()
    } else {
        trimmed.to_string()
    }
}

fn default_worktree_path(repo_root: &str, branch: &str) -> anyhow::Result<PathBuf> {
    let repo_root = PathBuf::from(repo_root);
    let repo_name = repo_root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("invalid repo root `{}`", repo_root.display()))?;
    let parent = repo_root
        .parent()
        .ok_or_else(|| anyhow::anyhow!("repo root `{}` has no parent", repo_root.display()))?;
    Ok(parent.join(format!("{repo_name}-{}", sanitize_branch_slug(branch))))
}

fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn gui_heartbeat_is_fresh(record: Option<&GuiHeartbeatRecord>, now_ms: u64) -> bool {
    record.is_some_and(|record| {
        now_ms.saturating_sub(record.last_seen_at_ms) <= gui_heartbeat_ttl_ms()
    })
}

fn gui_heartbeat_ttl_ms() -> u64 {
    std::env::var("AXIS_GUI_HEARTBEAT_TTL_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(5_000)
}
