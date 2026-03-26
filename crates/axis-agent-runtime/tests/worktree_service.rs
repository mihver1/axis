//! Git worktree workflows against a temporary repository.

use std::fs;
use std::path::Path;
use std::process::Command;

use axis_agent_runtime::WorktreeService;
use tempfile::TempDir;

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
    run_git(
        repo,
        &["config", "user.email", "axis-test@example.com"],
    );
    run_git(repo, &["config", "user.name", "axis test"]);
    fs::write(repo.join("README.md"), "hello\n").unwrap();
    run_git(repo, &["add", "README.md"]);
    run_git(repo, &["commit", "-m", "init"]);
}

#[test]
fn create_worktree_tracks_branch_base_and_clean_tree() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_repo_with_main(repo);

    let wt_dir = tmp.path().join("wt-feature");
    let binding = WorktreeService::create_worktree(repo, &wt_dir, "feature/x", "main").unwrap();

    assert_eq!(binding.branch, "feature/x");
    assert_eq!(binding.base_branch.as_deref(), Some("main"));
    assert!(!binding.dirty);
    assert_eq!((binding.ahead, binding.behind), (0, 0));

    let refreshed = WorktreeService::refresh(&binding).unwrap();
    assert_eq!(refreshed.branch, binding.branch);
    assert!(!refreshed.dirty);
}

#[test]
fn attach_existing_worktree_reads_branch_and_dirty_state() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_repo_with_main(repo);

    let wt_dir = tmp.path().join("wt-attach");
    WorktreeService::create_worktree(repo, &wt_dir, "feature/y", "main").unwrap();

    let binding = WorktreeService::attach(&wt_dir, Some("main".into())).unwrap();
    assert_eq!(binding.root_path, wt_dir.display().to_string());
    assert_eq!(binding.branch, "feature/y");

    fs::write(wt_dir.join("dirty.txt"), "x").unwrap();
    let dirty = WorktreeService::refresh(&binding).unwrap();
    assert!(dirty.dirty);

    let uncommitted = WorktreeService::uncommitted_changed_files(&wt_dir).unwrap();
    assert!(
        uncommitted.iter().any(|p| p.contains("dirty.txt")),
        "expected dirty.txt in {uncommitted:?}"
    );
}

#[test]
fn ahead_and_changed_files_reflect_commits_on_feature_branch() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_repo_with_main(repo);

    let wt_dir = tmp.path().join("wt-ahead");
    let binding = WorktreeService::create_worktree(repo, &wt_dir, "feature/z", "main").unwrap();

    fs::write(wt_dir.join("feature.md"), "work\n").unwrap();
    run_git(&wt_dir, &["add", "feature.md"]);
    run_git(&wt_dir, &["commit", "-m", "feature commit"]);

    let updated = WorktreeService::refresh(&binding).unwrap();
    assert_eq!(updated.ahead, 1);
    assert_eq!(updated.behind, 0);

    let names = WorktreeService::changed_files_since_base(&wt_dir, "main").unwrap();
    assert!(
        names.iter().any(|n| n == "feature.md"),
        "expected feature.md in {names:?}"
    );
}
