//! Canonical CLI adapter for OpenAI Codex (or compatible wrappers) via `process-manager`.

use std::collections::HashMap;
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use axis_core::agent::{AgentLifecycle, AgentSessionId};
use axis_core::agent_history::AgentSessionCapabilities;

use crate::cli_protocol::{encode_axis_command, parse_axis_output_line, AxisCliCommand};
use crate::events::RuntimeEvent;
use crate::provider::{
    AgentProvider, RespondApprovalRequest, ResumeRequest, SendTurnRequest, StartAgentRequest,
    StartedSession,
};
use process_manager::{spawn_process_launch, ProcessLaunchSpec, TerminalGridSize, WaitOutcome};

const DEFAULT_GRID: TerminalGridSize = TerminalGridSize::new(80, 24);
/// Max bytes retained without a newline (defensive cap for marker parsing).
const MAX_MARKER_BUFFER: usize = 64 * 1024;

pub struct CodexProvider {
    base_argv: Vec<String>,
    inner: Mutex<CodexInner>,
}

struct CodexInner {
    next_id: u64,
    sessions: HashMap<AgentSessionId, Arc<Mutex<CodexSession>>>,
}

struct CodexSession {
    spawned: process_manager::SpawnedProcess,
    buf: Vec<u8>,
    emitted_boot: bool,
    lifecycle_terminal: bool,
}

impl CodexProvider {
    pub fn new() -> Self {
        Self::with_base_argv(vec!["codex".to_string()])
    }

    pub fn with_base_argv(base_argv: Vec<String>) -> Self {
        Self {
            base_argv,
            inner: Mutex::new(CodexInner {
                next_id: 1,
                sessions: HashMap::new(),
            }),
        }
    }

    fn build_argv(&self, req: &StartAgentRequest) -> Vec<String> {
        self.base_argv
            .iter()
            .cloned()
            .chain(req.argv_suffix.iter().cloned())
            .collect()
    }
}

impl Default for CodexProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentProvider for CodexProvider {
    fn capabilities(&self) -> AgentSessionCapabilities {
        AgentSessionCapabilities {
            turn_input: true,
            tool_calls: true,
            approvals: true,
            resume: true,
            terminal_attachment: false,
        }
    }

    fn start(&self, req: StartAgentRequest) -> anyhow::Result<StartedSession> {
        let argv = self.build_argv(&req);
        let launch = ProcessLaunchSpec {
            argv,
            cwd: Some(PathBuf::from(&req.cwd)),
            env: req.env.clone(),
            use_pty: false,
        };
        let spawned = spawn_process_launch(&launch, DEFAULT_GRID)
            .map_err(|e| anyhow::anyhow!("codex spawn failed: {e:#}"))?;

        let slot = Arc::new(Mutex::new(CodexSession {
            spawned,
            buf: Vec::new(),
            emitted_boot: false,
            lifecycle_terminal: false,
        }));

        let mut g = self
            .inner
            .lock()
            .map_err(|e| anyhow::anyhow!("codex provider lock poisoned: {e}"))?;
        let id = AgentSessionId::new(format!("codex-session-{}", g.next_id));
        g.next_id += 1;
        g.sessions.insert(id.clone(), slot);
        Ok(StartedSession { session_id: id })
    }

    fn poll_events(&self, session_id: &AgentSessionId) -> anyhow::Result<Vec<RuntimeEvent>> {
        let slot = {
            let g = self
                .inner
                .lock()
                .map_err(|e| anyhow::anyhow!("codex provider lock poisoned: {e}"))?;
            g.sessions
                .get(session_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("unknown codex session {}", session_id.0))?
        };

        let mut state = slot
            .lock()
            .map_err(|e| anyhow::anyhow!("codex session lock poisoned: {e}"))?;

        if state.lifecycle_terminal {
            return Ok(vec![]);
        }

        if !state.emitted_boot {
            state.emitted_boot = true;
            let mut out = vec![
                RuntimeEvent::Lifecycle {
                    session_id: session_id.clone(),
                    lifecycle: AgentLifecycle::Starting,
                },
                RuntimeEvent::Lifecycle {
                    session_id: session_id.clone(),
                    lifecycle: AgentLifecycle::Running,
                },
            ];
            drain_child_stdout(&mut *state, session_id, &mut out)?;
            if let WaitOutcome::Exited(ex) = state.spawned.process.try_wait_exit()? {
                state.lifecycle_terminal = true;
                out.push(RuntimeEvent::Lifecycle {
                    session_id: session_id.clone(),
                    lifecycle: exit_to_lifecycle(&ex),
                });
            }
            return Ok(out);
        }

        let mut out = Vec::new();
        drain_child_stdout(&mut *state, session_id, &mut out)?;

        if let WaitOutcome::Exited(ex) = state.spawned.process.try_wait_exit()? {
            state.lifecycle_terminal = true;
            out.push(RuntimeEvent::Lifecycle {
                session_id: session_id.clone(),
                lifecycle: exit_to_lifecycle(&ex),
            });
        }

        Ok(out)
    }

    fn send_turn(&self, req: SendTurnRequest) -> anyhow::Result<Vec<RuntimeEvent>> {
        let command = encode_axis_command(&AxisCliCommand::SendTurn { text: req.text })?;
        self.write_command(&req.session_id, &command)
    }

    fn respond_approval(&self, req: RespondApprovalRequest) -> anyhow::Result<Vec<RuntimeEvent>> {
        let command = encode_axis_command(&AxisCliCommand::RespondApproval {
            approval_request_id: req.approval_request_id,
            approved: req.approved,
            note: req.note,
        })?;
        self.write_command(&req.session_id, &command)
    }

    fn resume(&self, req: ResumeRequest) -> anyhow::Result<Vec<RuntimeEvent>> {
        let command = encode_axis_command(&AxisCliCommand::Resume)?;
        self.write_command(&req.session_id, &command)
    }

    fn stop(&self, session_id: &AgentSessionId) -> anyhow::Result<()> {
        let slot = {
            let mut g = self
                .inner
                .lock()
                .map_err(|e| anyhow::anyhow!("codex provider lock poisoned: {e}"))?;
            g.sessions
                .remove(session_id)
                .ok_or_else(|| anyhow::anyhow!("unknown codex session {}", session_id.0))?
        };

        let process = {
            let session = slot
                .lock()
                .map_err(|e| anyhow::anyhow!("codex session lock poisoned: {e}"))?;
            session.spawned.process.clone()
        };
        process
            .kill()
            .context("codex stop: failed to kill child process")?;
        Ok(())
    }
}

impl CodexProvider {
    fn write_command(
        &self,
        session_id: &AgentSessionId,
        command: &str,
    ) -> anyhow::Result<Vec<RuntimeEvent>> {
        let slot = {
            let g = self
                .inner
                .lock()
                .map_err(|e| anyhow::anyhow!("codex provider lock poisoned: {e}"))?;
            g.sessions
                .get(session_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("unknown codex session {}", session_id.0))?
        };
        let mut state = slot
            .lock()
            .map_err(|e| anyhow::anyhow!("codex session lock poisoned: {e}"))?;
        let mut out = Vec::new();
        emit_boot_events_if_needed(&mut state, session_id, &mut out);
        state
            .spawned
            .process
            .write_all(command.as_bytes())
            .context("codex command write failed")?;
        let baseline = out.len();
        drain_child_stdout(&mut state, session_id, &mut out)?;
        for _ in 0..20 {
            if out.len() > baseline {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
            drain_child_stdout(&mut state, session_id, &mut out)?;
        }
        Ok(out)
    }
}

fn exit_to_lifecycle(ex: &process_manager::ProcessExit) -> AgentLifecycle {
    if ex.is_success() {
        AgentLifecycle::Completed
    } else {
        AgentLifecycle::Failed
    }
}

fn enforce_marker_buffer_cap(buf: &mut Vec<u8>) {
    while buf.len() > MAX_MARKER_BUFFER {
        if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            buf.drain(..=pos);
        } else {
            let drop = buf.len() - MAX_MARKER_BUFFER;
            buf.drain(..drop);
        }
    }
}

fn drain_child_stdout(
    session: &mut CodexSession,
    session_id: &AgentSessionId,
    out: &mut Vec<RuntimeEvent>,
) -> anyhow::Result<()> {
    let mut chunk = [0u8; 4096];
    loop {
        match session.spawned.reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                session.buf.extend_from_slice(&chunk[..n]);
                enforce_marker_buffer_cap(&mut session.buf);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(anyhow::anyhow!("read codex stdout: {e}")),
        }
    }

    while let Some(pos) = session.buf.iter().position(|b| *b == b'\n') {
        let line_bytes: Vec<u8> = session.buf.drain(..=pos).collect();
        let mut line =
            String::from_utf8_lossy(&line_bytes[..line_bytes.len().saturating_sub(1)]).into_owned();
        if line.ends_with('\r') {
            line.pop();
        }
        if let Some(events) = parse_axis_output_line(&line, session_id) {
            out.extend(events);
        }
    }
    Ok(())
}

#[cfg(test)]
mod buffer_cap_tests {
    use super::{enforce_marker_buffer_cap, MAX_MARKER_BUFFER};

    #[test]
    fn marker_buffer_cap_drops_oldest_without_newline() {
        let mut buf = vec![b'x'; MAX_MARKER_BUFFER + 500];
        enforce_marker_buffer_cap(&mut buf);
        assert!(buf.len() <= MAX_MARKER_BUFFER);
    }
}

fn emit_boot_events_if_needed(
    session: &mut CodexSession,
    session_id: &AgentSessionId,
    out: &mut Vec<RuntimeEvent>,
) {
    if session.emitted_boot {
        return;
    }
    session.emitted_boot = true;
    out.push(RuntimeEvent::Lifecycle {
        session_id: session_id.clone(),
        lifecycle: AgentLifecycle::Starting,
    });
    out.push(RuntimeEvent::Lifecycle {
        session_id: session_id.clone(),
        lifecycle: AgentLifecycle::Running,
    });
}
