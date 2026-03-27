use crate::daemon_client::DaemonClient;
use axis_core::terminal::{TerminalSessionId, TerminalSurfaceKind};
use axis_core::SurfaceId;
use axis_terminal::{
    spawn_terminal_session_with_grid, TerminalAgentMetadata, TerminalGridSize, TerminalReplayClient,
    TerminalSession, TerminalSnapshot,
};
use axis_core::PaneKind;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};

type RemoteResult<T> = Result<T, String>;

#[derive(Clone)]
pub(crate) struct RemoteTerminalSession {
    inner: Arc<RemoteTerminalSessionInner>,
}

enum RemoteTerminalSessionInner {
    Remote(RemoteTerminalBackend),
    Local(TerminalSession),
}

struct RemoteTerminalBackend {
    daemon: DaemonClient,
    terminal_session_id: TerminalSessionId,
    replay: TerminalReplayClient,
    revision: AtomicU64,
    closed: AtomicBool,
    status: Mutex<Option<String>>,
    transcript_offset: AtomicU64,
    agent_metadata: Mutex<Option<TerminalAgentMetadata>>,
}

impl RemoteTerminalSession {
    pub fn attach_or_create(
        workdesk_id: &str,
        surface_id: SurfaceId,
        kind: &PaneKind,
        title: &str,
        cwd: &str,
        grid: TerminalGridSize,
    ) -> Result<Self, String> {
        let daemon = DaemonClient::default();
        let Some(remote_kind) = surface_kind_from_pane(kind) else {
            return Err(format!("surface kind {kind:?} does not support remote terminals"));
        };

        match daemon.ensure_terminal(workdesk_id, surface_id, remote_kind, title, cwd, grid) {
            Ok(record) => {
                let backend = RemoteTerminalBackend::new(daemon, record, grid, title)?;
                let session = Self {
                    inner: Arc::new(RemoteTerminalSessionInner::Remote(backend)),
                };
                session.sync();
                Ok(session)
            }
            Err(_error) => {
                let local = spawn_terminal_session_with_grid(kind, title, grid)
                    .map_err(|spawn_error| format!("terminal boot failed for {title}: {spawn_error}"))?;
                Ok(Self {
                    inner: Arc::new(RemoteTerminalSessionInner::Local(local)),
                })
            }
        }
    }

    pub fn sync(&self) {
        if let RemoteTerminalSessionInner::Remote(backend) = self.inner.as_ref() {
            let _ = backend.sync();
        }
    }

    pub fn set_agent_metadata(&self, meta: Option<TerminalAgentMetadata>) {
        match self.inner.as_ref() {
            RemoteTerminalSessionInner::Remote(backend) => {
                *backend
                    .agent_metadata
                    .lock()
                    .expect("remote terminal agent metadata mutex poisoned") = meta;
            }
            RemoteTerminalSessionInner::Local(local) => local.set_agent_metadata(meta),
        }
    }

    pub fn agent_metadata(&self) -> Option<TerminalAgentMetadata> {
        match self.inner.as_ref() {
            RemoteTerminalSessionInner::Remote(backend) => backend
                .agent_metadata
                .lock()
                .expect("remote terminal agent metadata mutex poisoned")
                .clone(),
            RemoteTerminalSessionInner::Local(local) => local.agent_metadata(),
        }
    }

    pub fn revision(&self) -> u64 {
        match self.inner.as_ref() {
            RemoteTerminalSessionInner::Remote(backend) => backend.revision.load(Ordering::SeqCst),
            RemoteTerminalSessionInner::Local(local) => local.revision(),
        }
    }

    pub fn status(&self) -> Option<String> {
        match self.inner.as_ref() {
            RemoteTerminalSessionInner::Remote(backend) => backend
                .status
                .lock()
                .expect("remote terminal status mutex poisoned")
                .clone(),
            RemoteTerminalSessionInner::Local(local) => local.status(),
        }
    }

    pub fn closed(&self) -> bool {
        match self.inner.as_ref() {
            RemoteTerminalSessionInner::Remote(backend) => backend.closed.load(Ordering::SeqCst),
            RemoteTerminalSessionInner::Local(local) => local.closed(),
        }
    }

    pub fn snapshot(&self) -> TerminalSnapshot {
        match self.inner.as_ref() {
            RemoteTerminalSessionInner::Remote(backend) => backend.snapshot(),
            RemoteTerminalSessionInner::Local(local) => local.snapshot(),
        }
    }

    pub fn send_bytes(&self, bytes: &[u8]) -> RemoteResult<()> {
        match self.inner.as_ref() {
            RemoteTerminalSessionInner::Remote(backend) => backend.send_bytes(bytes),
            RemoteTerminalSessionInner::Local(local) => local.send_bytes(bytes).map_err(|e| e.to_string()),
        }
    }

    pub fn send_text(&self, text: &str) -> RemoteResult<()> {
        self.send_bytes(text.as_bytes())
    }

    pub fn scroll_viewport_delta(&self, delta: isize) -> RemoteResult<()> {
        match self.inner.as_ref() {
            RemoteTerminalSessionInner::Remote(backend) => backend.scroll_viewport_delta(delta),
            RemoteTerminalSessionInner::Local(local) => {
                local.scroll_viewport_delta(delta).map_err(|e| e.to_string())
            }
        }
    }

    pub fn scroll_viewport_bottom(&self) -> RemoteResult<()> {
        match self.inner.as_ref() {
            RemoteTerminalSessionInner::Remote(backend) => backend.scroll_viewport_bottom(),
            RemoteTerminalSessionInner::Local(local) => {
                local.scroll_viewport_bottom().map_err(|e| e.to_string())
            }
        }
    }

    pub fn resize(&self, grid: TerminalGridSize) -> RemoteResult<()> {
        match self.inner.as_ref() {
            RemoteTerminalSessionInner::Remote(backend) => backend.resize(grid),
            RemoteTerminalSessionInner::Local(local) => local.resize(grid).map_err(|e| e.to_string()),
        }
    }

    pub fn close(&self) {
        match self.inner.as_ref() {
            RemoteTerminalSessionInner::Remote(backend) => backend.close(),
            RemoteTerminalSessionInner::Local(local) => local.close(),
        }
    }
}

impl RemoteTerminalBackend {
    fn new(
        daemon: DaemonClient,
        record: axis_core::terminal::TerminalSessionRecord,
        grid: TerminalGridSize,
        fallback_title: &str,
    ) -> Result<Self, String> {
        let replay =
            TerminalReplayClient::new(fallback_title, grid).map_err(|error| error.to_string())?;
        Ok(Self {
            daemon,
            terminal_session_id: record.terminal_session_id,
            replay,
            revision: AtomicU64::new(1),
            closed: AtomicBool::new(record.closed),
            status: Mutex::new(closed_status(record.closed)),
            transcript_offset: AtomicU64::new(0),
            agent_metadata: Mutex::new(None),
        })
    }

    fn sync(&self) -> Result<(), String> {
        let offset = self.transcript_offset.load(Ordering::SeqCst);
        let result = self.daemon.read_terminal(&self.terminal_session_id, offset)?;
        let mut changed = false;

        if let Some(chunk) = result.chunk {
            if !chunk.bytes.is_empty() {
                self.replay
                    .apply_bytes(&chunk.bytes)
                    .map_err(|error| self.record_sync_error(format!("ghostty-vt replay failed: {error}")))?;
                self.transcript_offset.store(
                    chunk.offset + chunk.bytes.len() as u64,
                    Ordering::SeqCst,
                );
                changed = true;
            }
        }

        changed |= self.update_record_state(&result.record);
        if changed {
            self.revision.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }

    fn snapshot(&self) -> TerminalSnapshot {
        let mut snapshot = self.replay.snapshot();
        snapshot.closed = self.closed.load(Ordering::SeqCst);
        snapshot.status = self
            .status
            .lock()
            .expect("remote terminal status mutex poisoned")
            .clone();
        snapshot
    }

    fn send_bytes(&self, bytes: &[u8]) -> RemoteResult<()> {
        self.daemon
            .write_terminal(&self.terminal_session_id, bytes)
            .map(|record| {
                let changed = self.update_record_state(&record);
                if changed {
                    self.revision.fetch_add(1, Ordering::SeqCst);
                }
            })
    }

    fn scroll_viewport_delta(&self, delta: isize) -> RemoteResult<()> {
        self.replay
            .scroll_viewport_delta(delta)
            .map_err(|error| error.to_string())?;
        self.revision.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn scroll_viewport_bottom(&self) -> RemoteResult<()> {
        self.replay
            .scroll_viewport_bottom()
            .map_err(|error| error.to_string())?;
        self.revision.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn resize(&self, grid: TerminalGridSize) -> RemoteResult<()> {
        let record = self.daemon.resize_terminal(&self.terminal_session_id, grid)?;
        self.replay
            .resize(grid)
            .map_err(|error| error.to_string())?;
        let changed = self.update_record_state(&record);
        if changed {
            self.revision.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }

    fn close(&self) {
        if let Ok(record) = self.daemon.close_terminal(&self.terminal_session_id) {
            let changed = self.update_record_state(&record);
            if changed {
                self.revision.fetch_add(1, Ordering::SeqCst);
            }
        } else {
            self.record_sync_error("remote terminal close failed".to_string());
        }
    }

    fn update_record_state(&self, record: &axis_core::terminal::TerminalSessionRecord) -> bool {
        let mut changed = false;
        let closed = record.closed;
        if self.closed.swap(closed, Ordering::SeqCst) != closed {
            changed = true;
        }

        let next_status = closed_status(closed);
        let mut status = self
            .status
            .lock()
            .expect("remote terminal status mutex poisoned");
        if *status != next_status {
            *status = next_status;
            changed = true;
        }

        changed
    }

    fn record_sync_error(&self, message: String) -> String {
        self.closed.store(true, Ordering::SeqCst);
        *self
            .status
            .lock()
            .expect("remote terminal status mutex poisoned") = Some(message.clone());
        self.revision.fetch_add(1, Ordering::SeqCst);
        message
    }
}

fn surface_kind_from_pane(kind: &PaneKind) -> Option<TerminalSurfaceKind> {
    match kind {
        PaneKind::Shell => Some(TerminalSurfaceKind::Shell),
        PaneKind::Agent => Some(TerminalSurfaceKind::Agent),
        PaneKind::Browser | PaneKind::Editor => None,
    }
}

fn closed_status(closed: bool) -> Option<String> {
    closed.then(|| "Terminated".to_string())
}
