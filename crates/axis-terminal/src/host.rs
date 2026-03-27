use anyhow::{anyhow, Context, Result};
use process_manager::{
    spawn_process, spawn_process_launch, ProcessLaunchSpec, ProcessSpec, RunningProcess,
    TerminalGridSize,
};
use std::io::Read;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostReadEvent {
    Bytes(Vec<u8>),
    Closed(String),
    Error(String),
}

/// PTY-owning side of a terminal session. Intended for daemon-side runtime ownership.
#[derive(Clone)]
pub struct TerminalHostSession {
    process: RunningProcess,
    reader: Arc<Mutex<Option<Box<dyn Read + Send>>>>,
}

impl TerminalHostSession {
    pub fn spawn(spec: &ProcessSpec, grid: TerminalGridSize) -> Result<Self> {
        let spawned = spawn_process(spec, grid).context("failed to boot PTY process")?;
        Ok(Self {
            process: spawned.process,
            reader: Arc::new(Mutex::new(Some(spawned.reader))),
        })
    }

    pub fn spawn_launch(spec: &ProcessLaunchSpec, grid: TerminalGridSize) -> Result<Self> {
        let spawned =
            spawn_process_launch(spec, grid).context("failed to boot PTY launch process")?;
        Ok(Self {
            process: spawned.process,
            reader: Arc::new(Mutex::new(Some(spawned.reader))),
        })
    }

    pub fn spawn_reader_thread<F>(
        &self,
        thread_name: impl Into<String>,
        mut on_event: F,
    ) -> Result<()>
    where
        F: FnMut(HostReadEvent) + Send + 'static,
    {
        let Some(mut reader) = self
            .reader
            .lock()
            .expect("terminal host reader mutex poisoned")
            .take()
        else {
            return Err(anyhow!("terminal host reader thread already started"));
        };
        let process = self.process.clone();

        std::thread::Builder::new()
            .name(thread_name.into())
            .spawn(move || {
                let mut buffer = [0u8; 8192];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => {
                            let status_text = process
                                .try_wait_status()
                                .ok()
                                .flatten()
                                .unwrap_or_else(|| "PTY stream closed".to_string());
                            on_event(HostReadEvent::Closed(status_text));
                            break;
                        }
                        Ok(bytes_read) => {
                            on_event(HostReadEvent::Bytes(buffer[..bytes_read].to_vec()))
                        }
                        Err(error) => {
                            on_event(HostReadEvent::Error(format!("PTY read error: {error}")));
                            break;
                        }
                    }
                }
            })
            .context("failed to spawn PTY reader thread")?;

        Ok(())
    }

    pub fn send_bytes(&self, bytes: &[u8]) -> Result<()> {
        self.process.write_all(bytes)
    }

    pub fn resize(&self, grid: TerminalGridSize) -> Result<()> {
        self.process.resize(grid)
    }

    pub fn kill(&self) -> Result<()> {
        self.process.kill()
    }

    pub fn try_wait_status(&self) -> Result<Option<String>> {
        self.process.try_wait_status()
    }
}
