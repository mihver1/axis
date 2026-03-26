use anyhow::{Context, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child as StdChild, Command as StdCommand, Stdio};
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
                    "printf '\\r\\n\\033[38;2;127;138;148m[axis]\\033[0m \\033[1;38;2;229;154;73mghostty-vt\\033[0m \\033[38;2;124;199;255mcolor\\033[0m \\033[4;38;2;119;209;154mcheck\\033[0m \\033[48;2;56;72;88;38;2;220;226;232m ok \\033[0m\\r\\n\\r\\n'; exec '{}' -l",
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
                    "printf '\\r\\n[axis] agent pane preset\\r\\n'; printf '\\033[1;38;2;229;154;73mghostty-vt\\033[0m  \\033[38;2;124;199;255mcolor\\033[0m  \\033[4;38;2;119;209;154mdemo\\033[0m  \\033[48;2;56;72;88;38;2;220;226;232m background \\033[0m\\r\\n\\r\\n'; exec '{}' -l",
                    quoted_shell
                ),
            ],
        }
    }
}

/// Full process launch description (cwd, env, PTY vs stdio).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessLaunchSpec {
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    pub use_pty: bool,
}

impl ProcessLaunchSpec {
    pub fn new(argv: impl Into<Vec<String>>) -> Self {
        Self {
            argv: argv.into(),
            cwd: None,
            env: BTreeMap::new(),
            use_pty: true,
        }
    }
}

/// Merges environment layers: keys in `overrides` replace those in `base`.
pub fn merge_string_env(
    base: &BTreeMap<String, String>,
    overrides: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut out = base.clone();
    for (k, v) in overrides {
        out.insert(k.clone(), v.clone());
    }
    out
}

/// Pure resolution of cwd and merged env for a launch (no spawn). Same rules used by PTY and stdio spawners.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedProcessLaunch {
    /// Working directory passed to the child, if any (`std::process::Command::current_dir` / PTY `cwd`).
    pub cwd: Option<PathBuf>,
    /// Default terminal-related variables merged with [`ProcessLaunchSpec::env`] (spec wins on key clash).
    pub merged_env: BTreeMap<String, String>,
}

/// Computes [`ResolvedProcessLaunch`] from a spec so cwd/env behavior is explicit and unit-testable.
pub fn resolve_process_launch(spec: &ProcessLaunchSpec) -> ResolvedProcessLaunch {
    ResolvedProcessLaunch {
        cwd: spec.cwd.clone(),
        merged_env: merge_string_env(&default_terminal_env(), &spec.env),
    }
}

fn default_terminal_env() -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert("TERM".into(), "xterm-256color".into());
    m.insert("COLORTERM".into(), "truecolor".into());
    m.insert("CLICOLOR".into(), "1".into());
    m.insert("CLICOLOR_FORCE".into(), "1".into());
    m.insert("FORCE_COLOR".into(), "1".into());
    m
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

enum RunningProcessInner {
    Pty {
        master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
        writer: Arc<Mutex<Box<dyn Write + Send>>>,
        child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    },
    Stdio {
        writer: Arc<Mutex<Box<dyn Write + Send>>>,
        child: Arc<Mutex<StdChild>>,
    },
}

#[derive(Clone)]
pub struct RunningProcess {
    inner: Arc<RunningProcessInner>,
}

/// Structured child exit (replaces stringly status parsing for callers that need logic).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessExit {
    pub success: bool,
    pub code: Option<i32>,
    /// Unix signal number when terminated by signal (stdio children).
    pub signal: Option<i32>,
    /// Human-readable signal (e.g. PTY layer) when no numeric signal is available.
    pub signal_note: Option<String>,
}

impl ProcessExit {
    pub fn is_success(&self) -> bool {
        self.success
    }
}

/// Result of a non-blocking wait on the child process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaitOutcome {
    StillRunning,
    Exited(ProcessExit),
}

pub struct SpawnedProcess {
    pub process: RunningProcess,
    pub reader: Box<dyn Read + Send>,
}

impl RunningProcess {
    pub fn write_all(&self, bytes: &[u8]) -> Result<()> {
        match self.inner.as_ref() {
            RunningProcessInner::Pty { writer, .. } | RunningProcessInner::Stdio { writer, .. } => {
                let mut w = writer.lock().expect("process writer mutex poisoned");
                w.write_all(bytes).context("failed to write to process stdin")?;
                w.flush().context("failed to flush process stdin")?;
                Ok(())
            }
        }
    }

    pub fn resize(&self, grid: TerminalGridSize) -> Result<()> {
        match self.inner.as_ref() {
            RunningProcessInner::Pty { master, .. } => master
                .lock()
                .expect("terminal master mutex poisoned")
                .resize(grid.to_pty_size())
                .context("failed to resize terminal PTY"),
            RunningProcessInner::Stdio { .. } => {
                anyhow::bail!("resize is not supported for non-PTY processes")
            }
        }
    }

    pub fn kill(&self) -> Result<()> {
        match self.inner.as_ref() {
            RunningProcessInner::Pty { child, .. } => child
                .lock()
                .expect("terminal child mutex poisoned")
                .kill()
                .context("failed to terminate PTY child process"),
            RunningProcessInner::Stdio { child, .. } => {
                let mut c = child.lock().expect("stdio child mutex poisoned");
                c.kill().context("failed to terminate stdio child process")?;
                Ok(())
            }
        }
    }

    pub fn try_wait_exit(&self) -> Result<WaitOutcome> {
        match self.inner.as_ref() {
            RunningProcessInner::Pty { child, .. } => {
                let status = child
                    .lock()
                    .expect("terminal child mutex poisoned")
                    .try_wait()
                    .context("failed to query PTY child status")?;

                Ok(match status {
                    None => WaitOutcome::StillRunning,
                    Some(s) => WaitOutcome::Exited(process_exit_from_portable(s)),
                })
            }
            RunningProcessInner::Stdio { child, .. } => {
                let status = child
                    .lock()
                    .expect("stdio child mutex poisoned")
                    .try_wait()
                    .context("failed to query stdio child status")?;

                Ok(match status {
                    None => WaitOutcome::StillRunning,
                    Some(s) => WaitOutcome::Exited(process_exit_from_std(s)),
                })
            }
        }
    }

    /// Human-readable exit line for UI (legacy); prefer [`Self::try_wait_exit`] for logic.
    pub fn try_wait_status(&self) -> Result<Option<String>> {
        Ok(match self.try_wait_exit()? {
            WaitOutcome::StillRunning => None,
            WaitOutcome::Exited(e) => Some(format_process_exit(&e)),
        })
    }
}

fn process_exit_from_portable(status: portable_pty::ExitStatus) -> ProcessExit {
    if let Some(note) = status.signal() {
        ProcessExit {
            success: false,
            code: Some(status.exit_code() as i32),
            signal: None,
            signal_note: Some(note.to_string()),
        }
    } else {
        let code = status.exit_code() as i32;
        ProcessExit {
            success: status.success(),
            code: Some(code),
            signal: None,
            signal_note: None,
        }
    }
}

fn process_exit_from_std(status: std::process::ExitStatus) -> ProcessExit {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return ProcessExit {
                success: false,
                code: status.code(),
                signal: Some(signal),
                signal_note: None,
            };
        }
    }
    ProcessExit {
        success: status.success(),
        code: status.code(),
        signal: None,
        signal_note: None,
    }
}

fn format_process_exit(e: &ProcessExit) -> String {
    if let Some(note) = &e.signal_note {
        return format!("Exited via {note}");
    }
    if let Some(sig) = e.signal {
        return format!("Exited via {sig}");
    }
    format!("Exited with code {}", e.code.unwrap_or(-1))
}

fn build_pty_command(spec: &ProcessLaunchSpec) -> Result<CommandBuilder> {
    let resolved = resolve_process_launch(spec);
    let mut command = if spec.argv.is_empty() {
        CommandBuilder::new_default_prog()
    } else {
        CommandBuilder::from_argv(spec.argv.iter().map(OsString::from).collect())
    };
    if let Some(cwd) = &resolved.cwd {
        command.cwd(cwd);
    }
    for (k, v) in resolved.merged_env {
        command.env(k, v);
    }
    Ok(command)
}

#[cfg(unix)]
fn set_stdout_nonblocking(stdout: &mut std::process::ChildStdout) -> Result<()> {
    use std::os::unix::io::AsRawFd;
    let fd = stdout.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(anyhow::anyhow!("fcntl F_GETFL failed"));
    }
    let r = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if r < 0 {
        return Err(anyhow::anyhow!("fcntl F_SETFL O_NONBLOCK failed"));
    }
    Ok(())
}

fn spawn_pty_process(spec: &ProcessLaunchSpec, grid: TerminalGridSize) -> Result<SpawnedProcess> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(grid.to_pty_size())
        .context("failed to open native PTY")?;
    let command = build_pty_command(spec).context("failed to build PTY command")?;

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
            inner: Arc::new(RunningProcessInner::Pty {
                master: Arc::new(Mutex::new(pair.master)),
                writer: Arc::new(Mutex::new(writer)),
                child: Arc::new(Mutex::new(child)),
            }),
        },
        reader: Box::new(reader),
    })
}

fn spawn_stdio_process(spec: &ProcessLaunchSpec) -> Result<SpawnedProcess> {
    anyhow::ensure!(
        !spec.argv.is_empty(),
        "stdio launch requires a non-empty argv"
    );
    let resolved = resolve_process_launch(spec);
    let prog = &spec.argv[0];
    let mut cmd = StdCommand::new(prog);
    cmd.args(&spec.argv[1..]);
    if let Some(cwd) = &resolved.cwd {
        cmd.current_dir(cwd);
    }
    for (k, v) in &resolved.merged_env {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::inherit());

    let mut child = cmd.spawn().context("failed to spawn stdio child process")?;
    let stdout = child
        .stdout
        .take()
        .context("failed to capture child stdout")?;
    let stdin = child
        .stdin
        .take()
        .context("failed to capture child stdin")?;

    let mut stdout = stdout;
    #[cfg(unix)]
    set_stdout_nonblocking(&mut stdout).context("failed to set non-blocking stdout")?;

    Ok(SpawnedProcess {
        process: RunningProcess {
            inner: Arc::new(RunningProcessInner::Stdio {
                writer: Arc::new(Mutex::new(Box::new(stdin))),
                child: Arc::new(Mutex::new(child)),
            }),
        },
        reader: Box::new(stdout),
    })
}

/// Spawns using [`ProcessLaunchSpec`]: PTY when `use_pty`, otherwise piped stdio (non-blocking stdout on Unix).
pub fn spawn_process_launch(spec: &ProcessLaunchSpec, grid: TerminalGridSize) -> Result<SpawnedProcess> {
    if spec.use_pty {
        spawn_pty_process(spec, grid)
    } else {
        spawn_stdio_process(spec)
    }
}

pub fn spawn_process(spec: &ProcessSpec, grid: TerminalGridSize) -> Result<SpawnedProcess> {
    let launch = ProcessLaunchSpec {
        argv: spec.argv.clone(),
        cwd: None,
        env: BTreeMap::new(),
        use_pty: true,
    };
    spawn_process_launch(&launch, grid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn merge_string_env_overrides_base_keys() {
        let mut base = BTreeMap::new();
        base.insert("A".into(), "1".into());
        base.insert("B".into(), "keep".into());
        let mut over = BTreeMap::new();
        over.insert("A".into(), "2".into());
        over.insert("C".into(), "3".into());
        let m = merge_string_env(&base, &over);
        assert_eq!(m.get("A").map(String::as_str), Some("2"));
        assert_eq!(m.get("B").map(String::as_str), Some("keep"));
        assert_eq!(m.get("C").map(String::as_str), Some("3"));
    }

    #[test]
    fn resolve_process_launch_cwd_none_by_default() {
        let spec = ProcessLaunchSpec::new(vec!["prog".to_string()]);
        let r = resolve_process_launch(&spec);
        assert!(r.cwd.is_none());
    }

    #[test]
    fn resolve_process_launch_cwd_passthrough() {
        let mut spec = ProcessLaunchSpec::new(vec!["a".to_string()]);
        spec.cwd = Some(PathBuf::from("/tmp/work"));
        let r = resolve_process_launch(&spec);
        assert_eq!(r.cwd.as_deref(), Some(Path::new("/tmp/work")));
    }

    #[test]
    fn resolve_process_launch_merges_default_terminal_env_and_spec_env() {
        let mut spec = ProcessLaunchSpec::new(vec!["x".to_string()]);
        spec.env.insert("TERM".into(), "dumb".into());
        spec.env.insert("AXIS_TEST".into(), "1".into());
        let r = resolve_process_launch(&spec);
        assert_eq!(r.merged_env.get("TERM").map(String::as_str), Some("dumb"));
        assert_eq!(r.merged_env.get("AXIS_TEST").map(String::as_str), Some("1"));
        assert!(r.merged_env.contains_key("COLORTERM"));
        assert!(r.merged_env.contains_key("FORCE_COLOR"));
    }

    #[cfg(unix)]
    #[test]
    fn process_exit_from_std_success() {
        let st = std::process::Command::new("true").status().unwrap();
        let e = process_exit_from_std(st);
        assert!(e.is_success());
        assert_eq!(e.code, Some(0));
        assert!(e.signal.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn process_exit_from_std_failure_code() {
        let st = std::process::Command::new("false").status().unwrap();
        let e = process_exit_from_std(st);
        assert!(!e.is_success());
    }
}
