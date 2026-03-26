//! Local Unix-socket transport for shared automation requests.

use axis_core::automation::{AutomationRequest, AutomationResponse};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::{fs::PermissionsExt, net::UnixListener};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

const AUTOMATION_SOCKET_PATH_ENV: &str = "AXIS_SOCKET_PATH";

pub(crate) struct AutomationServer {
    pub receiver: Receiver<AutomationEnvelope>,
    pub socket_path: PathBuf,
}

pub(crate) struct AutomationEnvelope {
    pub request: AutomationRequest,
    pub response_tx: Sender<AutomationResponse>,
}

#[derive(Debug)]
struct SocketAutomationEnvelopeError {
    id: Option<Value>,
    message: String,
}

#[derive(Clone, Debug, Deserialize)]
struct SocketAutomationRequest {
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

impl SocketAutomationRequest {
    fn into_envelope(
        self,
    ) -> Result<(Option<Value>, AutomationRequest), SocketAutomationEnvelopeError> {
        let Self { id, method, params } = self;
        match serde_json::from_value(json!({
            "method": method,
            "params": params,
        })) {
            Ok(request) => Ok((id, request)),
            Err(error) => Err(SocketAutomationEnvelopeError {
                id,
                message: format!("invalid request: {error}"),
            }),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
struct SocketAutomationResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(flatten)]
    response: AutomationResponse,
}

pub(crate) fn automation_socket_path() -> PathBuf {
    automation_socket_path_for(
        std::env::var_os(AUTOMATION_SOCKET_PATH_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
    )
}

fn automation_socket_path_for(explicit_override: Option<PathBuf>) -> PathBuf {
    if let Some(path) = explicit_override {
        return path;
    }
    crate::workspace_root_path()
        .join(crate::APP_DATA_DIR)
        .join(crate::AUTOMATION_SOCKET_FILE)
}

pub(crate) fn start_automation_server() -> Result<AutomationServer, String> {
    start_automation_server_at(automation_socket_path())
}

pub(crate) fn start_automation_server_at(socket_path: PathBuf) -> Result<AutomationServer, String> {
    let Some(socket_dir) = socket_path.parent() else {
        return Err("invalid automation socket path".to_string());
    };
    fs::create_dir_all(socket_dir)
        .map_err(|error| format!("create {}: {error}", socket_dir.display()))?;
    if socket_path.exists() {
        fs::remove_file(&socket_path)
            .map_err(|error| format!("remove stale {}: {error}", socket_path.display()))?;
    }

    let listener = UnixListener::bind(&socket_path)
        .map_err(|error| format!("bind {}: {error}", socket_path.display()))?;
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("chmod {}: {error}", socket_path.display()))?;

    let (tx, rx) = mpsc::channel();
    let tx_listener = tx.clone();
    let socket_path_for_thread = socket_path.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                continue;
            };
            let tx = tx_listener.clone();
            let socket_path = socket_path_for_thread.clone();
            thread::spawn(move || {
                let mut line = String::new();
                let read_result = {
                    let mut reader = BufReader::new(&mut stream);
                    reader.read_line(&mut line)
                };

                let response = match read_result {
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            SocketAutomationResponse {
                                id: None,
                                response: AutomationResponse::failure(format!(
                                    "empty automation request sent to {}",
                                    socket_path.display()
                                )),
                            }
                        } else {
                            match serde_json::from_str::<SocketAutomationRequest>(trimmed)
                                .map_err(|error| format!("invalid request: {error}"))
                            {
                                Ok(request) => match request.into_envelope() {
                                    Ok((_id, request)) => {
                                        let (response_tx, response_rx) = mpsc::channel();
                                        if tx
                                            .send(AutomationEnvelope {
                                                request,
                                                response_tx,
                                            })
                                            .is_err()
                                        {
                                            SocketAutomationResponse {
                                                id: _id,
                                                response: AutomationResponse::failure(
                                                    "automation command queue is closed",
                                                ),
                                            }
                                        } else {
                                            let response =
                                                response_rx.recv().unwrap_or_else(|_| {
                                                    AutomationResponse::failure(
                                                        "automation response channel closed",
                                                    )
                                                });
                                            SocketAutomationResponse { id: _id, response }
                                        }
                                    }
                                    Err(error) => SocketAutomationResponse {
                                        id: error.id,
                                        response: AutomationResponse::failure(error.message),
                                    },
                                },
                                Err(error) => SocketAutomationResponse {
                                    id: None,
                                    response: AutomationResponse::failure(error),
                                },
                            }
                        }
                    }
                    Err(error) => SocketAutomationResponse {
                        id: None,
                        response: AutomationResponse::failure(format!("read request: {error}")),
                    },
                };

                if let Ok(payload) = serde_json::to_vec(&response) {
                    let _ = stream.write_all(&payload);
                    let _ = stream.write_all(b"\n");
                    let _ = stream.flush();
                }
            });
        }
    });

    Ok(AutomationServer {
        receiver: rx,
        socket_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axis_core::automation::AutomationRequest;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    #[test]
    fn automation_socket_path_prefers_explicit_override() {
        let override_path = PathBuf::from("/tmp/axis-smoke-demo.sock");
        assert_eq!(
            automation_socket_path_for(Some(override_path.clone())),
            override_path
        );
    }

    #[test]
    fn automation_server_round_trips_shared_request_lines() {
        let socket_path = std::env::temp_dir().join(format!(
            "axis-automation-test-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos()
        ));
        let server = start_automation_server_at(socket_path.clone())
            .expect("automation server should start");
        let mut stream = UnixStream::connect(&socket_path).expect("socket should accept clients");

        stream
            .write_all(br#"{"id":1,"method":"state.current","params":{}}"#)
            .expect("request should write");
        stream.write_all(b"\n").expect("newline should write");

        let envelope = server
            .receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("automation envelope should be received");
        assert_eq!(
            envelope.request,
            AutomationRequest::StateCurrent { workdesk_id: None }
        );
        envelope
            .response_tx
            .send(AutomationResponse::success_with_result(
                json!({ "ok": true }),
            ))
            .expect("response should send");

        let mut response_line = String::new();
        BufReader::new(stream)
            .read_line(&mut response_line)
            .expect("response line should read");
        let response: Value =
            serde_json::from_str(response_line.trim()).expect("response should be valid json");

        assert_eq!(response["ok"], Value::Bool(true));
        assert_eq!(response["result"]["ok"], Value::Bool(true));
        assert_eq!(response["id"], json!(1));

        let _ = fs::remove_file(socket_path);
    }

    #[test]
    fn automation_server_preserves_request_id_for_invalid_method_errors() {
        let socket_path = std::env::temp_dir().join(format!(
            "axis-auto-bad-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos()
        ));
        let _server = start_automation_server_at(socket_path.clone())
            .expect("automation server should start");
        let mut stream = UnixStream::connect(&socket_path).expect("socket should accept clients");

        stream
            .write_all(br#"{"id":7,"method":"state.unknown","params":{}}"#)
            .expect("request should write");
        stream.write_all(b"\n").expect("newline should write");

        let mut response_line = String::new();
        BufReader::new(stream)
            .read_line(&mut response_line)
            .expect("response line should read");
        let response: Value =
            serde_json::from_str(response_line.trim()).expect("response should be valid json");

        assert_eq!(response["ok"], Value::Bool(false));
        assert_eq!(response["id"], json!(7));
        assert!(
            response["error"]
                .as_str()
                .is_some_and(|error| error.contains("invalid request")),
            "expected invalid request error, got {response:?}"
        );

        let _ = fs::remove_file(socket_path);
    }
}
