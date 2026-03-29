use axis_core::agent::{AgentAttention, AgentSessionRecord};
use axis_core::automation::{AutomationRequest, AutomationResponse};
use axis_core::workdesk::{WorkdeskId, WorkdeskRecord, WorkdeskTemplateKind};
use axis_core::worktree::{WorktreeBinding, WorktreeId};
use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

pub fn send_request(
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

pub fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub struct EnvVarGuard {
    key: &'static str,
    original: Option<OsString>,
}

impl EnvVarGuard {
    pub fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
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

pub fn workdesk_record(
    workdesk_id: &str,
    workspace_root: &str,
    worktree_root: &str,
) -> WorkdeskRecord {
    WorkdeskRecord {
        workdesk_id: WorkdeskId::new(workdesk_id),
        workspace_root: workspace_root.to_string(),
        name: "Implementation Desk".to_string(),
        summary: "Review the current agent loop".to_string(),
        template: Some(WorkdeskTemplateKind::Implementation),
        worktree_binding: Some(WorktreeBinding {
            root_path: worktree_root.to_string(),
            branch: "feature/control-plane".to_string(),
            base_branch: Some("main".to_string()),
            ahead: 0,
            behind: 0,
            dirty: false,
        }),
    }
}

pub fn create_executable_script(path: &Path, contents: &str) {
    fs::write(path, contents).expect("script should be written");
    let mut permissions = fs::metadata(path)
        .expect("script metadata should exist")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("script should be executable");
}

pub fn poll_until_attention(
    socket_path: &Path,
    worktree_id: &WorktreeId,
    expected: AgentAttention,
) -> AgentSessionRecord {
    for _ in 0..30 {
        thread::sleep(Duration::from_millis(50));
        let response = send_request(
            socket_path,
            &AutomationRequest::AgentList {
                worktree_id: Some(worktree_id.clone()),
            },
        )
        .expect("agent list should succeed");
        assert!(response.ok);
        let sessions: Vec<AgentSessionRecord> =
            serde_json::from_value(response.result.expect("sessions payload should exist"))
                .expect("sessions should decode");
        if let Some(record) = sessions.into_iter().find(|record| record.attention == expected) {
            return record;
        }
    }
    panic!("expected attention {expected:?} within polling window");
}
