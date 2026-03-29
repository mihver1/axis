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

use axis_core::{
    automation::{AutomationRequest, AutomationResponse},
    terminal::{
        TerminalSessionId, TerminalSessionRecord, TerminalSurfaceKind, TerminalTranscriptChunk,
    },
    workdesk::WorkdeskId,
    SurfaceId,
};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::thread;
use std::time::Duration;

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

fn wait_for_transcript_text(
    socket_path: &std::path::Path,
    terminal_session_id: &TerminalSessionId,
    mut offset: u64,
    needle: &str,
) -> u64 {
    for _ in 0..30 {
        thread::sleep(Duration::from_millis(50));
        let response = send_request(
            socket_path,
            &AutomationRequest::TerminalRead {
                terminal_session_id: terminal_session_id.clone(),
                offset,
            },
        )
        .expect("terminal read should succeed");
        assert!(response.ok);
        let payload = response
            .result
            .expect("terminal read should return payload");
        let chunk = payload
            .get("chunk")
            .cloned()
            .and_then(|value| serde_json::from_value::<Option<TerminalTranscriptChunk>>(value).ok())
            .flatten();
        if let Some(chunk) = chunk {
            offset = chunk.offset + chunk.bytes.len() as u64;
            if String::from_utf8_lossy(&chunk.bytes).contains(needle) {
                return offset;
            }
        }
    }
    panic!("expected transcript replay to include `{needle}`");
}

#[test]
fn transcript_store_reads_appended_bytes_from_offset() {
    let temp = tempfile::tempdir().expect("tempdir should exist");
    let store = transcript_store::TranscriptStore::new(temp.path().join("transcripts"));
    let session_id = TerminalSessionId::new("term-1");

    store
        .append(&session_id, b"hello ")
        .expect("first append should work");
    store
        .append(&session_id, b"world")
        .expect("second append should work");

    let chunk = store
        .read_from(&session_id, 6)
        .expect("read should work")
        .expect("chunk should exist");

    assert_eq!(chunk.terminal_session_id, session_id);
    assert_eq!(chunk.offset, 6);
    assert_eq!(chunk.bytes, b"world");
}

#[test]
fn daemon_terminal_requests_spawn_write_read_and_close() {
    let temp = tempfile::tempdir().expect("tempdir should exist");
    let socket_path = temp.path().join("axisd.sock");
    let data_dir = temp.path().join("daemon-data");
    let server = request_handler::start_background_daemon(socket_path.clone(), data_dir)
        .expect("daemon should start");

    let ensure = send_request(
        &socket_path,
        &AutomationRequest::TerminalEnsure {
            workdesk_id: WorkdeskId::new("desk-1"),
            surface_id: SurfaceId::new(11),
            kind: TerminalSurfaceKind::Shell,
            title: "Shell".to_string(),
            cwd: Some(temp.path().display().to_string()),
            cols: 80,
            rows: 24,
            command: None,
        },
    )
    .expect("terminal ensure should succeed");
    assert!(ensure.ok);
    let record: TerminalSessionRecord = serde_json::from_value(
        ensure
            .result
            .expect("terminal ensure should return a record"),
    )
    .expect("terminal ensure result should decode");

    assert_eq!(record.workdesk_id, WorkdeskId::new("desk-1"));
    assert_eq!(record.surface_id, SurfaceId::new(11));

    let write = send_request(
        &socket_path,
        &AutomationRequest::TerminalWrite {
            terminal_session_id: record.terminal_session_id.clone(),
            bytes: b"printf 'ready\\n'\n".to_vec(),
        },
    )
    .expect("terminal write should succeed");
    assert!(write.ok);
    let _ = wait_for_transcript_text(&socket_path, &record.terminal_session_id, 0, "ready");

    let close = send_request(
        &socket_path,
        &AutomationRequest::TerminalClose {
            terminal_session_id: record.terminal_session_id.clone(),
        },
    )
    .expect("terminal close should succeed");
    assert!(close.ok);

    drop(server);
}

#[test]
fn daemon_terminal_reuses_session_for_same_surface_after_reconnect() {
    let temp = tempfile::tempdir().expect("tempdir should exist");
    let socket_path = temp.path().join("axisd.sock");
    let data_dir = temp.path().join("daemon-data");
    let server = request_handler::start_background_daemon(socket_path.clone(), data_dir)
        .expect("daemon should start");

    let first_ensure = send_request(
        &socket_path,
        &AutomationRequest::TerminalEnsure {
            workdesk_id: WorkdeskId::new("desk-reconnect"),
            surface_id: SurfaceId::new(29),
            kind: TerminalSurfaceKind::Shell,
            title: "Reconnect".to_string(),
            cwd: Some(temp.path().display().to_string()),
            cols: 90,
            rows: 28,
            command: None,
        },
    )
    .expect("first ensure should succeed");
    assert!(first_ensure.ok);
    let first_record: TerminalSessionRecord = serde_json::from_value(
        first_ensure
            .result
            .expect("first ensure should return a record"),
    )
    .expect("first ensure should decode");

    let write_first = send_request(
        &socket_path,
        &AutomationRequest::TerminalWrite {
            terminal_session_id: first_record.terminal_session_id.clone(),
            bytes: b"printf 'alpha\\n'\n".to_vec(),
        },
    )
    .expect("first write should succeed");
    assert!(write_first.ok);
    let offset =
        wait_for_transcript_text(&socket_path, &first_record.terminal_session_id, 0, "alpha");

    let second_ensure = send_request(
        &socket_path,
        &AutomationRequest::TerminalEnsure {
            workdesk_id: WorkdeskId::new("desk-reconnect"),
            surface_id: SurfaceId::new(29),
            kind: TerminalSurfaceKind::Shell,
            title: "Reconnect".to_string(),
            cwd: Some(temp.path().display().to_string()),
            cols: 90,
            rows: 28,
            command: None,
        },
    )
    .expect("second ensure should succeed");
    assert!(second_ensure.ok);
    let second_record: TerminalSessionRecord = serde_json::from_value(
        second_ensure
            .result
            .expect("second ensure should return a record"),
    )
    .expect("second ensure should decode");
    assert_eq!(
        second_record.terminal_session_id, first_record.terminal_session_id,
        "same workdesk/surface should reattach to existing daemon terminal",
    );

    let replay = send_request(
        &socket_path,
        &AutomationRequest::TerminalRead {
            terminal_session_id: second_record.terminal_session_id.clone(),
            offset: 0,
        },
    )
    .expect("replay read should succeed");
    assert!(replay.ok);
    let payload = replay.result.expect("replay should return payload");
    let replay_chunk = payload
        .get("chunk")
        .cloned()
        .and_then(|value| serde_json::from_value::<Option<TerminalTranscriptChunk>>(value).ok())
        .flatten()
        .expect("replay should include transcript bytes");
    assert!(
        String::from_utf8_lossy(&replay_chunk.bytes).contains("alpha"),
        "reconnected client should replay prior transcript history",
    );

    let write_second = send_request(
        &socket_path,
        &AutomationRequest::TerminalWrite {
            terminal_session_id: second_record.terminal_session_id.clone(),
            bytes: b"printf 'beta\\n'\n".to_vec(),
        },
    )
    .expect("second write should succeed");
    assert!(write_second.ok);
    let _ = wait_for_transcript_text(
        &socket_path,
        &second_record.terminal_session_id,
        offset,
        "beta",
    );

    let close = send_request(
        &socket_path,
        &AutomationRequest::TerminalClose {
            terminal_session_id: second_record.terminal_session_id.clone(),
        },
    )
    .expect("close should succeed");
    assert!(close.ok);

    drop(server);
}
