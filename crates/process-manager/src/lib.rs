use anyhow::{Context, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessSpec {
    pub argv: Vec<String>,
}

impl ProcessSpec {
    pub fn new(argv: impl Into<Vec<String>>) -> Self {
        Self { argv: argv.into() }
    }

    pub fn login_shell() -> Self {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let quoted_shell = shell.replace('\'', "'\"'\"'");
        Self {
            argv: vec![
                shell,
                "-lc".to_string(),
                format!(
                    "printf '\\r\\n\\033[38;2;127;138;148m[canvas]\\033[0m \\033[1;38;2;229;154;73mghostty-vt\\033[0m \\033[38;2;124;199;255mcolor\\033[0m \\033[4;38;2;119;209;154mcheck\\033[0m \\033[48;2;56;72;88;38;2;220;226;232m ok \\033[0m\\r\\n\\r\\n'; exec '{}' -l",
                    quoted_shell
                ),
            ],
        }
    }

    pub fn agent_shell() -> Self {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let quoted_shell = shell.replace('\'', "'\"'\"'");

        Self {
            argv: vec![
                shell,
                "-lc".to_string(),
                format!(
                    "printf '\\r\\n[canvas] agent pane preset\\r\\n'; printf '\\033[1;38;2;229;154;73mghostty-vt\\033[0m  \\033[38;2;124;199;255mcolor\\033[0m  \\033[4;38;2;119;209;154mdemo\\033[0m  \\033[48;2;56;72;88;38;2;220;226;232m background \\033[0m\\r\\n\\r\\n'; exec '{}' -l",
                    quoted_shell
                ),
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalGridSize {
    pub cols: u16,
    pub rows: u16,
}

impl TerminalGridSize {
    pub const fn new(cols: u16, rows: u16) -> Self {
        Self { cols, rows }
    }

    pub const fn to_pty_size(self) -> PtySize {
        PtySize {
            rows: self.rows,
            cols: self.cols,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

#[derive(Clone)]
pub struct RunningProcess {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
}

pub struct SpawnedProcess {
    pub process: RunningProcess,
    pub reader: Box<dyn Read + Send>,
}

impl RunningProcess {
    pub fn write_all(&self, bytes: &[u8]) -> Result<()> {
        let mut writer = self.writer.lock().expect("terminal writer mutex poisoned");
        writer
            .write_all(bytes)
            .context("failed to write to terminal PTY")?;
        writer.flush().context("failed to flush terminal PTY")?;
        Ok(())
    }

    pub fn resize(&self, grid: TerminalGridSize) -> Result<()> {
        self.master
            .lock()
            .expect("terminal master mutex poisoned")
            .resize(grid.to_pty_size())
            .context("failed to resize terminal PTY")
    }

    pub fn kill(&self) -> Result<()> {
        self.child
            .lock()
            .expect("terminal child mutex poisoned")
            .kill()
            .context("failed to terminate PTY child process")
    }

    pub fn try_wait_status(&self) -> Result<Option<String>> {
        let status = self
            .child
            .lock()
            .expect("terminal child mutex poisoned")
            .try_wait()
            .context("failed to query PTY child status")?;

        Ok(status.map(|status| {
            if let Some(signal) = status.signal() {
                format!("Exited via {signal}")
            } else {
                format!("Exited with code {}", status.exit_code())
            }
        }))
    }
}

pub fn spawn_process(spec: &ProcessSpec, grid: TerminalGridSize) -> Result<SpawnedProcess> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(grid.to_pty_size())
        .context("failed to open native PTY")?;
    let mut command = if spec.argv.is_empty() {
        CommandBuilder::new_default_prog()
    } else {
        CommandBuilder::from_argv(spec.argv.iter().map(OsString::from).collect())
    };
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    command.env("CLICOLOR", "1");
    command.env("CLICOLOR_FORCE", "1");
    command.env("FORCE_COLOR", "1");

    let child = pair
        .slave
        .spawn_command(command)
        .context("failed to spawn PTY child process")?;
    let reader = pair
        .master
        .try_clone_reader()
        .context("failed to clone PTY reader")?;
    let writer = pair
        .master
        .take_writer()
        .context("failed to acquire PTY writer")?;

    Ok(SpawnedProcess {
        process: RunningProcess {
            master: Arc::new(Mutex::new(pair.master)),
            writer: Arc::new(Mutex::new(writer)),
            child: Arc::new(Mutex::new(child)),
        },
        reader,
    })
}
