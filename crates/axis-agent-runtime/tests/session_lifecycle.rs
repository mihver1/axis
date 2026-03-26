//! Session registry, lifecycle/attention transitions, and revision bumps.

use std::collections::BTreeMap;
use std::sync::Arc;

use axis_agent_runtime::adapters::fake::FakeProvider;
use axis_agent_runtime::events::RuntimeEvent;
use axis_agent_runtime::provider::{ProviderRegistry, StartAgentRequest};
use axis_agent_runtime::SessionManager;
use axis_core::agent::{
    AgentAttention, AgentLifecycle, AgentSessionId, AgentTransportKind,
};

#[test]
fn start_session_registers_planned_session_and_bumps_revision() {
    let mut reg = ProviderRegistry::new();
    reg.register("fake", Arc::new(FakeProvider::with_standard_script()));
    let mut mgr = SessionManager::new(reg);
    assert_eq!(mgr.revision(), 0);

    let id = mgr
        .start_session(StartAgentRequest {
            cwd: "/tmp/wt".into(),
            provider_profile_id: "fake".into(),
            transport: AgentTransportKind::NativeAcp,
            argv_suffix: vec![],
            env: BTreeMap::new(),
        })
        .unwrap();

    assert_eq!(mgr.revision(), 1);
    let s = mgr.session(&id).expect("session");
    assert_eq!(s.lifecycle, AgentLifecycle::Planned);
    assert_eq!(s.attention, AgentAttention::Quiet);
}

#[test]
fn apply_events_update_lifecycle_attention_and_status() {
    let mut reg = ProviderRegistry::new();
    reg.register("fake", Arc::new(FakeProvider::with_standard_script()));
    let mut mgr = SessionManager::new(reg);
    let id = mgr
        .start_session(StartAgentRequest {
            cwd: "/tmp/wt".into(),
            provider_profile_id: "fake".into(),
            transport: AgentTransportKind::CliWrapped,
            argv_suffix: vec![],
            env: BTreeMap::new(),
        })
        .unwrap();
    let after_start = mgr.revision();

    mgr.apply_events([
        RuntimeEvent::Lifecycle {
            session_id: id.clone(),
            lifecycle: AgentLifecycle::Starting,
        },
        RuntimeEvent::Attention {
            session_id: id.clone(),
            attention: AgentAttention::Working,
        },
        RuntimeEvent::Status {
            session_id: id.clone(),
            message: "compiling".into(),
        },
    ])
    .unwrap();

    assert_eq!(mgr.revision(), after_start + 3);
    let s = mgr.session(&id).unwrap();
    assert_eq!(s.lifecycle, AgentLifecycle::Starting);
    assert_eq!(s.attention, AgentAttention::Working);
    assert_eq!(s.status_message, "compiling");
}

#[test]
fn transition_lifecycle_and_attention_bump_revision_when_changed() {
    let mut reg = ProviderRegistry::new();
    reg.register("fake", Arc::new(FakeProvider::with_standard_script()));
    let mut mgr = SessionManager::new(reg);
    let id = mgr
        .start_session(StartAgentRequest {
            cwd: "/tmp/wt".into(),
            provider_profile_id: "fake".into(),
            transport: AgentTransportKind::CliWrapped,
            argv_suffix: vec![],
            env: BTreeMap::new(),
        })
        .unwrap();
    mgr.apply_events([RuntimeEvent::Lifecycle {
        session_id: id.clone(),
        lifecycle: AgentLifecycle::Running,
    }])
    .unwrap();
    let mid = mgr.revision();

    mgr.transition_attention(&id, AgentAttention::Working).unwrap();
    mgr.transition_lifecycle(&id, AgentLifecycle::Completed).unwrap();

    assert_eq!(mgr.revision(), mid + 2);
    let s = mgr.session(&id).unwrap();
    assert_eq!(s.lifecycle, AgentLifecycle::Completed);
    assert_eq!(s.attention, AgentAttention::Working);
}

#[test]
fn apply_events_for_unknown_session_errors() {
    let mut reg = ProviderRegistry::new();
    reg.register("fake", Arc::new(FakeProvider::with_standard_script()));
    let mut mgr = SessionManager::new(reg);
    let err = mgr
        .apply_events([RuntimeEvent::Lifecycle {
            session_id: AgentSessionId::new("nope"),
            lifecycle: AgentLifecycle::Running,
        }])
        .unwrap_err();
    assert!(
        err.to_string().contains("unknown session"),
        "unexpected error: {err}"
    );
}

#[test]
fn stop_session_calls_provider_and_drops_local_record() {
    let mut reg = ProviderRegistry::new();
    reg.register("fake", Arc::new(FakeProvider::with_standard_script()));
    let mut mgr = SessionManager::new(reg);
    let id = mgr
        .start_session(StartAgentRequest {
            cwd: "/tmp/wt".into(),
            provider_profile_id: "fake".into(),
            transport: AgentTransportKind::CliWrapped,
            argv_suffix: vec![],
            env: BTreeMap::new(),
        })
        .unwrap();
    let rev_before = mgr.revision();
    mgr.stop_session(&id).unwrap();
    assert!(mgr.session(&id).is_none());
    assert_eq!(mgr.revision(), rev_before + 1);
}

#[test]
fn stop_session_unknown_id_errors() {
    let mut reg = ProviderRegistry::new();
    reg.register("fake", Arc::new(FakeProvider::with_standard_script()));
    let mut mgr = SessionManager::new(reg);
    let err = mgr
        .stop_session(&AgentSessionId::new("missing"))
        .unwrap_err();
    assert!(
        err.to_string().contains("unknown session"),
        "unexpected error: {err}"
    );
}
