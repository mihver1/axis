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
use axis_core::workdesk::{WorkdeskId, WorkdeskRecord, WorkdeskTemplateKind};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

fn send_request(
    socket_path: &std::path::Path,
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

fn record(workdesk_id: &str, workspace_root: &str, name: &str) -> WorkdeskRecord {
    WorkdeskRecord {
        workdesk_id: WorkdeskId::new(workdesk_id),
        workspace_root: workspace_root.to_string(),
        name: name.to_string(),
        summary: format!("{name} summary"),
        template: Some(WorkdeskTemplateKind::Implementation),
        worktree_binding: None,
    }
}

#[test]
fn workdesk_registry_persists_and_filters_by_workspace_root() {
    let temp = tempfile::tempdir().expect("tempdir should exist");
    let socket_path = temp.path().join("axisd.sock");
    let data_dir = temp.path().join("daemon-data");

    let server = request_handler::start_background_daemon(socket_path.clone(), data_dir.clone())
        .expect("daemon should start");

    let ensure_a = AutomationRequest::WorkdeskEnsure {
        record: record("desk-a", "/repo-a", "Desk A"),
    };
    let ensure_b = AutomationRequest::WorkdeskEnsure {
        record: record("desk-b", "/repo-b", "Desk B"),
    };
    assert!(send_request(&socket_path, &ensure_a).expect("ensure a").ok);
    assert!(send_request(&socket_path, &ensure_b).expect("ensure b").ok);

    let filtered = send_request(
        &socket_path,
        &AutomationRequest::WorkdeskList {
            workspace_root: Some("/repo-a".to_string()),
        },
    )
    .expect("filtered list should succeed");
    let filtered_records = filtered
        .result
        .expect("filtered result should exist")
        .as_array()
        .expect("filtered result should be array")
        .to_vec();
    assert_eq!(filtered_records.len(), 1);
    assert_eq!(filtered_records[0]["workdesk_id"], "desk-a");

    drop(server);

    let restarted = request_handler::start_background_daemon(socket_path.clone(), data_dir)
        .expect("daemon should restart from persisted state");
    let all = send_request(
        &socket_path,
        &AutomationRequest::WorkdeskList {
            workspace_root: None,
        },
    )
    .expect("list after restart should succeed");
    let all_records = all
        .result
        .expect("all result should exist")
        .as_array()
        .expect("all result should be array")
        .to_vec();
    assert_eq!(all_records.len(), 2);

    drop(restarted);
}
