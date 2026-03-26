use std::collections::BTreeMap;
use std::sync::Arc;

use axis_agent_runtime::adapters::process_only::ProcessOnlyProvider;
use axis_agent_runtime::provider::{ProviderRegistry, StartAgentRequest};
use axis_agent_runtime::SessionManager;
use axis_core::agent::{
    AgentAttention, AgentLifecycle, AgentSessionId, AgentSessionRecord, AgentTransportKind,
};

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

fn process_only_registry_sh_c(script_body: &str) -> ProviderRegistry {
    let mut reg = ProviderRegistry::new();
    reg.register(
        "claude-code",
        Arc::new(ProcessOnlyProvider::with_base_argv(
            "claude-code",
            vec!["/bin/sh".into(), "-c".into(), script_body.into()],
        )),
    );
    reg
}

#[test]
fn process_only_adapter_successful_launch_reaches_running() {
    let mut mgr = SessionManager::new(process_only_registry_sh_c("sleep 120"));
    let id = mgr
        .start_session(StartAgentRequest {
            cwd: "/".into(),
            provider_profile_id: "claude-code".into(),
            transport: AgentTransportKind::CliWrapped,
            argv_suffix: vec![],
            env: BTreeMap::new(),
        })
        .unwrap();

    poll_until(&mut mgr, &id, |s| s.lifecycle == AgentLifecycle::Running);
    let s = mgr.session(&id).unwrap();
    assert_eq!(s.attention, AgentAttention::Quiet);

    mgr.stop_session(&id).unwrap();
}

#[test]
fn process_only_adapter_successful_exit_sets_completed_and_last_status() {
    let mut mgr = SessionManager::new(process_only_registry_sh_c("/bin/echo done; exit 0"));
    let id = mgr
        .start_session(StartAgentRequest {
            cwd: "/".into(),
            provider_profile_id: "claude-code".into(),
            transport: AgentTransportKind::CliWrapped,
            argv_suffix: vec![],
            env: BTreeMap::new(),
        })
        .unwrap();

    poll_until(&mut mgr, &id, |s| {
        s.lifecycle == AgentLifecycle::Completed && s.status_message == "done"
    });
}

#[test]
fn process_only_adapter_stop_kills_child_and_drops_session() {
    let mut mgr = SessionManager::new(process_only_registry_sh_c("sleep 120"));
    let id = mgr
        .start_session(StartAgentRequest {
            cwd: "/".into(),
            provider_profile_id: "claude-code".into(),
            transport: AgentTransportKind::CliWrapped,
            argv_suffix: vec![],
            env: BTreeMap::new(),
        })
        .unwrap();

    poll_until(&mut mgr, &id, |s| s.lifecycle == AgentLifecycle::Running);

    mgr.stop_session(&id).unwrap();
    assert!(mgr.session(&id).is_none());
}
