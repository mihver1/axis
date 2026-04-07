//! Tests that SessionManager rejects operations when the provider lacks the required capability.

use std::collections::BTreeMap;
use std::sync::Arc;

use axis_agent_runtime::provider::{
    AgentProvider, ProviderRegistry, StartAgentRequest, StartedSession,
};
use axis_agent_runtime::events::RuntimeEvent;
use axis_agent_runtime::SessionManager;
use axis_core::agent::{AgentSessionId, AgentTransportKind};
use axis_core::agent_history::{AgentApprovalRequestId, AgentSessionCapabilities};

/// A provider that declares no capabilities at all.
struct NoCapProvider;

impl AgentProvider for NoCapProvider {
    fn start(&self, _req: StartAgentRequest) -> anyhow::Result<StartedSession> {
        Ok(StartedSession {
            session_id: AgentSessionId::new("nocap-1"),
        })
    }

    fn poll_events(&self, _session_id: &AgentSessionId) -> anyhow::Result<Vec<RuntimeEvent>> {
        Ok(vec![])
    }

    fn capabilities(&self) -> AgentSessionCapabilities {
        AgentSessionCapabilities {
            turn_input: false,
            tool_calls: false,
            approvals: false,
            resume: false,
            terminal_attachment: false,
        }
    }

    fn stop(&self, _session_id: &AgentSessionId) -> anyhow::Result<()> {
        Ok(())
    }
}

fn new_manager_with_nocap() -> (SessionManager, AgentSessionId) {
    let mut reg = ProviderRegistry::new();
    reg.register("nocap", Arc::new(NoCapProvider));
    let mut mgr = SessionManager::new(reg);
    let id = mgr
        .start_session(StartAgentRequest {
            cwd: "/tmp".into(),
            provider_profile_id: "nocap".into(),
            transport: AgentTransportKind::CliWrapped,
            argv_suffix: vec![],
            env: BTreeMap::new(),
        })
        .unwrap();
    (mgr, id)
}

#[test]
fn send_turn_rejected_when_capability_missing() {
    let (mut mgr, id) = new_manager_with_nocap();
    let err = mgr.send_turn(&id, "hello").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("turn_input"),
        "expected error to mention 'turn_input', got: {msg}"
    );
}

#[test]
fn respond_approval_rejected_when_capability_missing() {
    let (mut mgr, id) = new_manager_with_nocap();
    let err = mgr
        .respond_approval(
            &id,
            &AgentApprovalRequestId::new("req-1"),
            true,
            None,
        )
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("approvals"),
        "expected error to mention 'approvals', got: {msg}"
    );
}

#[test]
fn resume_rejected_when_capability_missing() {
    let (mut mgr, id) = new_manager_with_nocap();
    let err = mgr.resume(&id).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("resume"),
        "expected error to mention 'resume', got: {msg}"
    );
}
