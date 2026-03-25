use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    env,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

#[derive(Debug)]
struct CliOptions {
    socket_path: PathBuf,
    method: String,
    params: Value,
}

#[derive(Debug, Serialize)]
struct AutomationRequest {
    id: u64,
    method: String,
    params: Value,
}

#[derive(Debug, Deserialize)]
struct AutomationResponse {
    ok: bool,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<String>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let options = parse_cli(env::args().skip(1).collect())?;
    let response = send_request(&options.socket_path, &options.method, options.params)?;

    if !response.ok {
        return Err(response
            .error
            .unwrap_or_else(|| "canvas automation request failed".to_string()));
    }

    let result = response.result.unwrap_or(Value::Null);
    let pretty = serde_json::to_string_pretty(&result)
        .map_err(|error| format!("serialize response: {error}"))?;
    println!("{pretty}");
    Ok(())
}

fn parse_cli(args: Vec<String>) -> Result<CliOptions, String> {
    if args.is_empty() {
        return Err(help_text());
    }

    let mut socket_path = default_socket_path();
    let mut cursor = 0usize;
    while cursor < args.len() {
        match args[cursor].as_str() {
            "-h" | "--help" | "help" => return Err(help_text()),
            "--socket" => {
                cursor += 1;
                let value = args
                    .get(cursor)
                    .ok_or_else(|| "--socket requires a path".to_string())?;
                socket_path = PathBuf::from(value);
            }
            _ => break,
        }
        cursor += 1;
    }

    if cursor >= args.len() {
        return Err(help_text());
    }

    let command = &args[cursor];
    cursor += 1;

    if command == "raw" {
        let method = args
            .get(cursor)
            .ok_or_else(|| "raw requires a method".to_string())?
            .to_string();
        cursor += 1;
        let payload = args
            .get(cursor)
            .ok_or_else(|| "raw requires a JSON object payload".to_string())?;
        let params = serde_json::from_str::<Value>(payload)
            .map_err(|error| format!("parse raw params JSON: {error}"))?;
        if !params.is_object() {
            return Err("raw payload must be a JSON object".to_string());
        }
        return Ok(CliOptions {
            socket_path,
            method,
            params,
        });
    }

    let method = resolve_method_alias(command);
    let mut params = parse_key_value_args(&args[cursor..])?;
    if let Some(kind) = default_kind_for_alias(command) {
        if let Some(object) = params.as_object_mut() {
            object
                .entry("kind".to_string())
                .or_insert_with(|| Value::String(kind.to_string()));
        }
    }

    Ok(CliOptions {
        socket_path,
        method: method.to_string(),
        params,
    })
}

fn resolve_method_alias(command: &str) -> &str {
    match command {
        "state" => "state.current",
        "desks" => "workdesk.list",
        "new-desk" => "workdesk.create",
        "select-desk" => "workdesk.select",
        "rename-desk" => "workdesk.rename",
        "new-pane" => "pane.create",
        "new-browser" => "pane.create",
        "new-editor" => "pane.create",
        "focus-pane" => "pane.focus",
        "list-surfaces" => "surface.list",
        "focus-surface" => "surface.focus",
        "close-surface" => "surface.close",
        "set-attention" => "attention.set",
        "clear-attention" => "attention.clear",
        "set-status" => "status.set",
        "set-progress" => "progress.set",
        "notify" => "notification.create",
        other => other,
    }
}

fn default_kind_for_alias(command: &str) -> Option<&'static str> {
    match command {
        "new-browser" => Some("browser"),
        "new-editor" => Some("editor"),
        _ => None,
    }
}

fn parse_key_value_args(args: &[String]) -> Result<Value, String> {
    let mut params = Map::new();

    for argument in args {
        let Some((key, raw_value)) = argument.split_once('=') else {
            return Err(format!(
                "expected `key=value`, got `{argument}`\n\n{}",
                help_text()
            ));
        };
        if key.trim().is_empty() {
            return Err(format!("invalid empty key in `{argument}`"));
        }
        insert_param_value(&mut params, key, parse_value(raw_value)?);
    }

    Ok(Value::Object(params))
}

fn parse_value(raw: &str) -> Result<Value, String> {
    match raw {
        "true" => Ok(Value::Bool(true)),
        "false" => Ok(Value::Bool(false)),
        "null" => Ok(Value::Null),
        _ => {
            if let Ok(number) = raw.parse::<i64>() {
                return Ok(Value::Number(number.into()));
            }
            if raw.starts_with('{')
                || raw.starts_with('[')
                || raw.starts_with('"')
                || raw.starts_with('-')
            {
                if let Ok(value) = serde_json::from_str::<Value>(raw) {
                    return Ok(value);
                }
            }
            Ok(Value::String(raw.to_string()))
        }
    }
}

fn insert_param_value(target: &mut Map<String, Value>, key: &str, value: Value) {
    let mut segments = key.split('.').peekable();
    let mut current = target;

    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            current.insert(segment.to_string(), value);
            return;
        }

        let entry = current
            .entry(segment.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        current = entry
            .as_object_mut()
            .expect("nested param entry should be an object");
    }
}

fn send_request(
    socket_path: &Path,
    method: &str,
    params: Value,
) -> Result<AutomationResponse, String> {
    let mut stream = UnixStream::connect(socket_path).map_err(|error| {
        format!(
            "connect {}: {error}\nLaunch `canvas-app` first or pass `--socket <path>`.",
            socket_path.display()
        )
    })?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let request = AutomationRequest {
        id: 1,
        method: method.to_string(),
        params,
    };
    let payload =
        serde_json::to_vec(&request).map_err(|error| format!("serialize request: {error}"))?;
    stream
        .write_all(&payload)
        .and_then(|_| stream.write_all(b"\n"))
        .and_then(|_| stream.flush())
        .map_err(|error| format!("write request: {error}"))?;

    let mut response_line = String::new();
    BufReader::new(stream)
        .read_line(&mut response_line)
        .map_err(|error| format!("read response: {error}"))?;
    if response_line.trim().is_empty() {
        return Err("empty response from canvas socket".to_string());
    }

    serde_json::from_str::<AutomationResponse>(response_line.trim())
        .map_err(|error| format!("parse response: {error}"))
}

fn default_socket_path() -> PathBuf {
    workspace_root_path().join(".canvas").join("canvas.sock")
}

fn workspace_root_path() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    root.canonicalize().unwrap_or(root)
}

fn help_text() -> String {
    format!(
        "\
canvas <method|alias> [key=value ...]
canvas raw <method> '{{...json...}}'
canvas --socket <path> <method|alias> ...

Aliases:
  state             -> state.current
  desks             -> workdesk.list
  new-desk          -> workdesk.create
  select-desk       -> workdesk.select
  rename-desk       -> workdesk.rename
  new-pane          -> pane.create
  new-browser       -> pane.create (kind=browser)
  new-editor        -> pane.create (kind=editor)
  focus-pane        -> pane.focus
  list-surfaces     -> surface.list
  focus-surface     -> surface.focus
  close-surface     -> surface.close
  set-attention     -> attention.set
  clear-attention   -> attention.clear
  set-status        -> status.set
  set-progress      -> progress.set
  notify            -> notification.create

Examples:
  canvas state
  canvas desks
  canvas new-desk template=implementation name='Release Desk'
  canvas set-status workdesk_index=0 value=Reviewing
  canvas set-progress workdesk_index=0 label=Build value=55
  canvas new-pane workdesk_index=0 kind=agent title='Verifier'
  canvas new-browser workdesk_index=0 url='https://example.com'
  canvas new-editor workdesk_index=0 file_path='/tmp/notes.rs'
  canvas list-surfaces workdesk_index=0 pane_id=3
  canvas set-attention workdesk_index=0 pane_id=3 state=waiting unread=true
  canvas raw pane.create '{{\"workdesk_index\":0,\"kind\":\"shell\"}}'

Default socket:
  {}
",
        default_socket_path().display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_aliases() {
        assert_eq!(resolve_method_alias("state"), "state.current");
        assert_eq!(resolve_method_alias("new-pane"), "pane.create");
        assert_eq!(resolve_method_alias("new-browser"), "pane.create");
        assert_eq!(resolve_method_alias("new-editor"), "pane.create");
        assert_eq!(resolve_method_alias("list-surfaces"), "surface.list");
        assert_eq!(resolve_method_alias("workdesk.list"), "workdesk.list");
    }

    #[test]
    fn parses_nested_key_value_pairs() {
        let params = parse_key_value_args(&[
            "progress.label=Build".to_string(),
            "progress.value=55".to_string(),
            "desktop=true".to_string(),
            "name=Desk".to_string(),
        ])
        .expect("params should parse");

        assert_eq!(
            params["progress"]["label"],
            Value::String("Build".to_string())
        );
        assert_eq!(params["progress"]["value"], Value::Number(55.into()));
        assert_eq!(params["desktop"], Value::Bool(true));
        assert_eq!(params["name"], Value::String("Desk".to_string()));
    }

    #[test]
    fn parses_raw_mode() {
        let options = parse_cli(vec![
            "raw".to_string(),
            "pane.create".to_string(),
            "{\"workdesk_index\":0,\"kind\":\"agent\"}".to_string(),
        ])
        .expect("cli should parse");

        assert_eq!(options.method, "pane.create");
        assert_eq!(options.params["workdesk_index"], Value::Number(0.into()));
        assert_eq!(options.params["kind"], Value::String("agent".to_string()));
    }

    #[test]
    fn new_browser_alias_injects_kind() {
        let options = parse_cli(vec![
            "new-browser".to_string(),
            "workdesk_index=0".to_string(),
            "url=https://example.com".to_string(),
        ])
        .expect("cli should parse");

        assert_eq!(options.method, "pane.create");
        assert_eq!(options.params["kind"], Value::String("browser".to_string()));
        assert_eq!(
            options.params["url"],
            Value::String("https://example.com".to_string())
        );
    }

    #[test]
    fn new_editor_alias_injects_kind() {
        let options = parse_cli(vec![
            "new-editor".to_string(),
            "workdesk_index=0".to_string(),
            "file_path=/tmp/main.rs".to_string(),
        ])
        .expect("cli should parse");

        assert_eq!(options.method, "pane.create");
        assert_eq!(options.params["kind"], Value::String("editor".to_string()));
        assert_eq!(
            options.params["file_path"],
            Value::String("/tmp/main.rs".to_string())
        );
    }
}
