use crate::transcript_store::TranscriptStore;
use anyhow::Result;
use axis_core::terminal::TerminalSessionId;
use axis_terminal::{HostReadEvent, TerminalGridSize, TerminalHostSession};
use process_manager::ProcessLaunchSpec;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

/// Daemon-owned PTY session that continuously appends PTY output to the transcript store.
#[allow(dead_code)]
#[derive(Clone)]
pub struct DaemonPtySession {
    session_id: TerminalSessionId,
    host: TerminalHostSession,
    closed: Arc<AtomicBool>,
    status: Arc<Mutex<Option<String>>>,
}

#[allow(dead_code)]
impl DaemonPtySession {
    pub fn spawn(
        session_id: TerminalSessionId,
        spec: &ProcessLaunchSpec,
        grid: TerminalGridSize,
        transcripts: TranscriptStore,
    ) -> Result<Self> {
        let host = TerminalHostSession::spawn_launch(spec, grid)?;
        let closed = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(Some("Running".to_string())));
        let thread_closed = Arc::clone(&closed);
        let thread_status = Arc::clone(&status);
        let thread_session_id = session_id.clone();

        host.spawn_reader_thread(format!("axisd-pty-{}", thread_session_id.0), move |event| {
            match event {
                HostReadEvent::Bytes(bytes) => {
                    if let Err(error) = transcripts.append(&thread_session_id, &bytes) {
                        thread_closed.store(true, Ordering::SeqCst);
                        *thread_status.lock().expect("pty host status mutex poisoned") =
                            Some(format!("transcript append failed: {error}"));
                    }
                }
                HostReadEvent::Closed(message) | HostReadEvent::Error(message) => {
                    thread_closed.store(true, Ordering::SeqCst);
                    *thread_status.lock().expect("pty host status mutex poisoned") = Some(message);
                }
            }
        })?;

        Ok(Self {
            session_id,
            host,
            closed,
            status,
        })
    }

    pub fn session_id(&self) -> &TerminalSessionId {
        &self.session_id
    }

    pub fn closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub fn status(&self) -> Option<String> {
        self.status
            .lock()
            .expect("pty host status mutex poisoned")
            .clone()
    }

    pub fn send_bytes(&self, bytes: &[u8]) -> Result<()> {
        self.host.send_bytes(bytes)
    }

    pub fn resize(&self, grid: TerminalGridSize) -> Result<()> {
        self.host.resize(grid)
    }

    pub fn kill(&self) -> Result<()> {
        self.host.kill()
    }
}
