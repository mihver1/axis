use axis_core::{
    agent::AgentSessionId,
    automation::{AutomationRequest, AutomationResponse},
    paths::{axis_user_data_dir, daemon_socket_path_for, AXIS_SOCKET_PATH_ENV},
    worktree::WorktreeId,
};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::{
    env,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

const PRODUCT_NAME: &str = "axis";
const APP_BINARY: &str = "axis";
const DAEMON_BINARY: &str = "axisd";

#[derive(Debug)]
struct CliOptions {
    socket_path: PathBuf,
    request: AutomationRequest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommandAlias {
    State,
    Worktree,
    WorktreeStatus,
    StartAgent,
    StopAgent,
    ListAgents,
    Review,
    NextAttention,
    EnsureGui,
}

#[derive(Debug, Serialize)]
struct SocketAutomationRequest<'a> {
    id: u64,
    #[serde(flatten)]
    request: &'a AutomationRequest,
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
    let response = send_request(&options.socket_path, &options.request)?;

    if !response.ok {
        return Err(response
            .error
            .unwrap_or_else(|| format!("{PRODUCT_NAME} automation request failed")));
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

    let request = if command == "raw" {
        let method = args
            .get(cursor)
            .ok_or_else(|| "raw requires a method".to_string())?;
        cursor += 1;
        let payload = args
            .get(cursor)
            .ok_or_else(|| "raw requires a JSON object payload".to_string())?;
        let params = serde_json::from_str::<Value>(payload)
            .map_err(|error| format!("parse raw params JSON: {error}"))?;
        if !params.is_object() {
            return Err("raw payload must be a JSON object".to_string());
        }
        request_from_method_and_params(method, params)?
    } else if let Some(alias) = resolve_command_alias(command) {
        request_from_alias(alias, &args[cursor..])?
    } else if command.contains('.') {
        let params = parse_key_value_args(&args[cursor..])?;
        request_from_method_and_params(command, Value::Object(params))?
    } else {
        return Err(format!(
            "unknown command `{command}`\n\n{}",
            help_text()
        ));
    };

    Ok(CliOptions {
        socket_path,
        request,
    })
}

fn resolve_command_alias(command: &str) -> Option<CommandAlias> {
    match command {
        "state" => Some(CommandAlias::State),
        "worktree" => Some(CommandAlias::Worktree),
        "worktree-status" => Some(CommandAlias::WorktreeStatus),
        "start-agent" => Some(CommandAlias::StartAgent),
        "stop-agent" => Some(CommandAlias::StopAgent),
        "list-agents" => Some(CommandAlias::ListAgents),
        "review" => Some(CommandAlias::Review),
        "next-attention" => Some(CommandAlias::NextAttention),
        "ensure-gui" => Some(CommandAlias::EnsureGui),
        _ => None,
    }
}

fn request_from_alias(alias: CommandAlias, args: &[String]) -> Result<AutomationRequest, String> {
    let mut params = parse_key_value_args(args)?;
    let request = match alias {
        CommandAlias::State => {
            let workdesk_id = take_optional_string_param(&mut params, "workdesk_id")?;
            ensure_no_unknown_params(&params, "state")?;
            AutomationRequest::StateCurrent { workdesk_id }
        }
        CommandAlias::Worktree => {
            let repo_root = take_required_string_param(&mut params, "repo_root")?;
            let branch = take_optional_string_param(&mut params, "branch")?;
            let attach_path = take_optional_string_param(&mut params, "attach_path")?;
            ensure_no_unknown_params(&params, "worktree")?;
            AutomationRequest::WorktreeCreateOrAttach {
                repo_root,
                branch,
                attach_path,
            }
        }
        CommandAlias::WorktreeStatus => {
            let worktree_id = WorktreeId::new(take_required_string_param(&mut params, "worktree_id")?);
            ensure_no_unknown_params(&params, "worktree-status")?;
            AutomationRequest::WorktreeStatus { worktree_id }
        }
        CommandAlias::StartAgent => {
            let worktree_id = WorktreeId::new(take_required_string_param(&mut params, "worktree_id")?);
            let provider_profile_id =
                take_required_string_param(&mut params, "provider_profile_id")?;
            let argv = take_string_vec_param(&mut params, "argv")?;
            ensure_no_unknown_params(&params, "start-agent")?;
            AutomationRequest::AgentStart {
                worktree_id,
                provider_profile_id,
                argv,
                workdesk_id: None,
                surface_id: None,
            }
        }
        CommandAlias::StopAgent => {
            let agent_session_id =
                AgentSessionId::new(take_required_string_param(&mut params, "agent_session_id")?);
            ensure_no_unknown_params(&params, "stop-agent")?;
            AutomationRequest::AgentStop { agent_session_id }
        }
        CommandAlias::ListAgents => {
            let worktree_id = take_optional_string_param(&mut params, "worktree_id")?
                .map(WorktreeId::new);
            ensure_no_unknown_params(&params, "list-agents")?;
            AutomationRequest::AgentList { worktree_id }
        }
        CommandAlias::Review => {
            let worktree_id = WorktreeId::new(take_required_string_param(&mut params, "worktree_id")?);
            ensure_no_unknown_params(&params, "review")?;
            AutomationRequest::DeskReviewSummary { worktree_id }
        }
        CommandAlias::NextAttention => {
            let workdesk_id = take_optional_string_param(&mut params, "workdesk_id")?;
            ensure_no_unknown_params(&params, "next-attention")?;
            AutomationRequest::AttentionNext { workdesk_id }
        }
        CommandAlias::EnsureGui => {
            ensure_no_unknown_params(&params, "ensure-gui")?;
            AutomationRequest::GuiEnsureRunning {
                workspace_root: workspace_root_path().display().to_string(),
            }
        }
    };

    Ok(request)
}

fn request_from_method_and_params(method: &str, params: Value) -> Result<AutomationRequest, String> {
    let request_with_params = json!({
        "method": method,
        "params": params,
    });
    match serde_json::from_value(request_with_params) {
        Ok(request) => Ok(request),
        Err(error) if params.as_object().is_some_and(|params| params.is_empty()) => {
            serde_json::from_value(json!({ "method": method }))
                .map_err(|fallback| {
                    format!(
                        "parse automation request `{method}`: {error}; retry without params also failed: {fallback}"
                    )
                })
        }
        Err(error) => Err(format!("parse automation request `{method}`: {error}")),
    }
}

fn parse_key_value_args(args: &[String]) -> Result<Map<String, Value>, String> {
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

    Ok(params)
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
    let segments = key.split('.').collect::<Vec<_>>();
    insert_param_segments(target, &segments, value);
}

fn insert_param_segments(target: &mut Map<String, Value>, segments: &[&str], value: Value) {
    if let Some((segment, rest)) = segments.split_first() {
        if rest.is_empty() {
            target.insert((*segment).to_string(), value);
            return;
        }

        let entry = target
            .entry((*segment).to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        insert_param_segments(
            entry
                .as_object_mut()
                .expect("nested param entry should be an object"),
            rest,
            value,
        );
    }
}

fn take_required_string_param(
    params: &mut Map<String, Value>,
    key: &str,
) -> Result<String, String> {
    take_optional_string_param(params, key)?
        .ok_or_else(|| format!("missing required `{key}` parameter"))
}

fn take_optional_string_param(
    params: &mut Map<String, Value>,
    key: &str,
) -> Result<Option<String>, String> {
    match params.remove(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(other) => Err(format!("expected `{key}` to be a string, got {other}")),
    }
}

fn take_string_vec_param(params: &mut Map<String, Value>, key: &str) -> Result<Vec<String>, String> {
    match params.remove(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(values)) => values
            .into_iter()
            .map(|value| match value {
                Value::String(value) => Ok(value),
                other => Err(format!("expected `{key}` entries to be strings, got {other}")),
            })
            .collect(),
        Some(Value::String(value)) => Ok(vec![value]),
        Some(other) => Err(format!("expected `{key}` to be a string array, got {other}")),
    }
}

fn ensure_no_unknown_params(params: &Map<String, Value>, command: &str) -> Result<(), String> {
    if params.is_empty() {
        return Ok(());
    }

    let mut keys = params.keys().cloned().collect::<Vec<_>>();
    keys.sort();
    Err(format!(
        "unknown parameters for `{command}`: {}",
        keys.join(", ")
    ))
}

fn send_request(
    socket_path: &Path,
    request: &AutomationRequest,
) -> Result<AutomationResponse, String> {
    let mut stream = UnixStream::connect(socket_path).map_err(|error| {
        format!(
            "connect {}: {error}\nStart `{DAEMON_BINARY}` first or pass `--socket <path>`.",
            socket_path.display()
        )
    })?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let envelope = SocketAutomationRequest { id: 1, request };
    let payload =
        serde_json::to_vec(&envelope).map_err(|error| format!("serialize request: {error}"))?;
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
        return Err(format!("empty response from {PRODUCT_NAME} socket"));
    }

    serde_json::from_str::<AutomationResponse>(response_line.trim())
        .map_err(|error| format!("parse response: {error}"))
}

fn default_socket_path() -> PathBuf {
    default_socket_path_for(
        env::var_os(AXIS_SOCKET_PATH_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
    )
}

fn default_socket_path_for(explicit_override: Option<PathBuf>) -> PathBuf {
    daemon_socket_path_for(explicit_override, axis_user_data_dir())
}

fn workspace_root_path() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    root.canonicalize().unwrap_or(root)
}

fn help_text() -> String {
    format!(
        "\
{APP_BINARY} <method|alias> [key=value ...]
{APP_BINARY} raw <method> '{{...json...}}'
{APP_BINARY} --socket <path> <method|alias> ...

Aliases:
  state             -> state.current
  worktree          -> worktree.create_or_attach
  worktree-status   -> worktree.status
  start-agent       -> agent.start
  stop-agent        -> agent.stop
  list-agents       -> agent.list
  review            -> review.summary
  next-attention    -> attention.next
  ensure-gui        -> gui.ensure_running

Examples:
  {APP_BINARY} state
  {APP_BINARY} worktree repo_root=/repo branch=feature/demo
  {APP_BINARY} worktree repo_root=/repo attach_path=/repo-demo
  {APP_BINARY} worktree-status worktree_id=wt-demo
  {APP_BINARY} start-agent worktree_id=wt-demo provider_profile_id=codex argv='[\"--danger-full-access\"]'
  {APP_BINARY} stop-agent agent_session_id=session-1
  {APP_BINARY} list-agents worktree_id=wt-demo
  {APP_BINARY} review worktree_id=wt-demo
  {APP_BINARY} next-attention workdesk_id=desk-7
  {APP_BINARY} ensure-gui
  {APP_BINARY} raw state.current '{{\"workdesk_id\":\"desk-7\"}}'

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
    fn default_socket_path_prefers_explicit_override() {
        let override_path = PathBuf::from("/tmp/axis-smoke-demo.sock");
        assert_eq!(default_socket_path_for(Some(override_path.clone())), override_path);
    }

    #[test]
    fn resolves_aliases() {
        assert_eq!(resolve_command_alias("state"), Some(CommandAlias::State));
        assert_eq!(resolve_command_alias("worktree"), Some(CommandAlias::Worktree));
        assert_eq!(
            resolve_command_alias("worktree-status"),
            Some(CommandAlias::WorktreeStatus)
        );
        assert_eq!(
            resolve_command_alias("start-agent"),
            Some(CommandAlias::StartAgent)
        );
        assert_eq!(resolve_command_alias("ensure-gui"), Some(CommandAlias::EnsureGui));
        assert_eq!(resolve_command_alias("unknown"), None);
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
    fn parses_raw_mode_into_shared_request() {
        let options = parse_cli(vec![
            "raw".to_string(),
            "state.current".to_string(),
            "{\"workdesk_id\":\"desk-3\"}".to_string(),
        ])
        .expect("cli should parse");

        assert_eq!(
            options.request,
            AutomationRequest::StateCurrent {
                workdesk_id: Some("desk-3".to_string()),
            }
        );
    }

    #[test]
    fn parses_unit_variant_raw_request_with_empty_object_payload() {
        let options = parse_cli(vec![
            "raw".to_string(),
            "daemon.health".to_string(),
            "{}".to_string(),
        ])
        .expect("raw unit variant should parse");

        assert_eq!(options.request, AutomationRequest::DaemonHealth);
    }

    #[test]
    fn parses_method_name_without_raw_prefix() {
        let options = parse_cli(vec![
            "agent.list".to_string(),
            "worktree_id=wt-7".to_string(),
        ])
        .expect("cli should parse");

        assert_eq!(
            options.request,
            AutomationRequest::AgentList {
                worktree_id: Some(WorktreeId::new("wt-7")),
            }
        );
    }

    #[test]
    fn parses_worktree_create_and_attach_aliases_into_shared_requests() {
        let create = parse_cli(vec![
            "worktree".to_string(),
            "repo_root=/repo".to_string(),
            "branch=feature/x".to_string(),
        ])
        .expect("create alias should parse");
        assert_eq!(
            create.request,
            AutomationRequest::WorktreeCreateOrAttach {
                repo_root: "/repo".to_string(),
                branch: Some("feature/x".to_string()),
                attach_path: None,
            }
        );

        let attach = parse_cli(vec![
            "worktree".to_string(),
            "repo_root=/repo".to_string(),
            "attach_path=/repo-wt".to_string(),
        ])
        .expect("attach alias should parse");
        assert_eq!(
            attach.request,
            AutomationRequest::WorktreeCreateOrAttach {
                repo_root: "/repo".to_string(),
                branch: None,
                attach_path: Some("/repo-wt".to_string()),
            }
        );
    }

    #[test]
    fn parses_agent_start_profile_selection_into_shared_request() {
        let options = parse_cli(vec![
            "start-agent".to_string(),
            "worktree_id=wt-1".to_string(),
            "provider_profile_id=codex".to_string(),
            "argv=[\"--danger-full-access\"]".to_string(),
        ])
        .expect("agent start alias should parse");

        assert_eq!(
            options.request,
            AutomationRequest::AgentStart {
                worktree_id: WorktreeId::new("wt-1"),
                provider_profile_id: "codex".to_string(),
                argv: vec!["--danger-full-access".to_string()],
                workdesk_id: None,
                surface_id: None,
            }
        );
    }

    #[test]
    fn parses_status_stop_and_list_aliases_into_shared_requests() {
        let status = parse_cli(vec![
            "worktree-status".to_string(),
            "worktree_id=wt-3".to_string(),
        ])
        .expect("status alias should parse");
        assert_eq!(
            status.request,
            AutomationRequest::WorktreeStatus {
                worktree_id: WorktreeId::new("wt-3"),
            }
        );

        let stop = parse_cli(vec![
            "stop-agent".to_string(),
            "agent_session_id=session-42".to_string(),
        ])
        .expect("stop alias should parse");
        assert_eq!(
            stop.request,
            AutomationRequest::AgentStop {
                agent_session_id: AgentSessionId::new("session-42"),
            }
        );

        let list = parse_cli(vec!["list-agents".to_string()])
            .expect("list alias should parse");
        assert_eq!(
            list.request,
            AutomationRequest::AgentList { worktree_id: None }
        );
    }

    #[test]
    fn parses_review_and_attention_aliases_into_shared_requests() {
        let review = parse_cli(vec!["review".to_string(), "worktree_id=wt-2".to_string()])
            .expect("review alias should parse");
        assert_eq!(
            review.request,
            AutomationRequest::DeskReviewSummary {
                worktree_id: WorktreeId::new("wt-2"),
            }
        );

        let attention = parse_cli(vec![
            "next-attention".to_string(),
            "workdesk_id=desk-7".to_string(),
        ])
        .expect("attention alias should parse");
        assert_eq!(
            attention.request,
            AutomationRequest::AttentionNext {
                workdesk_id: Some("desk-7".to_string()),
            }
        );

        let ensure_gui =
            parse_cli(vec!["ensure-gui".to_string()]).expect("ensure gui alias should parse");
        assert_eq!(
            ensure_gui.request,
            AutomationRequest::GuiEnsureRunning {
                workspace_root: workspace_root_path().display().to_string(),
            }
        );
    }
}
