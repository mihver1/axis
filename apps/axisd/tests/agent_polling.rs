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

use axis_core::agent::AgentSessionRecord;
use axis_core::automation::{AutomationRequest, AutomationResponse};
use axis_core::worktree::WorktreeId;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
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

#[test]
fn agent_list_polls_daemon_sessions_forward() {
    let temp = tempfile::tempdir().expect("tempdir should exist");
    let socket_path = temp.path().join("axisd.sock");
    let data_dir = temp.path().join("daemon-data");
    let wrapper = temp.path().join("claude-code-wrapper.sh");
    let worktree_root = temp.path().join("worktree");
    fs::create_dir_all(&worktree_root).expect("worktree dir should exist");

    fs::write(&wrapper, "#!/bin/sh\nsleep 60\n").expect("wrapper script should exist");
    let mut permissions = fs::metadata(&wrapper)
        .expect("wrapper metadata should exist")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&wrapper, permissions).expect("wrapper should be executable");

    std::env::set_var("AXIS_CLAUDE_CODE_BIN", &wrapper);
    let server = request_handler::start_background_daemon(socket_path.clone(), data_dir)
        .expect("daemon should start");

    let start = send_request(
        &socket_path,
        &AutomationRequest::AgentStart {
            worktree_id: WorktreeId::new(worktree_root.display().to_string()),
            provider_profile_id: "claude-code".to_string(),
            argv: vec![],
            workdesk_id: None,
            surface_id: None,
        },
    )
    .expect("agent start should succeed");
    assert!(start.ok);

    let mut running = None;
    for _ in 0..30 {
        thread::sleep(Duration::from_millis(50));
        let list = send_request(
            &socket_path,
            &AutomationRequest::AgentList {
                worktree_id: Some(WorktreeId::new(worktree_root.display().to_string())),
            },
        )
        .expect("agent list should succeed");
        assert!(list.ok);
        let sessions: Vec<AgentSessionRecord> =
            serde_json::from_value(list.result.expect("list should return sessions"))
                .expect("agent sessions should decode");
        if let Some(record) = sessions.first() {
            if record.lifecycle == axis_core::agent::AgentLifecycle::Running {
                running = Some(record.clone());
                break;
            }
        }
    }

    let record = running.expect("expected agent session to transition to running");
    let stop = send_request(
        &socket_path,
        &AutomationRequest::AgentStop {
            agent_session_id: record.id,
        },
    )
    .expect("agent stop should succeed");
    assert!(stop.ok);

    drop(server);
    std::env::remove_var("AXIS_CLAUDE_CODE_BIN");
}
