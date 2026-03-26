//! Integration tests for agent lifecycle, attention, review summary, and automation schema.

use axis_core::agent::{
    AgentAttention, AgentLifecycle, AgentSessionId, AgentSessionRecord, AgentTransportKind,
};
use axis_core::automation::AutomationRequest;
use axis_core::worktree::{ReviewSummary, WorktreeBinding, WorktreeId};
use axis_core::SurfaceId;

#[test]
fn agent_lifecycle_json_round_trips() {
    for state in [
        AgentLifecycle::Planned,
        AgentLifecycle::Starting,
        AgentLifecycle::Running,
        AgentLifecycle::Waiting,
        AgentLifecycle::Completed,
        AgentLifecycle::Failed,
        AgentLifecycle::Cancelled,
    ] {
        let json = serde_json::to_string(&state).expect("serialize lifecycle");
        let back: AgentLifecycle = serde_json::from_str(&json).expect("deserialize lifecycle");
        assert_eq!(back, state, "json was {json}");
    }
}

#[test]
fn agent_attention_json_round_trips() {
    for state in [
        AgentAttention::Quiet,
        AgentAttention::Working,
        AgentAttention::NeedsInput,
        AgentAttention::NeedsReview,
        AgentAttention::Error,
    ] {
        let json = serde_json::to_string(&state).expect("serialize attention");
        let back: AgentAttention = serde_json::from_str(&json).expect("deserialize attention");
        assert_eq!(back, state, "json was {json}");
    }
}

#[test]
fn review_summary_defaults() {
    let s = ReviewSummary::default();
    assert_eq!(s.files_changed, 0);
    assert_eq!(s.uncommitted_files, 0);
    assert!(!s.ready_for_review);
    assert_eq!(s.last_inspected_at_ms, None);

    let json = serde_json::to_string(&s).unwrap();
    assert_eq!(json, "{}");

    let back: ReviewSummary = serde_json::from_str("{}").unwrap();
    assert_eq!(back, s);
}

#[test]
fn worktree_binding_uses_dirty_not_setup_complete() {
    let b = WorktreeBinding {
        root_path: "/wt".to_string(),
        branch: "main".to_string(),
        base_branch: None,
        ahead: 0,
        behind: 0,
        dirty: true,
    };
    let v = serde_json::to_value(&b).unwrap();
    assert_eq!(v["dirty"], true);
    assert!(v.get("setup_complete").is_none());

    let json = r#"{"root_path":"/wt","branch":"main","dirty":false}"#;
    let back: WorktreeBinding = serde_json::from_str(json).unwrap();
    assert!(!back.dirty);
}

#[test]
fn agent_session_record_surface_id_round_trips_as_ui_surface_id() {
    let rec = AgentSessionRecord {
        id: AgentSessionId::new("sess-1"),
        provider_profile_id: "codex".to_string(),
        transport: AgentTransportKind::CliWrapped,
        workdesk_id: Some("desk-1".to_string()),
        surface_id: Some(SurfaceId::new(99)),
        cwd: "/wt".to_string(),
        lifecycle: AgentLifecycle::Running,
        attention: AgentAttention::Working,
        status_message: "ok".to_string(),
    };
    let json = serde_json::to_string(&rec).unwrap();
    assert!(
        json.contains("\"surface_id\":99"),
        "surface_id should serialize as bare u64 (transparent SurfaceId), got {json}"
    );

    let back: AgentSessionRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(back.surface_id, Some(SurfaceId::new(99)));

    let none_surf: AgentSessionRecord = serde_json::from_str(
        r#"{"id":"x","provider_profile_id":"p","transport":"cli_wrapped","cwd":"/","lifecycle":"running","attention":"quiet","status_message":""}"#,
    )
    .unwrap();
    assert_eq!(none_surf.surface_id, None);
}

#[test]
fn automation_request_encodes_agent_start() {
    let req = AutomationRequest::AgentStart {
        worktree_id: WorktreeId::new("wt-1"),
        provider_profile_id: "codex".to_string(),
        argv: vec!["--verbose".to_string()],
    };
    let v = serde_json::to_value(&req).unwrap();
    assert_eq!(v["method"], "agent.start");
    assert_eq!(v["params"]["worktree_id"], "wt-1");
    assert_eq!(v["params"]["provider_profile_id"], "codex");
    assert_eq!(v["params"]["argv"], serde_json::json!(["--verbose"]));
}

#[test]
fn automation_request_encodes_worktree_create_or_attach() {
    let req = AutomationRequest::WorktreeCreateOrAttach {
        repo_root: "/repo".to_string(),
        branch: Some("feature/x".to_string()),
        attach_path: None,
    };
    let v = serde_json::to_value(&req).unwrap();
    assert_eq!(v["method"], "worktree.create_or_attach");
    assert_eq!(v["params"]["repo_root"], "/repo");
    assert_eq!(v["params"]["branch"], "feature/x");
    assert!(
        v["params"].get("attach_path").is_none(),
        "omit null optional attach_path, got {}",
        v["params"]
    );
}

#[test]
fn automation_worktree_status_and_agent_stop_use_newtypes_and_round_trip() {
    let status = AutomationRequest::WorktreeStatus {
        worktree_id: WorktreeId::new("wt-9"),
    };
    let v = serde_json::to_value(&status).unwrap();
    assert_eq!(v["method"], "worktree.status");
    assert_eq!(v["params"]["worktree_id"], "wt-9");
    let back: AutomationRequest = serde_json::from_value(v).unwrap();
    assert_eq!(
        back,
        AutomationRequest::WorktreeStatus {
            worktree_id: WorktreeId::new("wt-9")
        }
    );

    let stop = AutomationRequest::AgentStop {
        agent_session_id: AgentSessionId::new("as-7"),
    };
    let v = serde_json::to_value(&stop).unwrap();
    assert_eq!(v["method"], "agent.stop");
    assert_eq!(v["params"]["agent_session_id"], "as-7");
    let back: AutomationRequest = serde_json::from_value(v).unwrap();
    assert_eq!(
        back,
        AutomationRequest::AgentStop {
            agent_session_id: AgentSessionId::new("as-7")
        }
    );
}

#[test]
fn automation_agent_list_optional_worktree_id_round_trips() {
    let list = AutomationRequest::AgentList {
        worktree_id: Some(WorktreeId::new("wt-a")),
    };
    let v = serde_json::to_value(&list).unwrap();
    assert_eq!(v["params"]["worktree_id"], "wt-a");
    let back: AutomationRequest = serde_json::from_value(v).unwrap();
    assert_eq!(
        back,
        AutomationRequest::AgentList {
            worktree_id: Some(WorktreeId::new("wt-a"))
        }
    );

    let list_none = AutomationRequest::AgentList {
        worktree_id: None,
    };
    let v = serde_json::to_value(&list_none).unwrap();
    assert!(v["params"].get("worktree_id").is_none());
}
