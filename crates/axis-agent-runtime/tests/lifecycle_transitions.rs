//! Tests for the lifecycle state machine validation in [`SessionManager`].

use std::collections::BTreeMap;
use std::sync::Arc;

use axis_agent_runtime::adapters::fake::{FakeProvider, FakeScriptStep};
use axis_agent_runtime::events::RuntimeEvent;
use axis_agent_runtime::provider::{ProviderRegistry, StartAgentRequest};
use axis_agent_runtime::SessionManager;
use axis_core::agent::{AgentLifecycle, AgentTransportKind};

fn make_manager() -> (SessionManager, axis_core::agent::AgentSessionId) {
    let mut reg = ProviderRegistry::new();
    // Use a custom script so the session starts at Planned and we control all transitions.
    reg.register(
        "fake",
        Arc::new(FakeProvider::with_steps(vec![
            FakeScriptStep::Lifecycle(AgentLifecycle::Starting),
        ])),
    );
    let mut mgr = SessionManager::new(reg);
    let id = mgr
        .start_session(StartAgentRequest {
            cwd: "/tmp".into(),
            provider_profile_id: "fake".into(),
            transport: AgentTransportKind::CliWrapped,
            argv_suffix: vec![],
            env: BTreeMap::new(),
        })
        .unwrap();
    (mgr, id)
}

// ---------------------------------------------------------------------------
// Valid transitions
// ---------------------------------------------------------------------------

#[test]
fn valid_starting_to_running() {
    let (mut mgr, id) = make_manager();
    // Advance to Starting first.
    mgr.apply_events([RuntimeEvent::Lifecycle {
        session_id: id.clone(),
        lifecycle: AgentLifecycle::Starting,
    }])
    .unwrap();
    let rev_before = mgr.revision();

    mgr.transition_lifecycle(&id, AgentLifecycle::Running)
        .unwrap();

    assert!(mgr.revision() > rev_before, "revision must bump on valid transition");
    assert_eq!(
        mgr.session(&id).unwrap().lifecycle,
        AgentLifecycle::Running
    );
}

#[test]
fn valid_running_to_waiting() {
    let (mut mgr, id) = make_manager();
    mgr.apply_events([
        RuntimeEvent::Lifecycle {
            session_id: id.clone(),
            lifecycle: AgentLifecycle::Starting,
        },
        RuntimeEvent::Lifecycle {
            session_id: id.clone(),
            lifecycle: AgentLifecycle::Running,
        },
    ])
    .unwrap();

    mgr.transition_lifecycle(&id, AgentLifecycle::Waiting)
        .unwrap();

    assert_eq!(
        mgr.session(&id).unwrap().lifecycle,
        AgentLifecycle::Waiting
    );
}

#[test]
fn valid_running_to_completed() {
    let (mut mgr, id) = make_manager();
    mgr.apply_events([
        RuntimeEvent::Lifecycle {
            session_id: id.clone(),
            lifecycle: AgentLifecycle::Starting,
        },
        RuntimeEvent::Lifecycle {
            session_id: id.clone(),
            lifecycle: AgentLifecycle::Running,
        },
    ])
    .unwrap();

    mgr.transition_lifecycle(&id, AgentLifecycle::Completed)
        .unwrap();

    assert_eq!(
        mgr.session(&id).unwrap().lifecycle,
        AgentLifecycle::Completed
    );
}

#[test]
fn valid_waiting_to_running() {
    let (mut mgr, id) = make_manager();
    mgr.apply_events([
        RuntimeEvent::Lifecycle {
            session_id: id.clone(),
            lifecycle: AgentLifecycle::Starting,
        },
        RuntimeEvent::Lifecycle {
            session_id: id.clone(),
            lifecycle: AgentLifecycle::Running,
        },
        RuntimeEvent::Lifecycle {
            session_id: id.clone(),
            lifecycle: AgentLifecycle::Waiting,
        },
    ])
    .unwrap();

    mgr.transition_lifecycle(&id, AgentLifecycle::Running)
        .unwrap();

    assert_eq!(
        mgr.session(&id).unwrap().lifecycle,
        AgentLifecycle::Running
    );
}

// ---------------------------------------------------------------------------
// Invalid transitions — transition_lifecycle should hard-error
// ---------------------------------------------------------------------------

#[test]
fn invalid_completed_to_running_errors() {
    let (mut mgr, id) = make_manager();
    // Drive session to Completed through valid path.
    mgr.apply_events([
        RuntimeEvent::Lifecycle {
            session_id: id.clone(),
            lifecycle: AgentLifecycle::Starting,
        },
        RuntimeEvent::Lifecycle {
            session_id: id.clone(),
            lifecycle: AgentLifecycle::Running,
        },
        RuntimeEvent::Lifecycle {
            session_id: id.clone(),
            lifecycle: AgentLifecycle::Completed,
        },
    ])
    .unwrap();
    assert_eq!(
        mgr.session(&id).unwrap().lifecycle,
        AgentLifecycle::Completed
    );

    let err = mgr
        .transition_lifecycle(&id, AgentLifecycle::Running)
        .unwrap_err();
    assert!(
        err.to_string().contains("invalid lifecycle transition"),
        "unexpected error message: {err}"
    );
    // Lifecycle must be unchanged after the failed transition.
    assert_eq!(
        mgr.session(&id).unwrap().lifecycle,
        AgentLifecycle::Completed
    );
}

#[test]
fn invalid_running_to_planned_errors() {
    let (mut mgr, id) = make_manager();
    mgr.apply_events([
        RuntimeEvent::Lifecycle {
            session_id: id.clone(),
            lifecycle: AgentLifecycle::Starting,
        },
        RuntimeEvent::Lifecycle {
            session_id: id.clone(),
            lifecycle: AgentLifecycle::Running,
        },
    ])
    .unwrap();
    assert_eq!(
        mgr.session(&id).unwrap().lifecycle,
        AgentLifecycle::Running
    );

    let err = mgr
        .transition_lifecycle(&id, AgentLifecycle::Planned)
        .unwrap_err();
    assert!(
        err.to_string().contains("invalid lifecycle transition"),
        "unexpected error message: {err}"
    );
    assert_eq!(
        mgr.session(&id).unwrap().lifecycle,
        AgentLifecycle::Running
    );
}

// ---------------------------------------------------------------------------
// Invalid provider events — apply_events should warn and skip (no revision bump)
// ---------------------------------------------------------------------------

#[test]
fn invalid_provider_event_is_skipped_not_errored() {
    let (mut mgr, id) = make_manager();
    let rev_before = mgr.revision();

    // Planned → Completed is not a valid transition; the event should be silently skipped.
    mgr.apply_events([RuntimeEvent::Lifecycle {
        session_id: id.clone(),
        lifecycle: AgentLifecycle::Completed,
    }])
    .unwrap(); // must not return an error

    assert_eq!(
        mgr.revision(),
        rev_before,
        "revision must not change when invalid provider event is skipped"
    );
    assert_eq!(
        mgr.session(&id).unwrap().lifecycle,
        AgentLifecycle::Planned,
        "lifecycle must remain unchanged after skipped event"
    );
}

// ---------------------------------------------------------------------------
// Noop — same state should not bump revision
// ---------------------------------------------------------------------------

#[test]
fn noop_same_state_does_not_bump_revision() {
    let (mut mgr, id) = make_manager();
    // Advance to Running.
    mgr.apply_events([
        RuntimeEvent::Lifecycle {
            session_id: id.clone(),
            lifecycle: AgentLifecycle::Starting,
        },
        RuntimeEvent::Lifecycle {
            session_id: id.clone(),
            lifecycle: AgentLifecycle::Running,
        },
    ])
    .unwrap();
    let rev_before = mgr.revision();

    // Transitioning to the same state should be a noop.
    mgr.transition_lifecycle(&id, AgentLifecycle::Running)
        .unwrap();

    assert_eq!(
        mgr.revision(),
        rev_before,
        "revision must not bump on noop lifecycle transition"
    );
}

#[test]
fn noop_via_apply_events_does_not_bump_revision() {
    let (mut mgr, id) = make_manager();
    mgr.apply_events([RuntimeEvent::Lifecycle {
        session_id: id.clone(),
        lifecycle: AgentLifecycle::Starting,
    }])
    .unwrap();
    let rev_before = mgr.revision();

    // Sending the same lifecycle event again is a noop.
    mgr.apply_events([RuntimeEvent::Lifecycle {
        session_id: id.clone(),
        lifecycle: AgentLifecycle::Starting,
    }])
    .unwrap();

    assert_eq!(
        mgr.revision(),
        rev_before,
        "revision must not bump on repeated identical lifecycle event"
    );
}
