//! Integration tests for the Codex CLI adapter (inline shell via `sh -c`; real `codex` is opt-in).

use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;

use axis_agent_runtime::adapters::codex::CodexProvider;
use axis_agent_runtime::provider::{ProviderRegistry, StartAgentRequest};
use axis_agent_runtime::SessionManager;
use axis_core::agent::{
    AgentAttention, AgentLifecycle, AgentSessionId, AgentSessionRecord, AgentTransportKind,
};
use tempfile::tempdir;

const MAX_POLLS: u32 = 10_000;

fn poll_until(
    mgr: &mut SessionManager,
    id: &AgentSessionId,
    pred: impl Fn(&AgentSessionRecord) -> bool,
) {
    for _ in 0..MAX_POLLS {
        mgr.poll_provider(id).unwrap();
        if let Some(s) = mgr.session(id) {
            if pred(s) {
                return;
            }
        }
        std::thread::yield_now();
    }
    panic!("condition not met within {MAX_POLLS} poll_provider calls");
}

fn codex_registry_sh_c(script_body: &str) -> ProviderRegistry {
    let mut reg = ProviderRegistry::new();
    reg.register(
        "codex",
        Arc::new(CodexProvider::with_base_argv(vec![
            "/bin/sh".into(),
            "-c".into(),
            script_body.into(),
        ])),
    );
    reg
}

#[test]
fn codex_adapter_successful_launch_reaches_running() {
    let mut mgr = SessionManager::new(codex_registry_sh_c("sleep 120"));
    let id = mgr
        .start_session(StartAgentRequest {
            cwd: "/".into(),
            provider_profile_id: "codex".into(),
            transport: AgentTransportKind::CliWrapped,
            argv_suffix: vec![],
            env: BTreeMap::new(),
            workdesk_id: None,
        })
        .unwrap();

    poll_until(&mut mgr, &id, |s| s.lifecycle == AgentLifecycle::Running);
    let s = mgr.session(&id).unwrap();
    assert_eq!(s.attention, AgentAttention::Quiet);

    mgr.stop_session(&id).unwrap();
}

#[test]
fn codex_adapter_needs_review_sets_waiting_and_attention() {
    let mut mgr = SessionManager::new(codex_registry_sh_c(
        "/bin/echo 'AXIS_ATTENTION needs_review'; while true; do sleep 60; done",
    ));
    let id = mgr
        .start_session(StartAgentRequest {
            cwd: "/".into(),
            provider_profile_id: "codex".into(),
            transport: AgentTransportKind::CliWrapped,
            argv_suffix: vec![],
            env: BTreeMap::new(),
            workdesk_id: None,
        })
        .unwrap();

    poll_until(&mut mgr, &id, |s| {
        s.lifecycle == AgentLifecycle::Waiting && s.attention == AgentAttention::NeedsReview
    });

    mgr.stop_session(&id).unwrap();
}

#[test]
fn codex_adapter_unexpected_exit_emits_failed() {
    let mut mgr = SessionManager::new(codex_registry_sh_c("/bin/echo 'AXIS_STATUS oops'; exit 1"));
    let id = mgr
        .start_session(StartAgentRequest {
            cwd: "/".into(),
            provider_profile_id: "codex".into(),
            transport: AgentTransportKind::CliWrapped,
            argv_suffix: vec![],
            env: BTreeMap::new(),
            workdesk_id: None,
        })
        .unwrap();

    poll_until(&mut mgr, &id, |s| {
        s.lifecycle == AgentLifecycle::Failed && s.status_message == "oops"
    });
}

#[test]
fn codex_adapter_stop_kills_child_and_drops_session() {
    let mut mgr = SessionManager::new(codex_registry_sh_c("sleep 120"));
    let id = mgr
        .start_session(StartAgentRequest {
            cwd: "/".into(),
            provider_profile_id: "codex".into(),
            transport: AgentTransportKind::CliWrapped,
            argv_suffix: vec![],
            env: BTreeMap::new(),
            workdesk_id: None,
        })
        .unwrap();

    poll_until(&mut mgr, &id, |s| s.lifecycle == AgentLifecycle::Running);

    mgr.stop_session(&id).unwrap();
    assert!(mgr.session(&id).is_none());
}

#[test]
fn codex_adapter_send_turn_writes_shared_cli_command_and_parses_structured_status() {
    let dir = tempdir().unwrap();
    let capture_path = dir.path().join("codex-stdin.txt");
    let script = format!(
        "while IFS= read -r line; do printf '%s\\n' \"$line\" > '{}'; /bin/echo 'AXIS_EVENT {{\"kind\":\"status\",\"message\":\"turn received\"}}'; done",
        capture_path.display()
    );
    let mut mgr = SessionManager::new(codex_registry_sh_c(&script));
    let id = mgr
        .start_session(StartAgentRequest {
            cwd: "/".into(),
            provider_profile_id: "codex".into(),
            transport: AgentTransportKind::CliWrapped,
            argv_suffix: vec![],
            env: BTreeMap::new(),
            workdesk_id: None,
        })
        .unwrap();

    poll_until(&mut mgr, &id, |s| s.lifecycle == AgentLifecycle::Running);
    assert!(mgr.session_detail(&id).unwrap().capabilities.turn_input);

    mgr.send_turn(&id, "Continue with the summary.").unwrap();
    poll_until(&mut mgr, &id, |s| s.status_message == "turn received");

    let line = fs::read_to_string(&capture_path).unwrap();
    assert!(
        line.contains(r#""kind":"send_turn""#),
        "unexpected command: {line}"
    );
    assert!(
        line.contains(r#""text":"Continue with the summary.""#),
        "unexpected command: {line}"
    );
}

#[test]
#[ignore = "set CODEX_ADAPTER_TESTS=1 to run against a real codex binary"]
fn codex_adapter_smoke_real_binary() {
    assert_eq!(
        std::env::var("CODEX_ADAPTER_TESTS").ok().as_deref(),
        Some("1"),
        "set CODEX_ADAPTER_TESTS=1 when running this test with cargo test -- --ignored"
    );
    let mut reg = ProviderRegistry::new();
    reg.register("codex", Arc::new(CodexProvider::new()));
    let mut mgr = SessionManager::new(reg);
    let id = mgr
        .start_session(StartAgentRequest {
            cwd: "/tmp".into(),
            provider_profile_id: "codex".into(),
            transport: AgentTransportKind::CliWrapped,
            argv_suffix: vec!["--help".into()],
            env: BTreeMap::new(),
            workdesk_id: None,
        })
        .unwrap();
    poll_until(&mut mgr, &id, |s| s.lifecycle != AgentLifecycle::Planned);
    let _ = mgr.stop_session(&id);
}
