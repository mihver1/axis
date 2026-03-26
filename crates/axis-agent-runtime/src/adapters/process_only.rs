//! Generic process-backed adapter with lifecycle/status only and no structured attention markers.

use std::collections::HashMap;
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use axis_core::agent::{AgentLifecycle, AgentSessionId};

use crate::events::RuntimeEvent;
use crate::provider::{AgentProvider, StartAgentRequest, StartedSession};
use process_manager::{spawn_process_launch, ProcessLaunchSpec, TerminalGridSize, WaitOutcome};

const DEFAULT_GRID: TerminalGridSize = TerminalGridSize::new(80, 24);

pub struct ProcessOnlyProvider {
    profile_id: String,
    base_argv: Vec<String>,
    inner: Mutex<ProcessOnlyInner>,
}

struct ProcessOnlyInner {
    next_id: u64,
    sessions: HashMap<AgentSessionId, Arc<Mutex<ProcessOnlySession>>>,
}

struct ProcessOnlySession {
    spawned: process_manager::SpawnedProcess,
    buf: Vec<u8>,
    emitted_boot: bool,
    lifecycle_terminal: bool,
}

impl ProcessOnlyProvider {
    pub fn new(profile_id: impl Into<String>) -> Self {
        let profile_id = profile_id.into();
        Self::with_base_argv(profile_id.clone(), vec![profile_id])
    }

    pub fn with_base_argv(profile_id: impl Into<String>, base_argv: Vec<String>) -> Self {
        Self {
            profile_id: profile_id.into(),
            base_argv,
            inner: Mutex::new(ProcessOnlyInner {
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

impl AgentProvider for ProcessOnlyProvider {
    fn start(&self, req: StartAgentRequest) -> anyhow::Result<StartedSession> {
        let launch = ProcessLaunchSpec {
            argv: self.build_argv(&req),
            cwd: Some(PathBuf::from(&req.cwd)),
            env: req.env.clone(),
            use_pty: false,
        };
        let spawned = spawn_process_launch(&launch, DEFAULT_GRID)
            .map_err(|e| anyhow::anyhow!("{} spawn failed: {e:#}", self.profile_id))?;

        let slot = Arc::new(Mutex::new(ProcessOnlySession {
            spawned,
            buf: Vec::new(),
            emitted_boot: false,
            lifecycle_terminal: false,
        }));

        let mut guard = self
            .inner
            .lock()
            .map_err(|e| anyhow::anyhow!("{} provider lock poisoned: {e}", self.profile_id))?;
        let id = AgentSessionId::new(format!("{}-session-{}", self.profile_id, guard.next_id));
        guard.next_id += 1;
        guard.sessions.insert(id.clone(), slot);
        Ok(StartedSession { session_id: id })
    }

    fn poll_events(&self, session_id: &AgentSessionId) -> anyhow::Result<Vec<RuntimeEvent>> {
        let slot = {
            let guard = self
                .inner
                .lock()
                .map_err(|e| anyhow::anyhow!("{} provider lock poisoned: {e}", self.profile_id))?;
            guard
                .sessions
                .get(session_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("unknown {} session {}", self.profile_id, session_id.0))?
        };

        let mut session = slot
            .lock()
            .map_err(|e| anyhow::anyhow!("{} session lock poisoned: {e}", self.profile_id))?;

        if session.lifecycle_terminal {
            return Ok(vec![]);
        }

        let mut out = Vec::new();
        if !session.emitted_boot {
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

        drain_child_stdout(&mut session, session_id, &mut out)?;
        if let WaitOutcome::Exited(exit) = session.spawned.process.try_wait_exit()? {
            session.lifecycle_terminal = true;
            out.push(RuntimeEvent::Lifecycle {
                session_id: session_id.clone(),
                lifecycle: if exit.is_success() {
                    AgentLifecycle::Completed
                } else {
                    AgentLifecycle::Failed
                },
            });
        }

        Ok(out)
    }

    fn stop(&self, session_id: &AgentSessionId) -> anyhow::Result<()> {
        let slot = {
            let mut guard = self
                .inner
                .lock()
                .map_err(|e| anyhow::anyhow!("{} provider lock poisoned: {e}", self.profile_id))?;
            guard
                .sessions
                .remove(session_id)
                .ok_or_else(|| anyhow::anyhow!("unknown {} session {}", self.profile_id, session_id.0))?
        };

        let process = {
            let session = slot
                .lock()
                .map_err(|e| anyhow::anyhow!("{} session lock poisoned: {e}", self.profile_id))?;
            session.spawned.process.clone()
        };
        process
            .kill()
            .with_context(|| format!("{} stop: failed to kill child process", self.profile_id))?;
        Ok(())
    }
}

fn drain_child_stdout(
    session: &mut ProcessOnlySession,
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
            Err(e) => return Err(anyhow::anyhow!("read process-only stdout: {e}")),
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
        out.push(RuntimeEvent::Status {
            session_id: session_id.clone(),
            message: trimmed.to_string(),
        });
    }

    Ok(())
}
