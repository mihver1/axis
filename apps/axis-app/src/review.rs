use axis_core::worktree::WorktreeBinding;
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeskReviewSummaryView {
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub dirty: bool,
    pub changed_files: Vec<String>,
    pub ready_for_review: bool,
}

pub(crate) fn merge_changed_files(base_changed: &[String], uncommitted: &[String]) -> Vec<String> {
    let mut merged = BTreeSet::new();
    merged.extend(base_changed.iter().cloned());
    merged.extend(uncommitted.iter().cloned());
    merged.into_iter().collect()
}

pub(crate) fn build_desk_review_summary_view(
    binding: &WorktreeBinding,
    changed_files: &[String],
) -> DeskReviewSummaryView {
    DeskReviewSummaryView {
        branch: binding.branch.clone(),
        ahead: binding.ahead,
        behind: binding.behind,
        dirty: binding.dirty,
        changed_files: changed_files.to_vec(),
        ready_for_review: !binding.dirty && !changed_files.is_empty(),
    }
}

pub(crate) fn refreshed_desk_review_summary_view(
    previous: Option<&DeskReviewSummaryView>,
    binding: Option<&WorktreeBinding>,
    changed_files: Option<&[String]>,
) -> Option<DeskReviewSummaryView> {
    match (binding, changed_files) {
        (Some(binding), Some(changed_files)) => {
            Some(build_desk_review_summary_view(binding, changed_files))
        }
        _ => previous.cloned(),
    }
}

pub(crate) fn review_status_label(view: &DeskReviewSummaryView) -> &'static str {
    if view.ready_for_review {
        "Ready for review"
    } else if view.dirty {
        "Dirty"
    } else {
        "In progress"
    }
}

pub(crate) fn review_changed_file_preview(
    view: &DeskReviewSummaryView,
    limit: usize,
) -> Vec<String> {
    let mut preview = view.changed_files.iter().take(limit).cloned().collect::<Vec<_>>();
    let remaining = view.changed_files.len().saturating_sub(preview.len());
    if remaining > 0 {
        preview.push(format!("+{remaining} more"));
    }
    preview
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(branch: &str, dirty: bool) -> WorktreeBinding {
        WorktreeBinding {
            root_path: "/tmp/axis".to_string(),
            branch: branch.to_string(),
            base_branch: Some("main".to_string()),
            ahead: 2,
            behind: 1,
            dirty,
        }
    }

    #[test]
    fn review_dirty_desk_is_not_ready_for_review() {
        let summary = build_desk_review_summary_view(
            &binding("feature/dirty", true),
            &[String::from("src/main.rs")],
        );

        assert!(!summary.ready_for_review);
        assert!(summary.dirty);
    }

    #[test]
    fn review_clean_changed_files_are_ready_for_review() {
        let summary = build_desk_review_summary_view(
            &binding("feature/ready", false),
            &[String::from("src/lib.rs"), String::from("Cargo.toml")],
        );

        assert!(summary.ready_for_review);
        assert_eq!(summary.branch, "feature/ready");
        assert_eq!(summary.changed_files.len(), 2);
    }

    #[test]
    fn review_refresh_keeps_last_known_state_when_new_metadata_is_stale() {
        let previous = build_desk_review_summary_view(
            &binding("feature/known", false),
            &[String::from("src/lib.rs")],
        );

        let refreshed =
            refreshed_desk_review_summary_view(Some(&previous), None, None)
                .expect("last known review summary should be retained");

        assert_eq!(refreshed, previous);
    }
}
