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

#[test]
fn daemon_health_round_trips_over_temp_socket() {
    let temp = tempfile::tempdir().expect("tempdir should exist");
    let socket_path = temp.path().join("axisd.sock");
    let data_dir = temp.path().join("daemon-data");

    let server = request_handler::start_background_daemon(socket_path.clone(), data_dir)
        .expect("daemon should start");
    let response =
        send_request(&socket_path, &AutomationRequest::DaemonHealth).expect("request should round-trip");

    assert!(response.ok);
    assert_eq!(response.result.expect("health result")["status"], "ok");

    drop(server);
}

#[test]
fn daemon_socket_is_single_owner() {
    let temp = tempfile::tempdir().expect("tempdir should exist");
    let socket_path = temp.path().join("axisd.sock");
    let data_dir = temp.path().join("daemon-data");

    let server = request_handler::start_background_daemon(socket_path.clone(), data_dir.clone())
        .expect("first daemon should start");
    let error = request_handler::start_background_daemon(socket_path.clone(), data_dir)
        .expect_err("second daemon bind should fail");

    assert!(
        error.to_string().contains("bind"),
        "expected bind-related error, got {error:#}"
    );

    drop(server);
}
