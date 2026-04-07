#[path = "../src/agent_runtime.rs"]
mod agent_runtime;
#[path = "../../axis-app/src/review.rs"]
mod app_review;
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

use axis_agent_runtime::WorktreeService;
use axis_core::automation::AutomationRequest;
use axis_core::review::{DeskReviewPayload, ReviewFileChangeKind, ReviewLineKind};
use axis_core::worktree::WorktreeId;
use std::fs;
use std::path::Path;
use std::process::Command;
use support::{env_lock, send_request, workdesk_record};

fn run_git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .expect("spawn git");
    assert!(status.success(), "git {:?} failed", args);
}

fn init_repo_with_main(repo: &Path) {
    run_git(repo, &["init", "-b", "main"]);
    run_git(repo, &["config", "user.email", "axis-test@example.com"]);
    run_git(repo, &["config", "user.name", "axis test"]);
    fs::write(repo.join("README.md"), "hello\n").unwrap();
    run_git(repo, &["add", "README.md"]);
    run_git(repo, &["commit", "-m", "init"]);
}

fn workdesk_record_with_base_branch(
    workdesk_id: &str,
    workspace_root: &str,
    worktree_root: &str,
    base_branch: Option<&str>,
) -> axis_core::workdesk::WorkdeskRecord {
    let mut record = workdesk_record(workdesk_id, workspace_root, worktree_root);
    if let Some(binding) = record.worktree_binding.as_mut() {
        binding.base_branch = base_branch.map(str::to_string);
    }
    record
}

fn desk_review_from_daemon(socket_path: &Path, worktree_id: &WorktreeId) -> serde_json::Value {
    let response = send_request(
        socket_path,
        &AutomationRequest::DeskReviewSummary {
            worktree_id: worktree_id.clone(),
        },
    )
    .expect("desk review summary should be sent");
    assert!(response.ok, "desk review failed: {response:?}");
    response.result.expect("desk review result")
}

/// Parity: daemon automation matches the app's local `review.summary` fallback payload.
#[test]
fn review_surface_parity_daemon_matches_app_local_fallback_payload() {
    let _env_guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path();
    init_repo_with_main(repo);

    let wt_dir = temp.path().join("wt-parity");
    WorktreeService::create_worktree(repo, &wt_dir, "feature/review-parity", "main").unwrap();

    fs::write(wt_dir.join("tracked.md"), "line one\nline two\n").unwrap();
    run_git(&wt_dir, &["add", "tracked.md"]);
    run_git(&wt_dir, &["commit", "-m", "add tracked"]);
    fs::write(wt_dir.join("untracked-only.txt"), "secret\n").unwrap();

    // Match daemon scope: ensured workdesk supplies `base_branch: main` for merge-base review.
    let binding = WorktreeService::attach(&wt_dir, Some("main".to_string())).unwrap();
    let mut local = app_review::resolve_local_desk_review_payload(
        &WorktreeId::new(wt_dir.display().to_string()),
        &binding,
        None,
    )
    .expect("app local fallback payload")
    .payload;
    local.summary.last_inspected_at_ms = None;

    let socket_path = temp.path().join("axisd-review.sock");
    let data_dir = temp.path().join("daemon-data-review");
    let workspace_root = temp.path().join("ws-root");
    fs::create_dir_all(&workspace_root).unwrap();
    let server = request_handler::start_background_daemon(socket_path.clone(), data_dir).unwrap();
    let worktree_id = WorktreeId::new(wt_dir.display().to_string());
    let ensure = send_request(
        &socket_path,
        &AutomationRequest::WorkdeskEnsure {
            record: workdesk_record(
                "desk-review-parity",
                &workspace_root.display().to_string(),
                &worktree_id.0,
            ),
        },
    )
    .expect("ensure");
    assert!(ensure.ok, "{ensure:?}");

    let from_daemon = desk_review_from_daemon(&socket_path, &worktree_id);
    assert!(
        from_daemon.get("files").is_some(),
        "expected structured `files` array, got {from_daemon:?}"
    );
    assert!(
        from_daemon.get("changed_files").is_none(),
        "legacy `changed_files` should not appear: {from_daemon:?}"
    );
    let mut from_daemon_payload: DeskReviewPayload =
        serde_json::from_value(from_daemon).expect("decode daemon desk review");
    from_daemon_payload.summary.last_inspected_at_ms = None;
    assert_eq!(from_daemon_payload.summary, local.summary);
    assert_eq!(from_daemon_payload.files, local.files);
    assert_eq!(from_daemon_payload.truncated, local.truncated);
    drop(server);
}

#[test]
fn review_surface_parity_includes_hunks_for_textual_changes() {
    let _env_guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path();
    init_repo_with_main(repo);
    let wt_dir = temp.path().join("wt-hunks");
    WorktreeService::create_worktree(repo, &wt_dir, "feature/hunks", "main").unwrap();
    fs::write(wt_dir.join("a.txt"), "alpha\n").unwrap();
    run_git(&wt_dir, &["add", "a.txt"]);
    run_git(&wt_dir, &["commit", "-m", "a"]);
    fs::write(wt_dir.join("a.txt"), "alpha\nbeta\n").unwrap();

    let socket_path = temp.path().join("axisd-hunks.sock");
    let data_dir = temp.path().join("daemon-data-hunks");
    let workspace_root = temp.path().join("ws-hunks");
    fs::create_dir_all(&workspace_root).unwrap();
    let server = request_handler::start_background_daemon(socket_path.clone(), data_dir).unwrap();
    let worktree_id = WorktreeId::new(wt_dir.display().to_string());
    let ensure = send_request(
        &socket_path,
        &AutomationRequest::WorkdeskEnsure {
            record: workdesk_record(
                "desk-hunks",
                &workspace_root.display().to_string(),
                &worktree_id.0,
            ),
        },
    )
    .unwrap();
    assert!(ensure.ok);

    let v = desk_review_from_daemon(&socket_path, &worktree_id);
    let payload: DeskReviewPayload = serde_json::from_value(v).expect("decode DeskReviewPayload");
    let a = payload
        .files
        .iter()
        .find(|f| f.path == "a.txt")
        .expect("a.txt");
    assert!(
        a.hunks
            .iter()
            .any(|h| h.lines.iter().any(|l| !l.text.is_empty())),
        "expected non-empty hunk lines: {:?}",
        a.hunks
    );
    drop(server);
}

#[test]
fn review_surface_parity_ambiguous_base_branch_returns_error() {
    let _env_guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path();
    init_repo_with_main(repo);

    let socket_path = temp.path().join("axisd-ambiguous.sock");
    let data_dir = temp.path().join("daemon-data-ambiguous");
    let workspace_root = temp.path().join("ws-ambiguous");
    fs::create_dir_all(&workspace_root).unwrap();
    let server = request_handler::start_background_daemon(socket_path.clone(), data_dir).unwrap();
    let worktree_id = WorktreeId::new(repo.display().to_string());

    let ensure_main = send_request(
        &socket_path,
        &AutomationRequest::WorkdeskEnsure {
            record: workdesk_record_with_base_branch(
                "desk-ambiguous-main",
                &workspace_root.display().to_string(),
                &worktree_id.0,
                Some("main"),
            ),
        },
    )
    .expect("ensure main");
    assert!(ensure_main.ok, "{ensure_main:?}");

    let ensure_develop = send_request(
        &socket_path,
        &AutomationRequest::WorkdeskEnsure {
            record: workdesk_record_with_base_branch(
                "desk-ambiguous-develop",
                &workspace_root.display().to_string(),
                &worktree_id.0,
                Some("develop"),
            ),
        },
    )
    .expect("ensure develop");
    assert!(ensure_develop.ok, "{ensure_develop:?}");

    let response = send_request(
        &socket_path,
        &AutomationRequest::DeskReviewSummary {
            worktree_id: worktree_id.clone(),
        },
    )
    .expect("desk review summary should respond");
    assert!(!response.ok, "ambiguous review base should fail");
    let error = response.error.expect("failure should include error");
    assert!(
        error.contains("ambiguous review base branch"),
        "unexpected error: {error}"
    );

    drop(server);
}

#[test]
fn review_surface_parity_includes_conflicted_file() {
    let _env_guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path();
    init_repo_with_main(repo);
    fs::write(repo.join("conflict.txt"), "base\n").unwrap();
    run_git(repo, &["add", "conflict.txt"]);
    run_git(repo, &["commit", "-m", "base"]);
    run_git(repo, &["checkout", "-b", "branch-a"]);
    fs::write(repo.join("conflict.txt"), "side-a\n").unwrap();
    run_git(repo, &["add", "conflict.txt"]);
    run_git(repo, &["commit", "-m", "a"]);
    run_git(repo, &["checkout", "main"]);
    run_git(repo, &["checkout", "-b", "branch-b"]);
    fs::write(repo.join("conflict.txt"), "side-b\n").unwrap();
    run_git(repo, &["add", "conflict.txt"]);
    run_git(repo, &["commit", "-m", "b"]);
    let merge_status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge", "branch-a"])
        .status()
        .unwrap();
    assert!(!merge_status.success(), "expected merge conflict");

    let socket_path = temp.path().join("axisd-conflict.sock");
    let data_dir = temp.path().join("daemon-data-conflict");
    let workspace_root = temp.path().join("ws-conflict");
    fs::create_dir_all(&workspace_root).unwrap();
    let server =
        request_handler::start_background_daemon(socket_path.clone(), data_dir).expect("daemon");
    let worktree_id = WorktreeId::new(repo.display().to_string());
    let ensure = send_request(
        &socket_path,
        &AutomationRequest::WorkdeskEnsure {
            record: workdesk_record(
                "desk-conflict",
                &workspace_root.display().to_string(),
                &worktree_id.0,
            ),
        },
    )
    .unwrap();
    assert!(ensure.ok);

    let v = desk_review_from_daemon(&socket_path, &worktree_id);
    let payload: DeskReviewPayload = serde_json::from_value(v).unwrap();
    assert!(
        payload.files.iter().any(|f| f.path == "conflict.txt"),
        "files {:?}",
        payload.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
    drop(server);
}

#[test]
fn review_surface_parity_rename_only_has_no_text_hunks() {
    let _env_guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path();
    init_repo_with_main(repo);
    fs::write(repo.join("old_name.txt"), "same\n").unwrap();
    run_git(repo, &["add", "old_name.txt"]);
    run_git(repo, &["commit", "-m", "add file"]);
    run_git(repo, &["checkout", "-b", "feature/rename"]);
    run_git(repo, &["mv", "old_name.txt", "new_name.txt"]);
    run_git(repo, &["commit", "-m", "rename only"]);

    let socket_path = temp.path().join("axisd-rename.sock");
    let data_dir = temp.path().join("daemon-data-rename");
    let workspace_root = temp.path().join("ws-rename");
    fs::create_dir_all(&workspace_root).unwrap();
    let server = request_handler::start_background_daemon(socket_path.clone(), data_dir).unwrap();
    let worktree_id = WorktreeId::new(repo.display().to_string());
    let ensure = send_request(
        &socket_path,
        &AutomationRequest::WorkdeskEnsure {
            record: workdesk_record(
                "desk-rename",
                &workspace_root.display().to_string(),
                &worktree_id.0,
            ),
        },
    )
    .unwrap();
    assert!(ensure.ok);

    let v = desk_review_from_daemon(&socket_path, &worktree_id);
    let payload: DeskReviewPayload = serde_json::from_value(v).unwrap();
    let renamed = payload
        .files
        .iter()
        .find(|f| f.path == "new_name.txt")
        .expect("new_name.txt");
    assert_eq!(renamed.change_kind, ReviewFileChangeKind::Renamed);
    assert_eq!(renamed.old_path.as_deref(), Some("old_name.txt"));
    assert!(renamed.hunks.is_empty(), "rename-only: {:?}", renamed.hunks);
    drop(server);
}

#[test]
fn review_surface_parity_rename_with_edits_retains_text_hunks() {
    let _env_guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path();
    init_repo_with_main(repo);
    fs::write(repo.join("orig.rs"), "fn main() {}\n").unwrap();
    run_git(repo, &["add", "orig.rs"]);
    run_git(repo, &["commit", "-m", "orig"]);
    run_git(repo, &["checkout", "-b", "feature/rename-edit"]);
    run_git(repo, &["mv", "orig.rs", "renamed.rs"]);
    fs::write(repo.join("renamed.rs"), "fn main() { let _ = 1; }\n").unwrap();
    run_git(repo, &["add", "renamed.rs"]);
    run_git(repo, &["commit", "-m", "rename+edit"]);

    let socket_path = temp.path().join("axisd-rename-edit.sock");
    let data_dir = temp.path().join("daemon-data-rename-edit");
    let workspace_root = temp.path().join("ws-rename-edit");
    fs::create_dir_all(&workspace_root).unwrap();
    let server = request_handler::start_background_daemon(socket_path.clone(), data_dir).unwrap();
    let worktree_id = WorktreeId::new(repo.display().to_string());
    let ensure = send_request(
        &socket_path,
        &AutomationRequest::WorkdeskEnsure {
            record: workdesk_record(
                "desk-rename-edit",
                &workspace_root.display().to_string(),
                &worktree_id.0,
            ),
        },
    )
    .unwrap();
    assert!(ensure.ok);

    let v = desk_review_from_daemon(&socket_path, &worktree_id);
    let payload: DeskReviewPayload = serde_json::from_value(v).unwrap();
    let entry = payload
        .files
        .iter()
        .find(|f| f.path == "renamed.rs")
        .expect("renamed.rs");
    assert!(
        entry.hunks.iter().any(|h| !h.lines.is_empty()),
        "expected textual hunks: {:?}",
        entry.hunks
    );
    drop(server);
}

#[test]
fn review_surface_parity_deleted_file_has_removal_hunks() {
    let _env_guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path();
    init_repo_with_main(repo);
    fs::write(repo.join("gone.txt"), "bye\n").unwrap();
    run_git(repo, &["add", "gone.txt"]);
    run_git(repo, &["commit", "-m", "gone"]);
    run_git(repo, &["checkout", "-b", "feature/delete"]);
    fs::remove_file(repo.join("gone.txt")).unwrap();
    run_git(repo, &["rm", "gone.txt"]);
    run_git(repo, &["commit", "-m", "delete"]);

    let socket_path = temp.path().join("axisd-del.sock");
    let data_dir = temp.path().join("daemon-data-del");
    let workspace_root = temp.path().join("ws-del");
    fs::create_dir_all(&workspace_root).unwrap();
    let server = request_handler::start_background_daemon(socket_path.clone(), data_dir).unwrap();
    let worktree_id = WorktreeId::new(repo.display().to_string());
    let ensure = send_request(
        &socket_path,
        &AutomationRequest::WorkdeskEnsure {
            record: workdesk_record(
                "desk-delete",
                &workspace_root.display().to_string(),
                &worktree_id.0,
            ),
        },
    )
    .unwrap();
    assert!(ensure.ok);

    let v = desk_review_from_daemon(&socket_path, &worktree_id);
    let payload: DeskReviewPayload = serde_json::from_value(v).unwrap();
    let entry = payload
        .files
        .iter()
        .find(|f| f.path == "gone.txt")
        .expect("gone.txt");
    assert_eq!(entry.change_kind, ReviewFileChangeKind::Deleted);
    assert!(
        entry
            .hunks
            .iter()
            .any(|h| { h.lines.iter().any(|l| l.kind == ReviewLineKind::Removal) }),
        "expected removal lines: {:?}",
        entry.hunks
    );
    drop(server);
}
