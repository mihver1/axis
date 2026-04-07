//! Desk review payload construction from real git repos.

use axis_agent_runtime::{ReviewPayloadLimits, WorktreeService};
use axis_core::review::{ReviewFileChangeKind, ReviewLineKind};
use std::fs;
use std::path::Path;
use std::process::Command;
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
    run_git(repo, &["config", "user.email", "axis-test@example.com"]);
    run_git(repo, &["config", "user.name", "axis test"]);
    fs::write(repo.join("README.md"), "hello\n").unwrap();
    run_git(repo, &["add", "README.md"]);
    run_git(repo, &["commit", "-m", "init"]);
}

fn limits_open() -> ReviewPayloadLimits {
    ReviewPayloadLimits {
        max_files: 256,
        max_hunks_per_file: 64,
        max_lines_per_hunk: 4096,
    }
}

fn working_tree_dirty(repo: &Path) -> bool {
    WorktreeService::attach(repo, None).expect("attach").dirty
}

#[test]
fn review_payload_includes_committed_diff_and_untracked_file_entry() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_repo_with_main(repo);

    let wt_dir = tmp.path().join("wt-review");
    WorktreeService::create_worktree(repo, &wt_dir, "feature/review", "main").unwrap();

    fs::write(wt_dir.join("tracked.md"), "line one\nline two\n").unwrap();
    run_git(&wt_dir, &["add", "tracked.md"]);
    run_git(&wt_dir, &["commit", "-m", "add tracked"]);

    fs::write(wt_dir.join("untracked-only.txt"), "secret\n").unwrap();

    let payload = WorktreeService::review_payload(
        &wt_dir,
        Some("main"),
        working_tree_dirty(&wt_dir),
        limits_open(),
    )
    .unwrap();

    let tracked = payload
        .files
        .iter()
        .find(|f| f.path == "tracked.md")
        .expect("tracked.md entry");
    assert!(
        !tracked.hunks.is_empty(),
        "expected textual hunks for tracked.md: {:?}",
        tracked.hunks
    );
    assert!(
        tracked.hunks.iter().any(|h| !h.lines.is_empty()),
        "expected non-empty hunk lines"
    );

    let untracked = payload
        .files
        .iter()
        .find(|f| f.path == "untracked-only.txt")
        .expect("untracked file entry");
    assert!(
        untracked.hunks.is_empty(),
        "untracked file should be file-level only, got hunks {:?}",
        untracked.hunks
    );
    assert_eq!(untracked.change_kind, ReviewFileChangeKind::Added);

    assert_eq!(
        payload.summary.files_changed,
        payload.files.len() as u32,
        "summary file count should match payload entries"
    );
}

#[test]
fn review_payload_normalizes_spaced_paths_without_duplicates() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_repo_with_main(repo);

    fs::create_dir_all(repo.join("odd b")).unwrap();
    fs::write(repo.join("space name.txt"), "space v1\n").unwrap();
    fs::write(repo.join("odd b/file.txt"), "odd v1\n").unwrap();
    run_git(repo, &["add", "space name.txt", "odd b/file.txt"]);
    run_git(repo, &["commit", "-m", "add spaced files"]);

    fs::write(repo.join("space name.txt"), "space v2\n").unwrap();
    fs::write(repo.join("odd b/file.txt"), "odd v2\n").unwrap();

    let payload = WorktreeService::review_payload(
        repo,
        Some("main"),
        working_tree_dirty(repo),
        limits_open(),
    )
    .unwrap();
    let paths = payload
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        payload.files.len(),
        2,
        "expected exactly two entries: {paths:?}"
    );
    assert_eq!(
        paths
            .iter()
            .filter(|path| **path == "space name.txt")
            .count(),
        1,
        "expected one normalized spaced path entry: {paths:?}"
    );
    assert_eq!(
        paths
            .iter()
            .filter(|path| **path == "odd b/file.txt")
            .count(),
        1,
        "expected one normalized odd b path entry: {paths:?}"
    );
    assert!(
        !paths.iter().any(|path| path.contains('"')),
        "expected no quoted path entries: {paths:?}"
    );
}

#[test]
fn rename_without_content_change_is_file_level_without_fake_hunks() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_repo_with_main(repo);

    fs::write(repo.join("old_name.txt"), "same\n").unwrap();
    run_git(repo, &["add", "old_name.txt"]);
    run_git(repo, &["commit", "-m", "add file"]);

    run_git(repo, &["checkout", "-b", "feature/rename"]);
    run_git(repo, &["mv", "old_name.txt", "new_name.txt"]);
    run_git(repo, &["commit", "-m", "rename only"]);

    let payload = WorktreeService::review_payload(
        repo,
        Some("main"),
        working_tree_dirty(repo),
        limits_open(),
    )
    .unwrap();

    let renamed = payload
        .files
        .iter()
        .find(|f| f.path == "new_name.txt")
        .expect("rename target path");
    assert_eq!(renamed.change_kind, ReviewFileChangeKind::Renamed);
    assert_eq!(renamed.old_path.as_deref(), Some("old_name.txt"));
    assert!(
        renamed.hunks.is_empty(),
        "rename-only should not synthesize line hunks: {:?}",
        renamed.hunks
    );
}

#[test]
fn review_payload_parses_no_newline_metadata_line() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_repo_with_main(repo);

    // File without trailing newline — second commit adds a line; diff emits "\ No newline" marker.
    fs::write(repo.join("eof.txt"), "alpha").unwrap();
    run_git(repo, &["add", "eof.txt"]);
    run_git(repo, &["commit", "-m", "alpha no nl"]);

    fs::write(repo.join("eof.txt"), "alpha\nbeta\n").unwrap();

    let payload = WorktreeService::review_payload(
        repo,
        Some("main"),
        working_tree_dirty(repo),
        limits_open(),
    )
    .unwrap();
    let entry = payload
        .files
        .iter()
        .find(|f| f.path == "eof.txt")
        .expect("eof.txt");

    let has_no_newline = entry.hunks.iter().any(|h| {
        h.lines.iter().any(|line| {
            line.kind == ReviewLineKind::Metadata && line.text.contains("No newline at end of file")
        })
    });
    assert!(
        has_no_newline,
        "expected metadata line for no-newline marker, hunks {:?}",
        entry.hunks
    );
}

#[test]
fn multiple_hunks_in_one_file_are_preserved() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_repo_with_main(repo);

    let mut body = String::new();
    for i in 1..=80 {
        body.push_str(&format!("line{i}\n"));
    }
    fs::write(repo.join("wide.txt"), &body).unwrap();
    run_git(repo, &["add", "wide.txt"]);
    run_git(repo, &["commit", "-m", "wide"]);

    run_git(repo, &["checkout", "-b", "feature/wide"]);
    let mut edited = body;
    edited = edited.replacen("line2\n", "LINE2\n", 1);
    edited = edited.replacen("line79\n", "LINE79\n", 1);
    fs::write(repo.join("wide.txt"), edited).unwrap();
    run_git(repo, &["add", "wide.txt"]);
    run_git(repo, &["commit", "-m", "two distant edits"]);

    let payload = WorktreeService::review_payload(
        repo,
        Some("main"),
        working_tree_dirty(repo),
        limits_open(),
    )
    .unwrap();
    let entry = payload.files.iter().find(|f| f.path == "wide.txt").unwrap();
    assert!(
        entry.hunks.len() >= 2,
        "expected multiple hunks, got {}",
        entry.hunks.len()
    );
}

#[test]
fn deleted_file_keeps_identity_with_removal_hunks() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_repo_with_main(repo);

    fs::write(repo.join("gone.txt"), "bye\n").unwrap();
    run_git(repo, &["add", "gone.txt"]);
    run_git(repo, &["commit", "-m", "gone"]);

    run_git(repo, &["checkout", "-b", "feature/delete"]);
    fs::remove_file(repo.join("gone.txt")).unwrap();
    run_git(repo, &["rm", "gone.txt"]);
    run_git(repo, &["commit", "-m", "delete"]);

    let payload = WorktreeService::review_payload(
        repo,
        Some("main"),
        working_tree_dirty(repo),
        limits_open(),
    )
    .unwrap();
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
            .any(|h| h.lines.iter().any(|l| l.kind == ReviewLineKind::Removal)),
        "expected removal lines"
    );
}

#[test]
fn binary_change_is_placeholder_without_text_hunks() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_repo_with_main(repo);

    fs::write(repo.join("blob.bin"), [0u8, 1, 2]).unwrap();
    run_git(repo, &["add", "blob.bin"]);
    run_git(repo, &["commit", "-m", "bin"]);

    run_git(repo, &["checkout", "-b", "feature/bin"]);
    fs::write(repo.join("blob.bin"), [9u8, 9, 9, 9]).unwrap();
    run_git(repo, &["add", "blob.bin"]);
    run_git(repo, &["commit", "-m", "bin change"]);

    let payload = WorktreeService::review_payload(
        repo,
        Some("main"),
        working_tree_dirty(repo),
        limits_open(),
    )
    .unwrap();
    let entry = payload.files.iter().find(|f| f.path == "blob.bin").unwrap();
    assert!(
        entry.hunks.is_empty() || entry.hunks.iter().all(|h| h.lines.is_empty()),
        "binary file should not expose fake textual hunks: {:?}",
        entry.hunks
    );
}

#[test]
fn conflicted_file_appears_in_payload() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
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
    assert!(
        !merge_status.success(),
        "expected merge to stop with conflicts"
    );

    let payload = WorktreeService::review_payload(
        repo,
        Some("main"),
        working_tree_dirty(repo),
        limits_open(),
    )
    .unwrap();
    assert!(
        payload.files.iter().any(|f| f.path == "conflict.txt"),
        "expected conflicted path in payload, files {:?}",
        payload.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
}

#[test]
fn rename_with_edits_still_has_textual_hunks() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_repo_with_main(repo);

    fs::write(repo.join("orig.rs"), "fn main() {}\n").unwrap();
    run_git(repo, &["add", "orig.rs"]);
    run_git(repo, &["commit", "-m", "orig"]);

    run_git(repo, &["checkout", "-b", "feature/rename-edit"]);
    run_git(repo, &["mv", "orig.rs", "renamed.rs"]);
    fs::write(repo.join("renamed.rs"), "fn main() { let _ = 1; }\n").unwrap();
    run_git(repo, &["add", "renamed.rs"]);
    run_git(repo, &["commit", "-m", "rename+edit"]);

    let payload = WorktreeService::review_payload(
        repo,
        Some("main"),
        working_tree_dirty(repo),
        limits_open(),
    )
    .unwrap();
    let entry = payload
        .files
        .iter()
        .find(|f| f.path == "renamed.rs")
        .expect("renamed.rs");
    assert!(
        entry.hunks.iter().any(|h| !h.lines.is_empty()),
        "expected textual hunks for rename-with-edits"
    );
    // A single commit that `git mv` then edits often appears as delete+add in name-status/diff, not `R`.
    if entry.change_kind == ReviewFileChangeKind::Renamed {
        assert_eq!(entry.old_path.as_deref(), Some("orig.rs"));
    }
}

#[test]
fn truncation_sets_flags_when_hunk_line_cap_is_tight() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_repo_with_main(repo);

    let mut body = String::new();
    for i in 0..80 {
        body.push_str(&format!("line {i}\n"));
    }
    fs::write(repo.join("big.txt"), &body).unwrap();
    run_git(repo, &["add", "big.txt"]);
    run_git(repo, &["commit", "-m", "big"]);

    run_git(repo, &["checkout", "-b", "feature/big"]);
    // Large working-tree rewrite so a single hunk exceeds the line cap.
    let mut body2 = String::new();
    for i in 0..80 {
        body2.push_str(&format!("LINE {i}\n"));
    }
    fs::write(repo.join("big.txt"), &body2).unwrap();

    let tight = ReviewPayloadLimits {
        max_files: 256,
        max_hunks_per_file: 64,
        max_lines_per_hunk: 8,
    };
    let payload =
        WorktreeService::review_payload(repo, Some("main"), working_tree_dirty(repo), tight)
            .unwrap();
    assert!(payload.truncated, "payload should be marked truncated");
    let entry = payload.files.iter().find(|f| f.path == "big.txt").unwrap();
    assert!(
        entry.truncated || entry.hunks.iter().any(|h| h.truncated),
        "file or hunk should reflect truncation"
    );
}

#[test]
fn unborn_head_uses_porcelain_file_entries() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    run_git(repo, &["init", "-b", "main"]);
    run_git(repo, &["config", "user.email", "axis-test@example.com"]);
    run_git(repo, &["config", "user.name", "axis test"]);

    fs::write(repo.join("only.txt"), "x\n").unwrap();

    // Unborn HEAD: `attach` cannot run until first commit; tree is dirty from untracked files.
    let payload = WorktreeService::review_payload(repo, Some("main"), true, limits_open()).unwrap();
    assert!(
        payload.files.iter().any(|f| f.path == "only.txt"),
        "expected status-derived entry, got {:?}",
        payload.files
    );
}

#[test]
fn removal_hunks_get_anchor_new_line_for_jump() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_repo_with_main(repo);

    fs::write(repo.join("jump.txt"), "keep\nremove_me\nkeep2\n").unwrap();
    run_git(repo, &["add", "jump.txt"]);
    run_git(repo, &["commit", "-m", "jump"]);

    fs::write(repo.join("jump.txt"), "keep\nkeep2\n").unwrap();

    let payload = WorktreeService::review_payload(
        repo,
        Some("main"),
        working_tree_dirty(repo),
        limits_open(),
    )
    .unwrap();
    let entry = payload.files.iter().find(|f| f.path == "jump.txt").unwrap();
    let hunk = entry
        .hunks
        .iter()
        .find(|h| h.lines.iter().any(|l| l.kind == ReviewLineKind::Removal))
        .expect("removal hunk");
    assert!(
        hunk.anchor_new_line.is_some(),
        "anchor_new_line should help removals jump to surviving line"
    );
}

#[test]
fn parsed_removal_lines_are_jumpable_for_editor_navigation() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_repo_with_main(repo);

    fs::write(repo.join("jump.txt"), "keep\nremove_me\nkeep2\n").unwrap();
    run_git(repo, &["add", "jump.txt"]);
    run_git(repo, &["commit", "-m", "jump"]);

    fs::write(repo.join("jump.txt"), "keep\nkeep2\n").unwrap();

    let payload = WorktreeService::review_payload(
        repo,
        Some("main"),
        working_tree_dirty(repo),
        limits_open(),
    )
    .unwrap();
    let entry = payload.files.iter().find(|f| f.path == "jump.txt").unwrap();
    let removals: Vec<_> = entry
        .hunks
        .iter()
        .flat_map(|h| h.lines.iter())
        .filter(|l| l.kind == ReviewLineKind::Removal)
        .collect();
    assert!(
        !removals.is_empty(),
        "expected parsed removal rows from git diff"
    );
    for line in removals {
        assert!(
            line.jumpable,
            "parsed removal rows must be jumpable for editor navigation: {:?}",
            line
        );
    }
}

#[test]
fn summary_uncommitted_count_matches_visible_files_when_payload_truncated_by_file_cap() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_repo_with_main(repo);

    fs::write(repo.join("z_tracked.md"), "z\n").unwrap();
    run_git(repo, &["add", "z_tracked.md"]);
    run_git(repo, &["commit", "-m", "z"]);

    run_git(repo, &["checkout", "-b", "feature/trunc-summary"]);
    fs::write(repo.join("z_tracked.md"), "z2\n").unwrap();
    fs::write(repo.join("a_untracked.txt"), "only untracked\n").unwrap();

    let tight = ReviewPayloadLimits {
        max_files: 1,
        max_hunks_per_file: 64,
        max_lines_per_hunk: 4096,
    };
    let payload =
        WorktreeService::review_payload(repo, Some("main"), working_tree_dirty(repo), tight)
            .unwrap();

    assert!(payload.truncated, "expected file-cap truncation");
    assert_eq!(
        payload.files.len(),
        1,
        "fixture should only expose one file entry under cap"
    );
    assert_eq!(
        payload.files[0].path.as_str(),
        "a_untracked.txt",
        "first sorted path should be untracked a_untracked.txt"
    );
    let porcelain = WorktreeService::uncommitted_changed_files(repo).unwrap();
    let porcelain_set: std::collections::HashSet<_> = porcelain.iter().cloned().collect();
    let expected_visible_uncommitted = payload
        .files
        .iter()
        .filter(|f| porcelain_set.contains(&f.path))
        .count() as u32;
    assert!(
        porcelain.len() > payload.files.len(),
        "fixture needs more porcelain paths than visible files: porcelain={porcelain:?}"
    );
    assert_eq!(
        payload.summary.uncommitted_files, expected_visible_uncommitted,
        "uncommitted_files should count only paths present in the visible file list when truncated"
    );
    assert_eq!(
        payload.summary.files_changed,
        payload.files.len() as u32,
        "files_changed should stay aligned with visible entries"
    );
}

#[test]
fn two_file_committed_diff_maps_each_path_to_its_own_hunks() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_repo_with_main(repo);

    fs::write(repo.join("aaa.txt"), "aaa v1\n").unwrap();
    fs::write(repo.join("zzz.txt"), "zzz v1\n").unwrap();
    run_git(repo, &["add", "aaa.txt", "zzz.txt"]);
    run_git(repo, &["commit", "-m", "both"]);

    run_git(repo, &["checkout", "-b", "feature/two-paths"]);
    fs::write(repo.join("aaa.txt"), "aaa v2\n").unwrap();
    fs::write(repo.join("zzz.txt"), "zzz v2\n").unwrap();
    run_git(repo, &["add", "aaa.txt", "zzz.txt"]);
    run_git(repo, &["commit", "-m", "edit both"]);

    let payload = WorktreeService::review_payload(
        repo,
        Some("main"),
        working_tree_dirty(repo),
        limits_open(),
    )
    .unwrap();

    let aaa = payload
        .files
        .iter()
        .find(|f| f.path == "aaa.txt")
        .expect("aaa.txt");
    let zzz = payload
        .files
        .iter()
        .find(|f| f.path == "zzz.txt")
        .expect("zzz.txt");
    assert!(
        aaa.hunks
            .iter()
            .any(|h| h.lines.iter().any(|l| l.text.contains("aaa"))),
        "aaa entry should carry aaa hunk text, got {:?}",
        aaa.hunks
    );
    assert!(
        zzz.hunks
            .iter()
            .any(|h| h.lines.iter().any(|l| l.text.contains("zzz"))),
        "zzz entry should carry zzz hunk text, got {:?}",
        zzz.hunks
    );
    assert!(
        !aaa.hunks
            .iter()
            .any(|h| h.lines.iter().any(|l| l.text.contains("zzz"))),
        "aaa entry must not receive zzz diff lines"
    );
    assert!(
        !zzz.hunks
            .iter()
            .any(|h| h.lines.iter().any(|l| l.text.contains("aaa"))),
        "zzz entry must not receive aaa diff lines"
    );
}

#[test]
fn missing_base_branch_falls_back_to_head_for_diff() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_repo_with_main(repo);

    fs::write(repo.join("x.txt"), "v1\n").unwrap();
    run_git(repo, &["add", "x.txt"]);
    run_git(repo, &["commit", "-m", "x"]);

    fs::write(repo.join("x.txt"), "v2\n").unwrap();

    let payload = WorktreeService::review_payload(
        repo,
        Some("nonexistent-base"),
        working_tree_dirty(repo),
        limits_open(),
    )
    .unwrap();
    let entry = payload.files.iter().find(|f| f.path == "x.txt");
    assert!(
        entry.is_some(),
        "fallback to HEAD should still show working tree change, files {:?}",
        payload.files
    );
}

#[test]
fn existing_non_commit_base_ref_does_not_silently_fall_back_to_head() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_repo_with_main(repo);

    fs::write(repo.join("x.txt"), "v1\n").unwrap();
    run_git(repo, &["add", "x.txt"]);
    run_git(repo, &["commit", "-m", "x"]);
    fs::write(repo.join("x.txt"), "v2\n").unwrap();

    let tree_hash = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD^{tree}"])
        .output()
        .unwrap();
    assert!(tree_hash.status.success(), "expected tree hash");
    let tree_hash = String::from_utf8_lossy(&tree_hash.stdout)
        .trim()
        .to_string();
    run_git(repo, &["update-ref", "refs/tags/tree-base", &tree_hash]);

    let err = WorktreeService::review_payload(
        repo,
        Some("tree-base"),
        working_tree_dirty(repo),
        limits_open(),
    )
    .expect_err("existing non-commit base ref should not fall back to HEAD");
    let text = format!("{err:#}");
    assert!(
        text.contains("merge-base"),
        "expected merge-base failure, got {text}"
    );
}
