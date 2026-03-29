//! Git worktree inspection via the `git` CLI.

use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, Context};
use axis_core::review::DeskReviewPayload;
use axis_core::worktree::WorktreeBinding;

use crate::review_diff::{build_desk_review_payload, parse_porcelain_paths, ReviewPayloadLimits};

/// Helpers to create, attach, and inspect git worktrees.
pub struct WorktreeService;

impl WorktreeService {
    /// Creates a new worktree at `worktree_path` with branch `new_branch` starting from `base_branch`.
    pub fn create_worktree(
        repo_root: impl AsRef<Path>,
        worktree_path: impl AsRef<Path>,
        new_branch: &str,
        base_branch: &str,
    ) -> anyhow::Result<WorktreeBinding> {
        let repo_root = repo_root.as_ref();
        let worktree_path = worktree_path.as_ref();
        let status = Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .args(["worktree", "add", "-b", new_branch])
            .arg(worktree_path)
            .arg(base_branch)
            .status()
            .context("spawn git worktree add")?;
        if !status.success() {
            return Err(anyhow!("git worktree add failed with status {status}"));
        }
        Self::inspect(worktree_path, Some(base_branch.to_string()))
    }

    /// Treats `path` as an existing worktree (or repo) and reads current branch and status.
    pub fn attach(
        path: impl AsRef<Path>,
        base_branch: Option<String>,
    ) -> anyhow::Result<WorktreeBinding> {
        Self::inspect(path.as_ref(), base_branch)
    }

    /// Re-runs inspection for the same root and optional upstream base branch.
    pub fn refresh(binding: &WorktreeBinding) -> anyhow::Result<WorktreeBinding> {
        Self::inspect(Path::new(&binding.root_path), binding.base_branch.clone())
    }

    /// Paths differing between `base_branch` and `HEAD` (symmetric range).
    pub fn changed_files_since_base(
        root: impl AsRef<Path>,
        base_branch: &str,
    ) -> anyhow::Result<Vec<String>> {
        let root = root.as_ref();
        let spec = format!("{base_branch}...HEAD");
        let out = git_output(root, &["diff", "--name-only", &spec])?;
        Ok(parse_line_list(&out))
    }

    /// Working tree paths reported by `git status --porcelain -z`.
    pub fn uncommitted_changed_files(root: impl AsRef<Path>) -> anyhow::Result<Vec<String>> {
        let root = root.as_ref();
        let porcelain = git_output(root, &["status", "--porcelain", "-z"])?;
        Ok(parse_porcelain_paths(&porcelain))
    }

    /// Builds a structured desk review payload: working tree vs `merge-base(base_branch, HEAD)` when
    /// `base_branch` resolves, otherwise vs `HEAD` (or status-only when `HEAD` is unborn).
    pub fn review_payload(
        root: impl AsRef<Path>,
        base_branch: Option<&str>,
        working_tree_dirty: bool,
        limits: ReviewPayloadLimits,
    ) -> anyhow::Result<DeskReviewPayload> {
        build_desk_review_payload(
            root.as_ref(),
            base_branch,
            working_tree_dirty,
            limits,
        )
    }

    fn inspect(root: &Path, base_branch: Option<String>) -> anyhow::Result<WorktreeBinding> {
        let root_path = root.display().to_string();
        let branch = git_output(root, &["rev-parse", "--abbrev-ref", "HEAD"])?
            .trim()
            .to_string();
        let dirty = !git_output(root, &["status", "--porcelain"])?
            .trim()
            .is_empty();
        let (ahead, behind) = match base_branch.as_deref() {
            Some(base) => parse_ahead_behind(root, base)?,
            None => (0, 0),
        };
        Ok(WorktreeBinding {
            root_path,
            branch,
            base_branch,
            ahead,
            behind,
            dirty,
        })
    }
}

fn git_output(root: &Path, args: &[&str]) -> anyhow::Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .context("spawn git")?;
    if !out.status.success() {
        return Err(anyhow!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn parse_ahead_behind(root: &Path, base: &str) -> anyhow::Result<(u32, u32)> {
    let spec = format!("{base}...HEAD");
    let out = git_output(root, &["rev-list", "--left-right", "--count", &spec])?;
    let mut parts = out.split_whitespace();
    let left = parts.next();
    let right = parts.next();
    match (left, right) {
        (Some(b), Some(a)) => {
            let behind: u32 = b.parse().unwrap_or(0);
            let ahead: u32 = a.parse().unwrap_or(0);
            Ok((ahead, behind))
        }
        _ => Ok((0, 0)),
    }
}

fn parse_line_list(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}
