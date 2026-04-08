//! CLI adapter for Cursor (ACP-compatible agent) via `process-manager`.

use std::collections::HashMap;
use std::io::{self, Read};
use std::path::PathBuf;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axis_core::agent::{AgentLifecycle, AgentSessionId};
use axis_core::agent_history::AgentSessionCapabilities;

use crate::cli_protocol::{encode_axis_command, parse_axis_output_line, AxisCliCommand};
use crate::events::RuntimeEvent;
use crate::provider::{
    AgentProvider, RespondApprovalRequest, SendTurnRequest, StartAgentRequest, StartedSession,
};
use process_manager::{spawn_process_launch, ProcessLaunchSpec, TerminalGridSize, WaitOutcome};

const DEFAULT_GRID: TerminalGridSize = TerminalGridSize::new(80, 24);

pub struct CursorProvider {
    base_argv: Vec<String>,
    inner: Mutex<CursorInner>,
}

struct CursorInner {
    next_id: u64,
    sessions: HashMap<AgentSessionId, Arc<Mutex<CursorSession>>>,
}

struct CursorSession {
    spawned: process_manager::SpawnedProcess,
    buf: Vec<u8>,
    emitted_boot: bool,
    lifecycle_terminal: bool,
}

impl CursorProvider {
    pub fn new() -> Self {
        Self::with_base_argv(vec!["cursor".to_string()])
    }

    pub fn with_base_argv(base_argv: Vec<String>) -> Self {
        Self {
            base_argv,
            inner: Mutex::new(CursorInner {
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

impl Default for CursorProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentProvider for CursorProvider {
    fn capabilities(&self) -> AgentSessionCapabilities {
        AgentSessionCapabilities {
            turn_input: true,
            tool_calls: false,
            approvals: true,
            resume: false,
            terminal_attachment: true,
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
            .map_err(|e| anyhow::anyhow!("cursor spawn failed: {e:#}"))?;

        let slot = Arc::new(Mutex::new(CursorSession {
            spawned,
            buf: Vec::new(),
            emitted_boot: false,
            lifecycle_terminal: false,
        }));

        let mut g = self.inner.lock();
        let id = AgentSessionId::new(format!("cursor-session-{}", g.next_id));
        g.next_id += 1;
        g.sessions.insert(id.clone(), slot);
        Ok(StartedSession { session_id: id })
    }

    fn poll_events(&self, session_id: &AgentSessionId) -> anyhow::Result<Vec<RuntimeEvent>> {
        let slot = {
            let g = self.inner.lock();
            g.sessions
                .get(session_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("unknown cursor session {}", session_id.0))?
        };

        let mut state = slot.lock();

        if state.lifecycle_terminal {
            return Ok(vec![]);
        }

        let mut out = Vec::new();
        if !state.emitted_boot {
            state.emitted_boot = true;
            out.push(RuntimeEvent::Lifecycle {
                session_id: session_id.clone(),
                lifecycle: AgentLifecycle::Starting,
            });
            out.push(RuntimeEvent::Lifecycle {
                session_id: session_id.clone(),
                lifecycle: AgentLifecycle::Running,
            });
        }

        drain_child_stdout(&mut state, session_id, &mut out)?;
        if let WaitOutcome::Exited(ex) = state.spawned.process.try_wait_exit()? {
            state.lifecycle_terminal = true;
            out.push(RuntimeEvent::Lifecycle {
                session_id: session_id.clone(),
                lifecycle: if ex.is_success() {
                    AgentLifecycle::Completed
                } else {
                    AgentLifecycle::Failed
                },
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

    fn stop(&self, session_id: &AgentSessionId) -> anyhow::Result<()> {
        let slot = {
            let mut g = self.inner.lock();
            g.sessions
                .remove(session_id)
                .ok_or_else(|| anyhow::anyhow!("unknown cursor session {}", session_id.0))?
        };

        let process = {
            let session = slot.lock();
            session.spawned.process.clone()
        };
        process
            .kill()
            .context("cursor stop: failed to kill child process")?;
        Ok(())
    }
}

impl CursorProvider {
    fn write_command(
        &self,
        session_id: &AgentSessionId,
        command: &str,
    ) -> anyhow::Result<Vec<RuntimeEvent>> {
        let slot = {
            let g = self.inner.lock();
            g.sessions
                .get(session_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("unknown cursor session {}", session_id.0))?
        };
        let mut state = slot.lock();
        let mut out = Vec::new();
        emit_boot_events_if_needed(&mut state, session_id, &mut out);
        state
            .spawned
            .process
            .write_all(command.as_bytes())
            .context("cursor command write failed")?;
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

fn drain_child_stdout(
    session: &mut CursorSession,
    session_id: &AgentSessionId,
    out: &mut Vec<RuntimeEvent>,
) -> anyhow::Result<()> {
    let mut chunk = [0u8; 4096];
    loop {
        match session.spawned.reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => session.buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(anyhow::anyhow!("read cursor stdout: {e}")),
        }
    }

    while let Some(pos) = session.buf.iter().position(|byte| *byte == b'\n') {
        let line_bytes = session.buf.drain(..=pos).collect::<Vec<_>>();
        let mut line =
            String::from_utf8_lossy(&line_bytes[..line_bytes.len().saturating_sub(1)]).into_owned();
        if line.ends_with('\r') {
            line.pop();
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(events) = parse_axis_output_line(trimmed, session_id) {
            out.extend(events);
        } else {
            out.push(RuntimeEvent::Status {
                session_id: session_id.clone(),
                message: trimmed.to_string(),
            });
        }
    }

    Ok(())
}

fn emit_boot_events_if_needed(
    session: &mut CursorSession,
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
