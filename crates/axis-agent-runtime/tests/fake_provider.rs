//! Scripted fake provider drives lifecycle and attention without subprocesses.

use std::collections::BTreeMap;
use std::sync::Arc;

use axis_agent_runtime::adapters::fake::{FakeProvider, FakeScriptStep};
use axis_agent_runtime::provider::{AgentProvider, ProviderRegistry, StartAgentRequest};
use axis_agent_runtime::SessionManager;
use axis_core::agent::{AgentAttention, AgentLifecycle, AgentTransportKind};

#[test]
fn fake_emits_starting_running_needs_review_completed_sequence() {
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

    mgr.poll_provider(&id).unwrap();
    assert_eq!(
        mgr.session(&id).unwrap().lifecycle,
        AgentLifecycle::Starting
    );

    mgr.poll_provider(&id).unwrap();
    assert_eq!(mgr.session(&id).unwrap().lifecycle, AgentLifecycle::Running);
    assert_eq!(mgr.session(&id).unwrap().attention, AgentAttention::Quiet);

    mgr.poll_provider(&id).unwrap();
    assert_eq!(
        mgr.session(&id).unwrap().attention,
        AgentAttention::NeedsReview
    );
    assert_eq!(mgr.session(&id).unwrap().lifecycle, AgentLifecycle::Running);

    mgr.poll_provider(&id).unwrap();
    assert_eq!(
        mgr.session(&id).unwrap().lifecycle,
        AgentLifecycle::Completed
    );

    mgr.poll_provider(&id).unwrap();
    assert_eq!(
        mgr.session(&id).unwrap().lifecycle,
        AgentLifecycle::Completed
    );
}

#[test]
fn stop_session_teardown_prevents_further_polls_on_fake() {
    let fake = Arc::new(FakeProvider::with_standard_script());
    let mut reg = ProviderRegistry::new();
    reg.register("fake", fake.clone());
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

    mgr.stop_session(&id).unwrap();

    let err = fake.as_ref().poll_events(&id).unwrap_err();
    assert!(
        err.to_string().contains("unknown fake session"),
        "unexpected error: {err}"
    );
}

#[test]
fn with_steps_drives_custom_lifecycle_event() {
    let mut reg = ProviderRegistry::new();
    reg.register(
        "fake",
        Arc::new(FakeProvider::with_steps(vec![
            FakeScriptStep::Lifecycle(AgentLifecycle::Starting),
            FakeScriptStep::Lifecycle(AgentLifecycle::Failed),
        ])),
    );
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

    mgr.poll_provider(&id).unwrap();
    assert_eq!(
        mgr.session(&id).unwrap().lifecycle,
        AgentLifecycle::Starting
    );

    mgr.poll_provider(&id).unwrap();
    assert_eq!(mgr.session(&id).unwrap().lifecycle, AgentLifecycle::Failed);
}
