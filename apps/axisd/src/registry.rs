use crate::pty_host::DaemonPtySession;
use crate::transcript_store::TranscriptStore;
use anyhow::{anyhow, Result};
use axis_core::terminal::{
    TerminalSessionId, TerminalSessionRecord, TerminalSurfaceKind, TerminalTranscriptChunk,
};
use axis_core::workdesk::{WorkdeskId, WorkdeskRecord};
use axis_core::SurfaceId;
use axis_terminal::TerminalGridSize;
use process_manager::{ProcessLaunchSpec, ProcessSpec};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Persisted daemon-owned registry of known workdesks.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct DaemonRegistry {
    workdesks: HashMap<String, WorkdeskRecord>,
}

impl DaemonRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn workdesk_count(&self) -> usize {
        self.workdesks.len()
    }

    pub fn ensure_workdesk(&mut self, record: WorkdeskRecord) -> WorkdeskRecord {
        self.workdesks
            .insert(record.workdesk_id.0.clone(), record.clone());
        record
    }

    pub fn list_workdesks(&self, workspace_root: Option<&str>) -> Vec<WorkdeskRecord> {
        let mut records = self
            .workdesks
            .values()
            .filter(|record| {
                workspace_root
                    .map(|root| record.workspace_root == root)
                    .unwrap_or(true)
            })
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| left.workdesk_id.0.cmp(&right.workdesk_id.0));
        records
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct TerminalSurfaceKey {
    workdesk_id: String,
    surface_id: u64,
}

struct DaemonTerminalSession {
    record: TerminalSessionRecord,
    pty: DaemonPtySession,
}

pub struct TerminalRegistry {
    next_session_serial: u64,
    sessions_by_key: HashMap<TerminalSurfaceKey, DaemonTerminalSession>,
    session_key_by_id: HashMap<String, TerminalSurfaceKey>,
    transcripts: TranscriptStore,
}

impl TerminalRegistry {
    pub fn new(transcripts: TranscriptStore) -> Self {
        Self {
            next_session_serial: 1,
            sessions_by_key: HashMap::new(),
            session_key_by_id: HashMap::new(),
            transcripts,
        }
    }

    pub fn ensure_session(
        &mut self,
        workdesk_id: &WorkdeskId,
        surface_id: SurfaceId,
        kind: TerminalSurfaceKind,
        title: String,
        cwd: Option<String>,
        grid: TerminalGridSize,
    ) -> Result<TerminalSessionRecord> {
        let key = TerminalSurfaceKey {
            workdesk_id: workdesk_id.0.clone(),
            surface_id: surface_id.raw(),
        };

        if let Some(session) = self.sessions_by_key.get_mut(&key) {
            let session_id = {
                session.record.kind = kind;
                session.record.title = title;
                if let Some(cwd) = cwd {
                    session.record.cwd = cwd;
                }
                session.record.cols = grid.cols;
                session.record.rows = grid.rows;
                session.pty.resize(grid)?;
                session.record.terminal_session_id.clone()
            };
            return self.refresh_record(&session_id);
        }

        let session_id = TerminalSessionId::new(format!("term-{}", self.next_session_serial));
        self.next_session_serial = self.next_session_serial.saturating_add(1);
        let cwd = cwd.unwrap_or_default();
        let launch = terminal_launch_spec(kind, &cwd);
        let pty = DaemonPtySession::spawn(session_id.clone(), &launch, grid, self.transcripts.clone())?;

        let mut record = TerminalSessionRecord {
            terminal_session_id: session_id.clone(),
            workdesk_id: workdesk_id.clone(),
            surface_id,
            kind,
            title,
            cwd,
            cols: grid.cols,
            rows: grid.rows,
            transcript_len: 0,
            closed: false,
        };
        record.transcript_len = self.transcripts.len(&session_id)?;
        record.closed = pty.closed();

        self.session_key_by_id
            .insert(session_id.0.clone(), key.clone());
        self.sessions_by_key
            .insert(key, DaemonTerminalSession { record: record.clone(), pty });
        Ok(record)
    }

    pub fn read_from(
        &mut self,
        session_id: &TerminalSessionId,
        offset: u64,
    ) -> Result<(TerminalSessionRecord, Option<TerminalTranscriptChunk>)> {
        let record = self.refresh_record(session_id)?;
        let chunk = self.transcripts.read_from(session_id, offset)?;
        Ok((record, chunk))
    }

    pub fn write_bytes(
        &mut self,
        session_id: &TerminalSessionId,
        bytes: &[u8],
    ) -> Result<TerminalSessionRecord> {
        let key = self.session_key(session_id)?;
        self.sessions_by_key
            .get(&key)
            .ok_or_else(|| anyhow!("terminal session `{}` was not found", session_id.0))?
            .pty
            .send_bytes(bytes)?;
        self.refresh_record(session_id)
    }

    pub fn resize(
        &mut self,
        session_id: &TerminalSessionId,
        grid: TerminalGridSize,
    ) -> Result<TerminalSessionRecord> {
        let key = self.session_key(session_id)?;
        let session = self
            .sessions_by_key
            .get_mut(&key)
            .ok_or_else(|| anyhow!("terminal session `{}` was not found", session_id.0))?;
        session.pty.resize(grid)?;
        session.record.cols = grid.cols;
        session.record.rows = grid.rows;
        self.refresh_record(session_id)
    }

    pub fn close(&mut self, session_id: &TerminalSessionId) -> Result<TerminalSessionRecord> {
        let key = self.session_key(session_id)?;
        self.sessions_by_key
            .get(&key)
            .ok_or_else(|| anyhow!("terminal session `{}` was not found", session_id.0))?
            .pty
            .kill()?;
        self.refresh_record(session_id)
    }

    fn refresh_record(&mut self, session_id: &TerminalSessionId) -> Result<TerminalSessionRecord> {
        let key = self.session_key(session_id)?;
        let transcript_len = self.transcripts.len(session_id)?;
        let session = self
            .sessions_by_key
            .get_mut(&key)
            .ok_or_else(|| anyhow!("terminal session `{}` was not found", session_id.0))?;
        session.record.transcript_len = transcript_len;
        session.record.closed = session.pty.closed();
        Ok(session.record.clone())
    }

    fn session_key(&self, session_id: &TerminalSessionId) -> Result<TerminalSurfaceKey> {
        self.session_key_by_id
            .get(&session_id.0)
            .cloned()
            .ok_or_else(|| anyhow!("terminal session `{}` was not found", session_id.0))
    }
}

fn terminal_launch_spec(kind: TerminalSurfaceKind, cwd: &str) -> ProcessLaunchSpec {
    let process = match kind {
        TerminalSurfaceKind::Shell => ProcessSpec::login_shell(),
        TerminalSurfaceKind::Agent => ProcessSpec::agent_shell(),
    };
    let mut launch = ProcessLaunchSpec::new(process.argv);
    if !cwd.trim().is_empty() {
        launch.cwd = Some(PathBuf::from(cwd));
    }
    launch
}
