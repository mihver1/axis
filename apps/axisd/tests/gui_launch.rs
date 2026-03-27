#[path = "../src/agent_runtime.rs"]
mod agent_runtime;
#[path = "../src/gui_launcher.rs"]
mod gui_launcher;
#[path = "../src/persistence.rs"]
mod persistence;
#[path = "../src/pty_host.rs"]
mod pty_host;
#[path = "../src/registry.rs"]
mod registry;
#[path = "../src/request_handler.rs"]
mod request_handler;
#[path = "../src/transcript_store.rs"]
mod transcript_store;

use axis_core::automation::{AutomationRequest, AutomationResponse};
use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

fn send_request(
    socket_path: &Path,
    request: &AutomationRequest,
) -> anyhow::Result<AutomationResponse> {
    let mut stream = UnixStream::connect(socket_path)?;
    let payload = serde_json::to_vec(request)?;
    stream.write_all(&payload)?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    Ok(serde_json::from_str(line.trim())?)
}

fn wait_for_log_lines(log_path: &Path, expected_lines: usize) -> String {
    for _ in 0..40 {
        if let Ok(log) = fs::read_to_string(log_path) {
            if log.lines().count() >= expected_lines {
                return log;
            }
        }
        thread::sleep(Duration::from_millis(25));
    }
    fs::read_to_string(log_path).expect("launch log should exist")
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct EnvVarGuard {
    key: &'static str,
    original: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(value) = &self.original {
            std::env::set_var(self.key, value);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

#[test]
fn gui_ensure_running_only_launches_when_heartbeat_is_absent_or_stale() {
    let _env_guard = env_lock()
        .lock()
        .expect("environment test lock should not be poisoned");
    let temp = tempfile::tempdir().expect("tempdir should exist");
    let socket_path = temp.path().join("axisd.sock");
    let data_dir = temp.path().join("daemon-data");
    let launcher_script = temp.path().join("fake-axis-app.sh");
    let launch_log = temp.path().join("launch.log");
    let workspace_root = temp.path().join("workspace");
    fs::create_dir_all(&workspace_root).expect("workspace root should exist");

    fs::write(
        &launcher_script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$AXIS_WORKSPACE_ROOT\" >> \"{}\"\n",
            launch_log.display()
        ),
    )
    .expect("launcher script should be written");
    let mut permissions = fs::metadata(&launcher_script)
        .expect("launcher script metadata should exist")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&launcher_script, permissions)
        .expect("launcher script should be executable");

    let _app_bin = EnvVarGuard::set("AXIS_APP_BIN", &launcher_script);
    let _ttl = EnvVarGuard::set("AXIS_GUI_HEARTBEAT_TTL_MS", "25");

    let server = request_handler::start_background_daemon(socket_path.clone(), data_dir)
        .expect("daemon should start");
    let workspace_root = workspace_root.display().to_string();

    let launched = send_request(
        &socket_path,
        &AutomationRequest::GuiEnsureRunning {
            workspace_root: workspace_root.clone(),
        },
    )
    .expect("ensure running should succeed");
    assert!(launched.ok);
    assert_eq!(launched.result.expect("launch result")["launched"], true);

    let log = wait_for_log_lines(&launch_log, 1);
    assert_eq!(log.lines().count(), 1);

    let heartbeat = send_request(
        &socket_path,
        &AutomationRequest::GuiHeartbeat {
            workspace_root: workspace_root.clone(),
            gui_pid: 4242,
        },
    )
    .expect("heartbeat should succeed");
    assert!(heartbeat.ok);

    let skipped = send_request(
        &socket_path,
        &AutomationRequest::GuiEnsureRunning {
            workspace_root: workspace_root.clone(),
        },
    )
    .expect("fresh ensure running should succeed");
    assert!(skipped.ok);
    assert_eq!(skipped.result.expect("fresh result")["launched"], false);
    let log = wait_for_log_lines(&launch_log, 1);
    assert_eq!(log.lines().count(), 1);

    thread::sleep(Duration::from_millis(40));
    let relaunched = send_request(
        &socket_path,
        &AutomationRequest::GuiEnsureRunning {
            workspace_root: workspace_root.clone(),
        },
    )
    .expect("stale ensure running should succeed");
    assert!(relaunched.ok);
    assert_eq!(relaunched.result.expect("stale result")["launched"], true);

    let log = wait_for_log_lines(&launch_log, 2);
    assert_eq!(log.lines().count(), 2);
    assert!(log.lines().all(|line| line == workspace_root));

    drop(server);
}

#[test]
fn gui_ensure_running_does_not_relaunch_before_first_heartbeat_arrives() {
    let _env_guard = env_lock()
        .lock()
        .expect("environment test lock should not be poisoned");
    let temp = tempfile::tempdir().expect("tempdir should exist");
    let socket_path = temp.path().join("axisd.sock");
    let data_dir = temp.path().join("daemon-data");
    let launcher_script = temp.path().join("fake-axis-app.sh");
    let launch_log = temp.path().join("launch.log");
    let workspace_root = temp.path().join("workspace");
    fs::create_dir_all(&workspace_root).expect("workspace root should exist");

    fs::write(
        &launcher_script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$AXIS_WORKSPACE_ROOT\" >> \"{}\"\n",
            launch_log.display()
        ),
    )
    .expect("launcher script should be written");
    let mut permissions = fs::metadata(&launcher_script)
        .expect("launcher script metadata should exist")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&launcher_script, permissions)
        .expect("launcher script should be executable");

    let _app_bin = EnvVarGuard::set("AXIS_APP_BIN", &launcher_script);
    let _ttl = EnvVarGuard::set("AXIS_GUI_HEARTBEAT_TTL_MS", "1000");

    let server = request_handler::start_background_daemon(socket_path.clone(), data_dir)
        .expect("daemon should start");
    let workspace_root = workspace_root.display().to_string();

    let first = send_request(
        &socket_path,
        &AutomationRequest::GuiEnsureRunning {
            workspace_root: workspace_root.clone(),
        },
    )
    .expect("first ensure running should succeed");
    assert!(first.ok);
    assert_eq!(first.result.expect("first result")["launched"], true);
    let log = wait_for_log_lines(&launch_log, 1);
    assert_eq!(log.lines().count(), 1);

    let second = send_request(
        &socket_path,
        &AutomationRequest::GuiEnsureRunning {
            workspace_root: workspace_root.clone(),
        },
    )
    .expect("second ensure running should succeed");
    assert!(second.ok);
    assert_eq!(
        second.result.expect("second result")["launched"],
        false,
        "second ensure-running call should treat the pending launch as fresh until first heartbeat arrives",
    );

    let log = wait_for_log_lines(&launch_log, 1);
    assert_eq!(
        log.lines().count(),
        1,
        "expected only one GUI launch before heartbeat"
    );

    drop(server);
}
