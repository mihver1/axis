#[path = "../src/agent_runtime.rs"]
mod agent_runtime;
#[path = "../src/gui_launcher.rs"]
mod gui_launcher;
#[path = "../src/persistence.rs"]
mod persistence;
#[path = "../src/pty_host.rs"]
mod pty_host;
#[path = "../src/registry.rs"]
mod registry;
#[path = "../src/request_handler.rs"]
mod request_handler;
#[path = "../src/transcript_store.rs"]
mod transcript_store;

mod support;

use axis_core::agent::AgentAttention;
use axis_core::automation::AutomationRequest;
use axis_core::workdesk::WorkdeskId;
use axis_core::worktree::WorktreeId;
use axis_core::SurfaceId;
use std::fs;
use support::{
    create_executable_script, env_lock, poll_until_attention, send_request, workdesk_record,
    EnvVarGuard,
};

#[test]
fn control_plane_parity_state_current_returns_workdesk_snapshot_and_filtered_sessions() {
    let _env_guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("tempdir should exist");
    let socket_path = temp.path().join("axisd.sock");
    let data_dir = temp.path().join("daemon-data");
    let wrapper = temp.path().join("claude-code-wrapper.sh");
    let workspace_root = temp.path().join("repo");
    let worktree_root = temp.path().join("repo-control-plane");
    fs::create_dir_all(&workspace_root).expect("workspace root should exist");
    fs::create_dir_all(&worktree_root).expect("worktree root should exist");
    create_executable_script(&wrapper, "#!/bin/sh\nsleep 60\n");
    let _wrapper_guard = EnvVarGuard::set("AXIS_CLAUDE_CODE_BIN", &wrapper);

    let server = request_handler::start_background_daemon(socket_path.clone(), data_dir)
        .expect("daemon should start");
    let worktree_id = WorktreeId::new(worktree_root.display().to_string());
    let workdesk_id = "desk-control-plane";

    let ensure = send_request(
        &socket_path,
        &AutomationRequest::WorkdeskEnsure {
            record: workdesk_record(
                workdesk_id,
                &workspace_root.display().to_string(),
                &worktree_id.0,
            ),
        },
    )
    .expect("workdesk ensure should succeed");
    assert!(ensure.ok, "workdesk ensure failed: {ensure:?}");

    let start = send_request(
        &socket_path,
        &AutomationRequest::AgentStart {
            worktree_id: worktree_id.clone(),
            provider_profile_id: "claude-code".to_string(),
            argv: vec![],
            workdesk_id: Some(WorkdeskId::new(workdesk_id)),
            surface_id: Some(SurfaceId::new(7)),
        },
    )
    .expect("agent start should succeed");
    assert!(start.ok);

    let state = send_request(
        &socket_path,
        &AutomationRequest::StateCurrent {
            workdesk_id: Some(workdesk_id.to_string()),
        },
    )
    .expect("state.current should succeed");
    assert!(state.ok, "state.current should be handled by axisd");

    let result = state.result.expect("state payload should exist");
    assert_eq!(result["control_plane"], "axisd");
    assert_eq!(result["workdesk"]["workdesk_id"], workdesk_id);
    assert_eq!(result["worktree_id"], worktree_id.0);
    let sessions = result["agent_sessions"]
        .as_array()
        .expect("agent_sessions should be an array");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["workdesk_id"], workdesk_id);

    drop(server);
}

#[test]
fn control_plane_parity_state_current_accepts_workdesk_name_selector() {
    let _env_guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("tempdir should exist");
    let socket_path = temp.path().join("axisd.sock");
    let data_dir = temp.path().join("daemon-data");
    let workspace_root = temp.path().join("repo");
    let worktree_root = temp.path().join("repo-name-selector");
    fs::create_dir_all(&workspace_root).expect("workspace root should exist");
    fs::create_dir_all(&worktree_root).expect("worktree root should exist");

    let server = request_handler::start_background_daemon(socket_path.clone(), data_dir)
        .expect("daemon should start");
    let worktree_id = WorktreeId::new(worktree_root.display().to_string());
    let workdesk_id = "desk-name-selector";
    let workdesk_name = "Implementation Desk";

    let ensure = send_request(
        &socket_path,
        &AutomationRequest::WorkdeskEnsure {
            record: workdesk_record(
                workdesk_id,
                &workspace_root.display().to_string(),
                &worktree_id.0,
            ),
        },
    )
    .expect("workdesk ensure should succeed");
    assert!(ensure.ok, "workdesk ensure failed: {ensure:?}");

    let state = send_request(
        &socket_path,
        &AutomationRequest::StateCurrent {
            workdesk_id: Some(workdesk_name.to_string()),
        },
    )
    .expect("state.current should succeed");
    assert!(state.ok, "state.current should accept desk names as selectors");
    assert_eq!(state.result.expect("state payload should exist")["workdesk"]["workdesk_id"], workdesk_id);

    drop(server);
}

#[test]
fn control_plane_parity_attention_next_returns_highest_priority_session_for_workdesk() {
    let _env_guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("tempdir should exist");
    let socket_path = temp.path().join("axisd.sock");
    let data_dir = temp.path().join("daemon-data");
    let wrapper = temp.path().join("codex-wrapper.sh");
    let workspace_root = temp.path().join("repo");
    let worktree_root = temp.path().join("repo-attention");
    fs::create_dir_all(&workspace_root).expect("workspace root should exist");
    fs::create_dir_all(&worktree_root).expect("worktree root should exist");
    create_executable_script(
        &wrapper,
        "#!/bin/sh\nprintf 'AXIS_ATTENTION needs_review\\n'; sleep 60\n",
    );
    let _wrapper_guard = EnvVarGuard::set("AXIS_CODEX_BIN", &wrapper);

    let server = request_handler::start_background_daemon(socket_path.clone(), data_dir)
        .expect("daemon should start");
    let worktree_id = WorktreeId::new(worktree_root.display().to_string());
    let workdesk_id = "desk-attention";

    let ensure = send_request(
        &socket_path,
        &AutomationRequest::WorkdeskEnsure {
            record: workdesk_record(
                workdesk_id,
                &workspace_root.display().to_string(),
                &worktree_id.0,
            ),
        },
    )
    .expect("workdesk ensure should succeed");
    assert!(ensure.ok, "workdesk ensure failed: {ensure:?}");

    let start = send_request(
        &socket_path,
        &AutomationRequest::AgentStart {
            worktree_id: worktree_id.clone(),
            provider_profile_id: "codex".to_string(),
            argv: vec![],
            workdesk_id: Some(WorkdeskId::new(workdesk_id)),
            surface_id: Some(SurfaceId::new(11)),
        },
    )
    .expect("agent start should succeed");
    assert!(start.ok);

    let expected = poll_until_attention(&socket_path, &worktree_id, AgentAttention::NeedsReview);
    let response = send_request(
        &socket_path,
        &AutomationRequest::AttentionNext {
            workdesk_id: Some(workdesk_id.to_string()),
        },
    )
    .expect("attention.next should succeed");
    assert!(response.ok, "attention.next should be handled by axisd");

    let result = response.result.expect("attention payload should exist");
    assert_eq!(result["control_plane"], "axisd");
    assert_eq!(result["workdesk"]["workdesk_id"], workdesk_id);
    assert_eq!(result["agent_session"]["id"], expected.id.0);
    assert_eq!(result["agent_session"]["attention"], "needs_review");

    drop(server);
}
