//! Tests for multiple concurrent sessions managed by SessionManager.

use std::collections::BTreeMap;
use std::sync::Arc;

use axis_agent_runtime::adapters::fake::FakeProvider;
use axis_agent_runtime::provider::{ProviderRegistry, StartAgentRequest};
use axis_agent_runtime::SessionManager;
use axis_core::agent::{AgentLifecycle, AgentTransportKind};

fn start_request(profile: &str) -> StartAgentRequest {
    StartAgentRequest {
        cwd: "/tmp".into(),
        provider_profile_id: profile.to_string(),
        transport: AgentTransportKind::CliWrapped,
        argv_suffix: vec![],
        env: BTreeMap::new(),
        workdesk_id: None,
    }
}

/// Start three fake sessions and poll all in round-robin until all complete.
#[test]
fn three_sessions_all_complete() {
    let mut registry = ProviderRegistry::new();
    // Use a single FakeProvider instance: it produces unique session IDs per start() call
    // (fake-session-1, fake-session-2, fake-session-3) from a shared counter.
    registry.register("fake", Arc::new(FakeProvider::with_standard_script()));
    let mut manager = SessionManager::new(registry);

    let sid1 = manager.start_session(start_request("fake")).unwrap();
    let sid2 = manager.start_session(start_request("fake")).unwrap();
    let sid3 = manager.start_session(start_request("fake")).unwrap();

    let sessions = [&sid1, &sid2, &sid3];

    // Round-robin poll until all sessions reach Completed.
    // Standard script has 4 steps so 4 rounds suffice.
    for _ in 0..4 {
        for sid in &sessions {
            manager.poll_provider(sid).unwrap();
        }
    }

    for sid in &sessions {
        assert_eq!(
            manager.session(sid).unwrap().lifecycle,
            AgentLifecycle::Completed,
            "session {} did not complete",
            sid.0
        );
    }
}

/// Polling one session must not affect the lifecycle of another session.
///
/// Both sessions come from the same FakeProvider (standard script) so they
/// get unique IDs (fake-session-N). We poll only the first session through
/// all its script steps; the second session should remain untouched.
#[test]
fn polling_one_session_does_not_affect_another() {
    let mut registry = ProviderRegistry::new();
    // A single FakeProvider so both sessions get distinct IDs from the
    // shared monotonic counter (fake-session-1 and fake-session-2).
    registry.register("fake", Arc::new(FakeProvider::with_standard_script()));
    let mut manager = SessionManager::new(registry);

    let first_sid = manager.start_session(start_request("fake")).unwrap();
    let second_sid = manager.start_session(start_request("fake")).unwrap();

    // Poll the first session through all four standard-script steps.
    for _ in 0..4 {
        manager.poll_provider(&first_sid).unwrap();
    }

    assert_eq!(
        manager.session(&first_sid).unwrap().lifecycle,
        AgentLifecycle::Completed,
        "first session should have completed"
    );

    // The second session was never polled so its lifecycle must still be Planned.
    assert_eq!(
        manager.session(&second_sid).unwrap().lifecycle,
        AgentLifecycle::Planned,
        "second session lifecycle should be unaffected"
    );
}
