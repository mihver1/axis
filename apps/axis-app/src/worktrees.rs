//! Pure helpers for workdesk ↔ worktree binding and template pane layouts.

use std::path::Path;
use std::process::Command;

use axis_core::worktree::WorktreeBinding;
use axis_core::{PaneId, PaneKind, PaneRecord, Point, Size, SurfaceId, SurfaceRecord};

pub(crate) const DEFAULT_SHELL_SIZE: Size = Size::new(920.0, 560.0);
pub(crate) const DEFAULT_AGENT_SIZE: Size = Size::new(720.0, 420.0);

/// Whether the UI should attach to an existing checkout or create a new worktree.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorktreeBindChoice {
    AttachExisting,
    CreateNew,
}

/// Decide attach vs create: prefer attach when the path exists and the user did not ask for a new tree.
#[allow(dead_code)]
pub fn choose_worktree_bind(path_exists: bool, prefer_new_worktree: bool) -> WorktreeBindChoice {
    if prefer_new_worktree {
        WorktreeBindChoice::CreateNew
    } else if path_exists {
        WorktreeBindChoice::AttachExisting
    } else {
        WorktreeBindChoice::CreateNew
    }
}

/// Seed a [`WorktreeBinding`] from desk metadata (cwd + branch); does not hit the filesystem.
pub fn binding_from_desk_paths(
    cwd: impl Into<String>,
    branch: impl Into<String>,
) -> WorktreeBinding {
    WorktreeBinding {
        root_path: cwd.into(),
        branch: branch.into(),
        base_branch: None,
        ahead: 0,
        behind: 0,
        dirty: false,
    }
}

/// Rehydrate a binding from desk metadata, preserving missing-yet-intentional paths but refreshing live git info.
pub fn refreshed_binding_from_desk_paths(cwd: &str, branch: &str) -> Option<WorktreeBinding> {
    let cwd = cwd.trim();
    if cwd.is_empty() {
        return None;
    }

    let seed = binding_from_desk_paths(cwd, branch.trim());
    Some(match refresh_worktree_metadata(&seed) {
        RefreshWorktreeOutcome::MissingPath => seed,
        RefreshWorktreeOutcome::Updated(binding) => binding,
    })
}

/// Single-line label for cards and chrome (compact path + branch).
pub fn format_compact_worktree_line(binding: &WorktreeBinding) -> String {
    format!(
        "{} @ {}",
        compact_path_tail(&binding.root_path),
        binding.branch.trim()
    )
}

fn compact_path_tail(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return "~".to_string();
    }
    if parts.len() == 1 {
        return parts[0].to_string();
    }
    format!("{}/{}", parts[parts.len() - 2], parts[parts.len() - 1])
}

/// Outcome of probing disk + git for refreshed worktree metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefreshWorktreeOutcome {
    MissingPath,
    Updated(WorktreeBinding),
}

/// Refresh branch (and existence) for a binding. Missing root clears the binding upstream via [`apply_refresh`].
pub fn refresh_worktree_metadata(binding: &WorktreeBinding) -> RefreshWorktreeOutcome {
    let root = binding.root_path.trim();
    if root.is_empty() || !Path::new(root).exists() {
        return RefreshWorktreeOutcome::MissingPath;
    }
    let mut next = binding.clone();
    if let Some(branch) = git_branch_at_path(Path::new(root)) {
        next.branch = branch;
    }
    RefreshWorktreeOutcome::Updated(next)
}

/// Apply a refresh result to an optional binding: missing path clears; updated replaces.
#[allow(dead_code)]
pub fn apply_refresh(
    _current: Option<WorktreeBinding>,
    outcome: RefreshWorktreeOutcome,
) -> Option<WorktreeBinding> {
    match outcome {
        RefreshWorktreeOutcome::MissingPath => None,
        RefreshWorktreeOutcome::Updated(b) => Some(b),
    }
}

fn git_branch_at_path(path: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8(output.stdout).ok()?;
    let branch = s.trim().to_string();
    if branch.is_empty() {
        return None;
    }
    Some(branch)
}

/// Pane layout for a workdesk template (shell + agent presets where applicable).
pub fn panes_for_template(template: super::WorkdeskTemplate) -> Vec<PaneRecord> {
    match template {
        super::WorkdeskTemplate::ShellDesk => vec![single_surface_pane(
            1,
            "Shell 1",
            PaneKind::Shell,
            Point::new(120.0, 96.0),
            DEFAULT_SHELL_SIZE,
        )],
        super::WorkdeskTemplate::AgentReview => vec![
            single_surface_pane(
                1,
                "Review Shell",
                PaneKind::Shell,
                Point::new(80.0, 96.0),
                DEFAULT_SHELL_SIZE,
            ),
            single_surface_pane(
                2,
                "Review Agent",
                PaneKind::Agent,
                Point::new(1048.0, 132.0),
                DEFAULT_AGENT_SIZE,
            ),
        ],
        super::WorkdeskTemplate::Debug => vec![
            single_surface_pane(
                1,
                "Repro Shell",
                PaneKind::Shell,
                Point::new(80.0, 96.0),
                DEFAULT_SHELL_SIZE,
            ),
            single_surface_pane(
                2,
                "Debug Agent",
                PaneKind::Agent,
                Point::new(1048.0, 120.0),
                DEFAULT_AGENT_SIZE,
            ),
        ],
        super::WorkdeskTemplate::Implementation => vec![
            single_surface_pane(
                1,
                "Build Shell",
                PaneKind::Shell,
                Point::new(80.0, 96.0),
                DEFAULT_SHELL_SIZE,
            ),
            single_surface_pane(
                2,
                "Implement Agent",
                PaneKind::Agent,
                Point::new(1048.0, 132.0),
                DEFAULT_AGENT_SIZE,
            ),
        ],
    }
}

/// Build a new workdesk from template metadata: panes + transient worktree binding from cwd/branch.
pub fn create_desk_from_template(
    name: impl Into<String>,
    summary: impl Into<String>,
    template: super::WorkdeskTemplate,
    metadata: super::WorkdeskMetadata,
) -> super::WorkdeskState {
    let name = name.into();
    let summary = summary.into();
    let panes = panes_for_template(template);
    let mut desk = super::WorkdeskState::new(name, summary, panes);
    desk.metadata = metadata;
    desk.worktree_binding =
        refreshed_binding_from_desk_paths(&desk.metadata.cwd, &desk.metadata.branch);
    desk
}

fn single_surface_pane(
    raw_id: u64,
    title: impl Into<String>,
    kind: PaneKind,
    position: Point,
    size: Size,
) -> PaneRecord {
    let title = title.into();
    let pane_id = PaneId::new(raw_id);
    let surface_id = SurfaceId::new(raw_id);
    PaneRecord::new(
        pane_id,
        position,
        size,
        SurfaceRecord::new(surface_id, title, kind),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn compact_worktree_line_formats_tail_and_branch() {
        let b = binding_from_desk_paths("/Users/dev/projects/axis", "main");
        assert_eq!(format_compact_worktree_line(&b), "projects/axis @ main");
    }

    #[test]
    fn choose_worktree_bind_prefers_attach_when_path_exists_and_not_forcing_new() {
        assert_eq!(
            choose_worktree_bind(true, false),
            WorktreeBindChoice::AttachExisting
        );
        assert_eq!(
            choose_worktree_bind(false, false),
            WorktreeBindChoice::CreateNew
        );
        assert_eq!(
            choose_worktree_bind(true, true),
            WorktreeBindChoice::CreateNew
        );
    }

    #[test]
    fn refresh_clears_when_root_path_missing() {
        let b = binding_from_desk_paths("/this/path/does/not/exist-AXIS-999", "main");
        let out = refresh_worktree_metadata(&b);
        assert_eq!(out, RefreshWorktreeOutcome::MissingPath);
        assert!(apply_refresh(Some(b), out).is_none());
    }

    #[test]
    fn refresh_updates_branch_when_git_repo_present() {
        let tmp = std::env::temp_dir().join(format!("axis-wt-git-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let status = Command::new("git").arg("-C").arg(&tmp).arg("init").status();
        if status.map(|s| !s.success()).unwrap_or(true) {
            let _ = fs::remove_dir_all(&tmp);
            return;
        }
        fs::write(tmp.join("marker.txt"), "x").unwrap();
        if !Command::new("git")
            .arg("-C")
            .arg(&tmp)
            .args(["add", "marker.txt"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            let _ = fs::remove_dir_all(&tmp);
            return;
        }
        if !Command::new("git")
            .arg("-C")
            .arg(&tmp)
            .args(["commit", "-m", "init"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            let _ = fs::remove_dir_all(&tmp);
            return;
        }

        let root = tmp.to_string_lossy().to_string();
        let b = binding_from_desk_paths(&root, "stale-branch");
        let RefreshWorktreeOutcome::Updated(updated) = refresh_worktree_metadata(&b) else {
            let _ = fs::remove_dir_all(&tmp);
            panic!("expected Updated");
        };
        assert_ne!(updated.branch, "stale-branch");
        assert_eq!(updated.root_path, root);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn refreshed_binding_from_blank_cwd_is_none() {
        assert_eq!(refreshed_binding_from_desk_paths("   ", "main"), None);
    }
}
