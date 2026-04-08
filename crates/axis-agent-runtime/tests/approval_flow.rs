//! Tests for approval flow using the FakeProvider.

use std::collections::BTreeMap;
use std::sync::Arc;

use axis_agent_runtime::adapters::fake::FakeProvider;
use axis_agent_runtime::provider::{ProviderRegistry, StartAgentRequest};
use axis_agent_runtime::SessionManager;
use axis_core::agent::{AgentAttention, AgentLifecycle, AgentSessionId, AgentTransportKind};
use axis_core::agent_history::AgentApprovalRequestId;

fn make_fake_session() -> (SessionManager, AgentSessionId) {
    let fake = Arc::new(FakeProvider::with_standard_script());
    let mut registry = ProviderRegistry::new();
    registry.register("fake", fake);
    let mut manager = SessionManager::new(registry);
    let sid = manager
        .start_session(StartAgentRequest {
            cwd: "/tmp".into(),
            provider_profile_id: "fake".to_string(),
            transport: AgentTransportKind::CliWrapped,
            argv_suffix: vec![],
            env: BTreeMap::new(),
            workdesk_id: None,
        })
        .unwrap();
    (manager, sid)
}

/// Poll through all standard script steps and verify NeedsReview attention is reached.
#[test]
fn fake_session_reaches_needs_review_attention() {
    let (mut manager, sid) = make_fake_session();

    // Step 1: Starting
    manager.poll_provider(&sid).unwrap();
    assert_eq!(manager.session(&sid).unwrap().lifecycle, AgentLifecycle::Starting);

    // Step 2: Running
    manager.poll_provider(&sid).unwrap();
    assert_eq!(manager.session(&sid).unwrap().lifecycle, AgentLifecycle::Running);

    // Step 3: NeedsReview attention
    manager.poll_provider(&sid).unwrap();
    assert_eq!(
        manager.session(&sid).unwrap().attention,
        AgentAttention::NeedsReview
    );
    // Lifecycle stays Running while awaiting review
    assert_eq!(manager.session(&sid).unwrap().lifecycle, AgentLifecycle::Running);
}

/// Advance session then call respond_approval; verify no error is returned.
#[test]
fn respond_approval_succeeds_on_fake_provider() {
    let (mut manager, sid) = make_fake_session();

    // Advance through the standard script steps
    manager.poll_provider(&sid).unwrap(); // Starting
    manager.poll_provider(&sid).unwrap(); // Running
    manager.poll_provider(&sid).unwrap(); // NeedsReview

    // Respond to approval — FakeProvider accepts any approval request id
    manager
        .respond_approval(
            &sid,
            &AgentApprovalRequestId::new("fake-approval-1"),
            true,
            None,
        )
        .unwrap();
}

/// Advance to Running then call send_turn; verify no error is returned.
#[test]
fn send_turn_succeeds_on_fake_provider() {
    let (mut manager, sid) = make_fake_session();

    // Advance to Running state
    manager.poll_provider(&sid).unwrap(); // Starting
    manager.poll_provider(&sid).unwrap(); // Running

    // FakeProvider accepts send_turn regardless of lifecycle
    manager.send_turn(&sid, "continue please").unwrap();
}
