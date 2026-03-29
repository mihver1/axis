//! Desk review summary projection, payload resolution, and local review state (selection + hunk markers).

use axis_agent_runtime::{ReviewPayloadLimits, WorktreeService};
use axis_core::review::{DeskReviewPayload, ReviewFileDiff, ReviewHunk, ReviewLine, ReviewLineKind};
use axis_core::workdesk::WorkdeskId;
use axis_core::worktree::{ReviewSummary, WorktreeBinding, WorktreeId};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeskReviewSummaryView {
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub dirty: bool,
    pub changed_files: Vec<String>,
    pub ready_for_review: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeskReviewPayloadResolution {
    pub payload: DeskReviewPayload,
    pub summary: DeskReviewSummaryView,
    pub stale: bool,
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

pub(crate) fn build_desk_review_summary_view_from_payload(
    binding: &WorktreeBinding,
    payload: &DeskReviewPayload,
) -> DeskReviewSummaryView {
    let changed_files: Vec<String> = payload.files.iter().map(|f| f.path.clone()).collect();
    DeskReviewSummaryView {
        branch: binding.branch.clone(),
        ahead: binding.ahead,
        behind: binding.behind,
        dirty: binding.dirty,
        changed_files,
        ready_for_review: payload.summary.ready_for_review,
    }
}

pub(crate) fn resolve_local_desk_review_payload(
    worktree_id: &WorktreeId,
    binding: &WorktreeBinding,
    cached: Option<&DeskReviewPayload>,
) -> Result<DeskReviewPayloadResolution, String> {
    let cached = reusable_review_payload_cache(cached, binding);
    resolve_local_desk_review_payload_with(
        worktree_id,
        binding,
        cached,
        || {
            WorktreeService::review_payload(
                &binding.root_path,
                binding.base_branch.as_deref(),
                binding.dirty,
                ReviewPayloadLimits::default(),
            )
            .map_err(|error| error.to_string())
        },
        || {
            let base_changed = binding
                .base_branch
                .as_deref()
                .map(|base_branch| {
                    WorktreeService::changed_files_since_base(&binding.root_path, base_branch)
                        .map_err(|error| error.to_string())
                })
                .transpose()?
                .unwrap_or_default();
            let uncommitted = WorktreeService::uncommitted_changed_files(&binding.root_path)
                .map_err(|error| error.to_string())?;
            Ok((merge_changed_files(&base_changed, &uncommitted), uncommitted.len()))
        },
        unix_time_ms(),
    )
}

fn resolve_local_desk_review_payload_with<RichLoader, CompactLoader>(
    worktree_id: &WorktreeId,
    binding: &WorktreeBinding,
    cached: Option<&DeskReviewPayload>,
    rich_loader: RichLoader,
    compact_loader: CompactLoader,
    now_ms: u64,
) -> Result<DeskReviewPayloadResolution, String>
where
    RichLoader: FnOnce() -> Result<DeskReviewPayload, String>,
    CompactLoader: FnOnce() -> Result<(Vec<String>, usize), String>,
{
    let cached = reusable_review_payload_cache(cached, binding);
    match rich_loader() {
        Ok(payload) => {
            let summary = build_desk_review_summary_view_from_payload(binding, &payload);
            Ok(DeskReviewPayloadResolution {
                payload,
                summary,
                stale: false,
            })
        }
        Err(_) => {
            let (merged_changed_files, uncommitted_count) = compact_loader()?;
            if let Some(mut cached) = cached.cloned() {
                cached.worktree_id = worktree_id.clone();
                cached.summary.last_inspected_at_ms = Some(now_ms);
                Ok(DeskReviewPayloadResolution {
                    payload: cached,
                    summary: build_desk_review_summary_view(binding, &merged_changed_files),
                    stale: true,
                })
            } else {
                let payload = DeskReviewPayload {
                    worktree_id: worktree_id.clone(),
                    summary: ReviewSummary {
                        files_changed: merged_changed_files.len() as u32,
                        uncommitted_files: uncommitted_count as u32,
                        ready_for_review: false,
                        last_inspected_at_ms: Some(now_ms),
                    },
                    files: vec![],
                    truncated: false,
                };
                let summary = build_desk_review_summary_view(binding, &merged_changed_files);
                Ok(DeskReviewPayloadResolution {
                    payload,
                    summary,
                    stale: false,
                })
            }
        }
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
    let mut preview = view
        .changed_files
        .iter()
        .take(limit)
        .cloned()
        .collect::<Vec<_>>();
    let remaining = view.changed_files.len().saturating_sub(preview.len());
    if remaining > 0 {
        preview.push(format!("+{remaining} more"));
    }
    preview
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Local-only per-hunk review marker (not persisted across app restarts).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HunkReviewState {
    Todo,
    Reviewed,
    FollowUp,
}

/// Desk-scoped stable identity for correlating local hunk state across refreshes.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct ReviewHunkKey {
    pub workdesk_id: WorkdeskId,
    pub path: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub header: String,
}

impl ReviewHunkKey {
    pub(crate) fn from_hunk(workdesk_id: &WorkdeskId, path: &str, hunk: &ReviewHunk) -> Self {
        Self {
            workdesk_id: workdesk_id.clone(),
            path: path.to_string(),
            old_start: hunk.old_start,
            old_lines: hunk.old_lines,
            new_start: hunk.new_start,
            new_lines: hunk.new_lines,
            header: hunk.header.clone(),
        }
    }
}

/// Aggregated per-file review progress derived from local hunk markers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FileReviewAggregate {
    NoHunks,
    AllReviewed,
    HasFollowUp,
    InProgress,
}

/// Combined review payload + desk-local selection and hunk markers (pure state for refresh/tests).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewPanelState {
    pub workdesk_id: WorkdeskId,
    pub payload: DeskReviewPayload,
    pub selected_file: usize,
    pub selected_hunk: Option<usize>,
    pub hunk_states: HashMap<ReviewHunkKey, HunkReviewState>,
    pub stale_notice: Option<String>,
    pub setup_notice: Option<String>,
}

/// Inputs that affect how a refresh merges local state with a new payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewPanelRefreshContext {
    pub workdesk_id: WorkdeskId,
    pub worktree_rebound: bool,
    pub stale_rich_payload: bool,
    pub setup_notice: Option<String>,
}

impl ReviewPanelState {
    pub(crate) fn for_payload(workdesk_id: WorkdeskId, payload: DeskReviewPayload) -> Self {
        let selected_file = default_selected_file(&payload);
        let selected_hunk = default_selected_hunk(&payload, selected_file);
        Self {
            workdesk_id,
            payload,
            selected_file,
            selected_hunk,
            hunk_states: HashMap::new(),
            stale_notice: None,
            setup_notice: None,
        }
    }

    pub(crate) fn selected_file_path(&self) -> Option<&str> {
        self.payload.files.get(self.selected_file).map(|f| f.path.as_str())
    }

    pub(crate) fn selected_hunk_header(&self) -> Option<&str> {
        let file = self.payload.files.get(self.selected_file)?;
        let hunk_index = self.selected_hunk?;
        file.hunks.get(hunk_index).map(|h| h.header.as_str())
    }

    pub(crate) fn set_hunk_state(&mut self, key: ReviewHunkKey, state: HunkReviewState) {
        self.hunk_states.insert(key, state);
    }

    pub(crate) fn clear_hunk_state(&mut self, key: &ReviewHunkKey) {
        self.hunk_states.remove(key);
    }
}

pub(crate) fn file_review_aggregate(
    workdesk_id: &WorkdeskId,
    file: &ReviewFileDiff,
    hunk_states: &HashMap<ReviewHunkKey, HunkReviewState>,
) -> FileReviewAggregate {
    if file.hunks.is_empty() {
        return FileReviewAggregate::NoHunks;
    }
    let mut has_follow_up = false;
    let mut all_reviewed = true;
    for hunk in &file.hunks {
        let key = ReviewHunkKey::from_hunk(workdesk_id, &file.path, hunk);
        match hunk_states.get(&key).copied().unwrap_or(HunkReviewState::Todo) {
            HunkReviewState::Todo => all_reviewed = false,
            HunkReviewState::FollowUp => {
                has_follow_up = true;
                all_reviewed = false;
            }
            HunkReviewState::Reviewed => {}
        }
    }
    if has_follow_up {
        FileReviewAggregate::HasFollowUp
    } else if all_reviewed {
        FileReviewAggregate::AllReviewed
    } else {
        FileReviewAggregate::InProgress
    }
}

/// Hunk-level actions (mark reviewed / follow-up / clear) are disabled when there are no textual hunks.
pub(crate) fn review_hunk_actions_disabled(file: &ReviewFileDiff) -> bool {
    file.hunks.is_empty()
}

/// Line jumps may still apply for some entries; this gates the local-only hunk action row.
pub(crate) fn review_local_hunk_actions_enabled(file: &ReviewFileDiff) -> bool {
    !review_hunk_actions_disabled(file)
}

/// Editor line (1-based) to focus when opening from a structured diff row, matching workspace-search semantics.
/// User-visible notice when jump-to-editor from the review diff cannot open a surface.
pub(crate) fn review_editor_open_failed_notice(path: &str, error: &str) -> String {
    format!("Could not open editor for review ({path}): {error}")
}

pub(crate) fn editor_jump_line_for_review_row(hunk: &ReviewHunk, line: &ReviewLine) -> Option<u32> {
    if !line.jumpable {
        return None;
    }
    match line.kind {
        ReviewLineKind::Addition => line.new_line,
        ReviewLineKind::Context => line.new_line.or(line.old_line),
        ReviewLineKind::Removal => hunk
            .anchor_new_line
            .or_else(|| nearest_surviving_new_line_for_removal(hunk, line)),
        ReviewLineKind::Metadata => None,
    }
}

fn nearest_surviving_new_line_for_removal(hunk: &ReviewHunk, removed: &ReviewLine) -> Option<u32> {
    let removed_old = removed.old_line;
    let mut after: Option<u32> = None;
    let mut before: Option<u32> = None;
    for row in &hunk.lines {
        if !matches!(row.kind, ReviewLineKind::Context | ReviewLineKind::Addition) {
            continue;
        }
        let Some(n) = row.new_line else {
            continue;
        };
        match removed_old {
            Some(ro) => {
                if n >= ro {
                    after = Some(after.map_or(n, |a| a.min(n)));
                } else if n < ro {
                    before = Some(before.map_or(n, |b| b.max(n)));
                }
            }
            None => return Some(n),
        }
    }
    after.or(before).or(Some(hunk.new_start))
}

pub(crate) fn review_file_hunkless_notice(file: &ReviewFileDiff) -> String {
    use axis_core::review::ReviewFileChangeKind;
    match file.change_kind {
        ReviewFileChangeKind::Renamed => file
            .old_path
            .as_ref()
            .map(|old| format!("Renamed from {old}. No textual diff hunks for this entry."))
            .unwrap_or_else(|| "Rename-only entry with no textual hunks.".to_string()),
        ReviewFileChangeKind::Added => {
            "New file with no line diff yet (empty or non-text).".to_string()
        }
        ReviewFileChangeKind::Deleted => {
            "Deleted file — locate it via workspace search or history.".to_string()
        }
        ReviewFileChangeKind::Modified => {
            "No textual hunks (binary or excluded). Review at file level only.".to_string()
        }
    }
}

fn default_selected_file(payload: &DeskReviewPayload) -> usize {
    if payload.files.is_empty() {
        0
    } else {
        0
    }
}

fn default_selected_hunk(payload: &DeskReviewPayload, file_index: usize) -> Option<usize> {
    payload
        .files
        .get(file_index)
        .and_then(|f| if f.hunks.is_empty() { None } else { Some(0) })
}

fn clamp_file_index(index: usize, file_count: usize) -> usize {
    if file_count == 0 {
        0
    } else {
        index.min(file_count - 1)
    }
}

fn find_file_index_by_path(payload: &DeskReviewPayload, path: &str) -> Option<usize> {
    payload.files.iter().position(|f| f.path == path)
}

fn find_hunk_index_by_key(payload: &DeskReviewPayload, file_index: usize, key: &ReviewHunkKey) -> Option<usize> {
    let file = payload.files.get(file_index)?;
    file.hunks.iter().position(|h| {
        ReviewHunkKey::from_hunk(&key.workdesk_id, &file.path, h) == *key
    })
}

pub(crate) fn reusable_review_payload_cache<'a>(
    cached_payload: Option<&'a DeskReviewPayload>,
    binding: &WorktreeBinding,
) -> Option<&'a DeskReviewPayload> {
    cached_payload.filter(|cached| !review_payload_worktree_rebound(Some(cached), binding))
}

/// Returns whether the cached rich payload belongs to a materially different worktree root.
///
/// The comparison canonicalizes real paths when possible and otherwise falls back to a lexical
/// normalization that collapses `.` and redundant separators so path spelling alone does not
/// reset local review progress.
pub(crate) fn review_payload_worktree_rebound(
    cached_payload: Option<&DeskReviewPayload>,
    binding: &WorktreeBinding,
) -> bool {
    cached_payload.is_some_and(|cached| {
        normalized_worktree_root(&cached.worktree_id.0) != normalized_worktree_root(&binding.root_path)
    })
}

fn normalized_worktree_root(path: &str) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| normalize_path_lexically(Path::new(path)))
}

fn normalize_path_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// Applies a newly fetched structured payload to desk-local review UI state (selection, markers, notices).
pub(crate) fn merge_review_local_after_fetch(
    workdesk_id: &WorkdeskId,
    previous_cache: Option<&DeskReviewPayload>,
    previous_local: &ReviewPanelLocalState,
    new_payload: DeskReviewPayload,
    ctx: ReviewPanelRefreshContext,
) -> (DeskReviewPayload, ReviewPanelLocalState) {
    let previous_panel = previous_cache.map(|p| ReviewPanelState {
        workdesk_id: workdesk_id.clone(),
        payload: p.clone(),
        selected_file: previous_local.selected_file,
        selected_hunk: previous_local.selected_hunk,
        hunk_states: previous_local.hunk_states.clone(),
        stale_notice: previous_local.stale_notice.clone(),
        setup_notice: previous_local.setup_notice.clone(),
    });
    let merged = refresh_review_panel_state(previous_panel.as_ref(), new_payload, ctx);
    let local = ReviewPanelLocalState {
        selected_file: merged.selected_file,
        selected_hunk: merged.selected_hunk,
        hunk_states: merged.hunk_states,
        stale_notice: merged.stale_notice,
        setup_notice: merged.setup_notice,
    };
    (merged.payload, local)
}

/// Merge a new structured payload into panel state, preserving selection and hunk markers when identities match.
pub(crate) fn refresh_review_panel_state(
    previous: Option<&ReviewPanelState>,
    new_payload: DeskReviewPayload,
    ctx: ReviewPanelRefreshContext,
) -> ReviewPanelState {
    let workdesk_id = ctx.workdesk_id.clone();

    if previous.is_none() || ctx.worktree_rebound {
        let mut next = ReviewPanelState::for_payload(workdesk_id.clone(), new_payload);
        next.stale_notice = stale_notice_for_refresh(None, ctx.stale_rich_payload);
        next.setup_notice = ctx.setup_notice.clone();
        return next;
    }

    let previous = previous.expect("checked");
    let file_count = new_payload.files.len();

    let mut hunk_states: HashMap<ReviewHunkKey, HunkReviewState> = HashMap::new();
    if !ctx.worktree_rebound {
        let valid_keys = valid_hunk_keys(&workdesk_id, &new_payload);
        for (key, state) in &previous.hunk_states {
            if valid_keys.contains(key) {
                hunk_states.insert(key.clone(), *state);
            }
        }
    }

    let prev_path = previous
        .payload
        .files
        .get(previous.selected_file)
        .map(|f| f.path.clone());

    let selected_file = prev_path
        .as_ref()
        .and_then(|path| find_file_index_by_path(&new_payload, path))
        .unwrap_or_else(|| clamp_file_index(previous.selected_file, file_count));

    let selected_hunk = if file_count == 0 {
        None
    } else if let Some(p) = prev_path.as_ref() {
        if find_file_index_by_path(&new_payload, p).is_none() {
            None
        } else {
            previous
                .payload
                .files
                .get(previous.selected_file)
                .filter(|pf| pf.path == *p)
                .and_then(|pf| previous.selected_hunk.and_then(|hi| pf.hunks.get(hi)))
                .and_then(|prev_hunk| {
                    let key = ReviewHunkKey::from_hunk(&workdesk_id, p, prev_hunk);
                    find_hunk_index_by_key(&new_payload, selected_file, &key)
                })
        }
    } else {
        None
    }
    .or_else(|| default_selected_hunk(&new_payload, selected_file));

    ReviewPanelState {
        workdesk_id,
        payload: new_payload,
        selected_file,
        selected_hunk,
        hunk_states,
        stale_notice: stale_notice_for_refresh(previous.stale_notice.as_deref(), ctx.stale_rich_payload),
        setup_notice: ctx.setup_notice.clone(),
    }
}

fn valid_hunk_keys(workdesk_id: &WorkdeskId, payload: &DeskReviewPayload) -> HashSet<ReviewHunkKey> {
    let mut keys = HashSet::new();
    for file in &payload.files {
        for hunk in &file.hunks {
            keys.insert(ReviewHunkKey::from_hunk(workdesk_id, &file.path, hunk));
        }
    }
    keys
}

fn stale_notice_for_refresh(previous: Option<&str>, stale_rich_payload: bool) -> Option<String> {
    if stale_rich_payload {
        Some(
            previous
                .map(String::from)
                .unwrap_or_else(|| {
                    "Diff details may be out of date; file list reflects the latest summary."
                        .to_string()
                }),
        )
    } else {
        None
    }
}

/// Best-effort setup/runtime messaging when git cannot provide the preferred review base.
pub(crate) fn review_workspace_setup_notice(binding: &WorktreeBinding) -> Option<String> {
    let root = Path::new(&binding.root_path);
    if !git_ref_verifies(root, "HEAD") {
        return Some(
            "This repository has no commits yet; review lists working tree paths from git status."
                .to_string(),
        );
    }
    if let Some(base) = binding.base_branch.as_deref() {
        if !git_ref_verifies(root, base) {
            return Some(format!(
                "Base branch '{base}' is not available in this clone; comparing against HEAD instead."
            ));
        }
    }
    None
}

fn git_ref_verifies(root: &Path, reference: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--verify", reference])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Desk-local slice stored on [`WorkdeskState`] while the structured payload lives in `review_payload_cache`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ReviewPanelLocalState {
    pub selected_file: usize,
    pub selected_hunk: Option<usize>,
    pub hunk_states: HashMap<ReviewHunkKey, HunkReviewState>,
    pub stale_notice: Option<String>,
    pub setup_notice: Option<String>,
}

impl ReviewPanelLocalState {
    pub(crate) fn selected_file_path<'a>(&self, payload: &'a DeskReviewPayload) -> Option<&'a str> {
        payload.files.get(self.selected_file).map(|f| f.path.as_str())
    }

    pub(crate) fn refresh_from_payload(
        &self,
        workdesk_id: &WorkdeskId,
        previous_payload: &DeskReviewPayload,
        new_payload: DeskReviewPayload,
        ctx: ReviewPanelRefreshContext,
    ) -> Self {
        let combined = refresh_review_panel_state(
            Some(&ReviewPanelState {
                workdesk_id: workdesk_id.clone(),
                payload: previous_payload.clone(),
                selected_file: self.selected_file,
                selected_hunk: self.selected_hunk,
                hunk_states: self.hunk_states.clone(),
                stale_notice: self.stale_notice.clone(),
                setup_notice: self.setup_notice.clone(),
            }),
            new_payload,
            ctx,
        );
        Self {
            selected_file: combined.selected_file,
            selected_hunk: combined.selected_hunk,
            hunk_states: combined.hunk_states,
            stale_notice: combined.stale_notice,
            setup_notice: combined.setup_notice,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axis_core::review::ReviewFileChangeKind;

    fn payload_file(path: &str) -> ReviewFileDiff {
        ReviewFileDiff {
            path: path.to_string(),
            old_path: None,
            change_kind: ReviewFileChangeKind::Modified,
            added_lines: 1,
            removed_lines: 0,
            truncated: false,
            hunks: vec![],
        }
    }

    fn payload(paths: &[&str], truncated: bool) -> DeskReviewPayload {
        DeskReviewPayload {
            worktree_id: WorktreeId::new("/tmp/axis"),
            summary: ReviewSummary {
                files_changed: paths.len() as u32,
                uncommitted_files: 0,
                ready_for_review: true,
                last_inspected_at_ms: Some(1),
            },
            files: paths.iter().map(|path| payload_file(path)).collect(),
            truncated,
        }
    }

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

        let refreshed = refreshed_desk_review_summary_view(Some(&previous), None, None)
            .expect("last known review summary should be retained");

        assert_eq!(refreshed, previous);
    }

    #[test]
    fn local_desk_review_no_cache_synthetic_payload_is_not_ready_for_review() {
        let worktree_id = WorktreeId::new("/tmp/axis");
        let resolved = resolve_local_desk_review_payload_with(
            &worktree_id,
            &binding("feature/compact-only", false),
            None,
            || Err("rich payload unavailable".to_string()),
            || Ok((vec!["src/lib.rs".to_string()], 1)),
            42,
        )
        .expect("compact fallback should synthesize a payload");

        assert!(!resolved.stale);
        assert!(resolved.payload.files.is_empty());
        assert_eq!(resolved.payload.summary.files_changed, 1);
        assert_eq!(resolved.payload.summary.uncommitted_files, 1);
        assert!(
            !resolved.payload.summary.ready_for_review,
            "synthetic payload with no files must not report ready_for_review"
        );
        assert_eq!(resolved.summary.changed_files, vec!["src/lib.rs".to_string()]);
        assert!(
            resolved.summary.ready_for_review,
            "desk card should still reflect the successful compact refresh"
        );
    }

    #[test]
    fn local_desk_review_stale_cached_payload_preserves_rich_files_but_refreshes_summary() {
        let worktree_id = WorktreeId::new("/tmp/axis");
        let cached = payload(&["src/lib.rs"], true);
        let resolved = resolve_local_desk_review_payload_with(
            &worktree_id,
            &binding("feature/stale-cache", false),
            Some(&cached),
            || Err("rich payload unavailable".to_string()),
            || Ok((vec!["src/lib.rs".to_string(), "src/new.rs".to_string()], 1)),
            84,
        )
        .expect("cached payload should survive compact fallback");

        assert!(resolved.stale);
        assert_eq!(resolved.payload.files, cached.files);
        assert!(resolved.payload.truncated);
        assert_eq!(resolved.payload.summary.files_changed, 1);
        assert!(resolved.payload.summary.ready_for_review);
        assert_eq!(resolved.payload.summary.last_inspected_at_ms, Some(84));
        assert_eq!(
            resolved.summary.changed_files,
            vec!["src/lib.rs".to_string(), "src/new.rs".to_string()]
        );
        assert!(resolved.summary.ready_for_review);
    }

    #[test]
    fn truncated_payload_summary_view_uses_visible_file_list() {
        let payload = payload(&["src/a.rs", "src/b.rs"], true);
        let summary =
            build_desk_review_summary_view_from_payload(&binding("feature/truncated", false), &payload);

        assert_eq!(
            summary.changed_files,
            vec!["src/a.rs".to_string(), "src/b.rs".to_string()]
        );
        assert_eq!(summary.changed_files.len(), payload.files.len());
        assert!(payload.truncated);
        assert!(summary.ready_for_review);
    }

    #[test]
    fn review_state_stale_cached_payload_uses_latest_compact_summary_when_paths_disappear() {
        let worktree_id = WorktreeId::new("/tmp/axis");
        let cached = payload(&["src/lib.rs"], false);
        let resolved = resolve_local_desk_review_payload_with(
            &worktree_id,
            &binding("feature/stale-empty-compact", false),
            Some(&cached),
            || Err("rich payload unavailable".to_string()),
            || Ok((Vec::new(), 0)),
            99,
        )
        .expect("cached payload should survive compact fallback");

        assert!(resolved.stale);
        assert_eq!(resolved.payload.files, cached.files);
        assert_eq!(resolved.payload.summary.files_changed, resolved.payload.files.len() as u32);
        assert_eq!(resolved.payload.summary.files_changed, 1);
        assert_eq!(resolved.payload.summary.uncommitted_files, 0);
        assert!(resolved.summary.changed_files.is_empty());
        assert!(
            !resolved.summary.ready_for_review,
            "desk card should reflect the empty compact refresh, not the stale rich payload"
        );
    }

    #[test]
    fn review_state_rebind_rich_failure_uses_compact_only_synthetic_payload() {
        let worktree_id = WorktreeId::new("/tmp/axis-new");
        let rebound_binding = WorktreeBinding {
            root_path: "/tmp/axis-new".to_string(),
            ..binding("feature/rebound-fallback", false)
        };
        let cached = DeskReviewPayload {
            worktree_id: WorktreeId::new("/tmp/axis-old/./"),
            ..payload(&["src/old.rs"], false)
        };

        assert!(
            reusable_review_payload_cache(Some(&cached), &rebound_binding).is_none(),
            "rebound desks must not reuse cached rich payloads"
        );

        let resolved = resolve_local_desk_review_payload_with(
            &worktree_id,
            &rebound_binding,
            Some(&cached),
            || Err("rich payload unavailable".to_string()),
            || Ok((vec!["src/new.rs".to_string()], 1)),
            123,
        )
        .expect("rebound fallback should synthesize a compact-only payload");

        assert!(!resolved.stale);
        assert_eq!(resolved.payload.worktree_id, worktree_id);
        assert!(resolved.payload.files.is_empty());
        assert_eq!(resolved.payload.summary.files_changed, 1);
        assert_eq!(resolved.payload.summary.uncommitted_files, 1);
        assert!(!resolved.payload.summary.ready_for_review);
        assert_eq!(resolved.summary.changed_files, vec!["src/new.rs".to_string()]);
        assert!(resolved.summary.ready_for_review);
    }

    fn desk_id() -> WorkdeskId {
        WorkdeskId::new("desk-test")
    }

    fn sample_hunk(header: &str, old_start: u32, old_lines: u32, new_start: u32, new_lines: u32) -> ReviewHunk {
        ReviewHunk {
            header: header.to_string(),
            old_start,
            old_lines,
            new_start,
            new_lines,
            anchor_new_line: None,
            truncated: false,
            lines: vec![],
        }
    }

    fn sample_payload(path: &str, header: &str) -> DeskReviewPayload {
        DeskReviewPayload {
            worktree_id: WorktreeId::new("/tmp/axis"),
            summary: ReviewSummary {
                files_changed: 1,
                uncommitted_files: 0,
                ready_for_review: true,
                last_inspected_at_ms: Some(1),
            },
            files: vec![ReviewFileDiff {
                path: path.to_string(),
                old_path: None,
                change_kind: ReviewFileChangeKind::Modified,
                added_lines: 1,
                removed_lines: 1,
                truncated: false,
                hunks: vec![sample_hunk(header, 4, 2, 4, 2)],
            }],
            truncated: false,
        }
    }

    fn ctx(workdesk_id: WorkdeskId, rebound: bool, stale: bool) -> ReviewPanelRefreshContext {
        ReviewPanelRefreshContext {
            workdesk_id,
            worktree_rebound: rebound,
            stale_rich_payload: stale,
            setup_notice: None,
        }
    }

    #[test]
    fn review_state_preserves_selected_hunk_when_refresh_keeps_same_identity() {
        let wid = desk_id();
        let previous = ReviewPanelState::for_payload(wid.clone(), sample_payload("src/lib.rs", "@@ -4,2 +4,2 @@"));
        let refreshed = refresh_review_panel_state(
            Some(&previous),
            sample_payload("src/lib.rs", "@@ -4,2 +4,2 @@"),
            ctx(wid, false, false),
        );

        assert_eq!(refreshed.selected_file_path(), Some("src/lib.rs"));
        assert_eq!(refreshed.selected_hunk_header(), Some("@@ -4,2 +4,2 @@"));
    }

    #[test]
    fn review_state_falls_back_to_first_hunk_when_ranges_change() {
        let wid = desk_id();
        let previous = ReviewPanelState::for_payload(wid.clone(), sample_payload("src/lib.rs", "@@ -4,2 +4,2 @@"));
        let refreshed = refresh_review_panel_state(
            Some(&previous),
            sample_payload("src/lib.rs", "@@ -10,3 +10,4 @@"),
            ctx(wid, false, false),
        );

        assert_eq!(refreshed.selected_file_path(), Some("src/lib.rs"));
        assert_ne!(refreshed.selected_hunk_header(), Some("@@ -4,2 +4,2 @@"));
        assert_eq!(refreshed.selected_hunk, Some(0));
    }

    #[test]
    fn review_state_falls_back_to_first_hunk_when_previous_selection_no_longer_matches() {
        let wid = desk_id();
        let previous = ReviewPanelState::for_payload(
            wid.clone(),
            sample_payload("src/lib.rs", "@@ -4,2 +4,2 @@"),
        );
        let refreshed = refresh_review_panel_state(
            Some(&previous),
            sample_payload("src/lib.rs", "@@ -10,3 +10,4 @@"),
            ctx(wid, false, false),
        );

        assert_eq!(refreshed.selected_file_path(), Some("src/lib.rs"));
        assert_eq!(
            refreshed.selected_hunk,
            Some(0),
            "panel state should keep actions aligned with the visible fallback hunk"
        );
    }

    #[test]
    fn review_state_applies_and_clears_local_hunk_state() {
        let wid = desk_id();
        let mut panel = ReviewPanelState::for_payload(wid.clone(), sample_payload("src/lib.rs", "@@ -4,2 +4,2 @@"));
        let key = ReviewHunkKey::from_hunk(&wid, "src/lib.rs", &panel.payload.files[0].hunks[0]);
        panel.set_hunk_state(key.clone(), HunkReviewState::Reviewed);
        assert_eq!(
            file_review_aggregate(&wid, &panel.payload.files[0], &panel.hunk_states),
            FileReviewAggregate::AllReviewed
        );
        panel.clear_hunk_state(&key);
        assert_eq!(
            file_review_aggregate(&wid, &panel.payload.files[0], &panel.hunk_states),
            FileReviewAggregate::InProgress
        );
    }

    #[test]
    fn review_state_preserves_stale_notice_text_across_refresh() {
        let wid = desk_id();
        let previous = ReviewPanelState::for_payload(wid.clone(), sample_payload("src/lib.rs", "@@ -4,2 +4,2 @@"));
        let mut previous = previous;
        previous.stale_notice = Some("custom stale".to_string());
        let refreshed = refresh_review_panel_state(
            Some(&previous),
            sample_payload("src/lib.rs", "@@ -4,2 +4,2 @@"),
            ctx(wid.clone(), false, true),
        );
        assert_eq!(refreshed.stale_notice.as_deref(), Some("custom stale"));

        let cleared = refresh_review_panel_state(
            Some(&refreshed),
            sample_payload("src/lib.rs", "@@ -4,2 +4,2 @@"),
            ctx(wid, false, false),
        );
        assert!(cleared.stale_notice.is_none());
    }

    #[test]
    fn review_state_resets_hunk_markers_on_worktree_rebind() {
        let wid = desk_id();
        let mut previous = ReviewPanelState::for_payload(wid.clone(), sample_payload("src/lib.rs", "@@ -4,2 +4,2 @@"));
        let key = ReviewHunkKey::from_hunk(&wid, "src/lib.rs", &previous.payload.files[0].hunks[0]);
        previous.set_hunk_state(key, HunkReviewState::FollowUp);
        let refreshed = refresh_review_panel_state(
            Some(&previous),
            sample_payload("src/lib.rs", "@@ -4,2 +4,2 @@"),
            ctx(wid, true, false),
        );
        assert!(refreshed.hunk_states.is_empty());
    }

    #[test]
    fn review_state_disabled_actions_for_hunkless_entries() {
        let file = ReviewFileDiff {
            path: "bin.dat".to_string(),
            old_path: None,
            change_kind: ReviewFileChangeKind::Modified,
            added_lines: 0,
            removed_lines: 0,
            truncated: false,
            hunks: vec![],
        };
        assert!(review_hunk_actions_disabled(&file));
        assert!(!review_local_hunk_actions_enabled(&file));
    }

    #[test]
    fn review_jump_line_for_removal_without_anchor_uses_nearest_surviving_new_line() {
        let hunk = ReviewHunk {
            header: "@@ -10,3 +10,2 @@".to_string(),
            old_start: 10,
            old_lines: 3,
            new_start: 10,
            new_lines: 2,
            anchor_new_line: None,
            truncated: false,
            lines: vec![
                ReviewLine::context(Some(10), Some(10), true, "before"),
                ReviewLine::removed(Some(11), None, true, "removed"),
                ReviewLine::context(Some(12), Some(11), true, "after"),
            ],
        };

        assert_eq!(
            editor_jump_line_for_review_row(&hunk, &hunk.lines[1]),
            Some(11),
            "without an explicit anchor, removals should jump to the nearest surviving new-side line"
        );
    }

    #[test]
    fn review_editor_open_failed_notice_includes_path_and_error() {
        let msg = review_editor_open_failed_notice("/tmp/demo.rs", "no such pane");
        assert!(
            msg.contains("/tmp/demo.rs"),
            "notice should include path: {msg}"
        );
        assert!(
            msg.contains("no such pane"),
            "notice should include error: {msg}"
        );
    }

    #[test]
    fn review_jump_line_for_context_rows_uses_context_line_number() {
        let hunk = ReviewHunk {
            header: "@@ -4,2 +4,2 @@".to_string(),
            old_start: 4,
            old_lines: 2,
            new_start: 4,
            new_lines: 2,
            anchor_new_line: Some(4),
            truncated: false,
            lines: vec![ReviewLine::context(Some(4), Some(4), true, "context")],
        };

        assert_eq!(editor_jump_line_for_review_row(&hunk, &hunk.lines[0]), Some(4));
    }

    #[test]
    fn review_state_local_refresh_merges_using_previous_payload() {
        let wid = desk_id();
        let p1 = sample_payload("src/lib.rs", "@@ -4,2 +4,2 @@");
        let mut p2 = p1.clone();
        p2.summary.files_changed = 2;
        p2.files.push(ReviewFileDiff {
            path: "b.rs".to_string(),
            old_path: None,
            change_kind: ReviewFileChangeKind::Added,
            added_lines: 1,
            removed_lines: 0,
            truncated: false,
            hunks: vec![],
        });

        let p2_for_assert = p2.clone();
        let local = ReviewPanelLocalState::default();
        let local = local.refresh_from_payload(&wid, &p1, p2, ctx(wid.clone(), false, false));
        assert_eq!(local.selected_file_path(&p2_for_assert), Some("src/lib.rs"));
        assert_eq!(local.selected_hunk, Some(0));
    }

    #[test]
    fn review_state_merge_review_local_after_fetch_resets_progress_on_rebind() {
        let wid = desk_id();
        let previous_payload = sample_payload("src/lib.rs", "@@ -4,2 +4,2 @@");
        let mut local = ReviewPanelLocalState {
            selected_file: 0,
            selected_hunk: Some(0),
            hunk_states: HashMap::new(),
            stale_notice: Some("stale".to_string()),
            setup_notice: None,
        };
        let key =
            ReviewHunkKey::from_hunk(&wid, "src/lib.rs", &previous_payload.files[0].hunks[0]);
        local.hunk_states.insert(key, HunkReviewState::Reviewed);

        let next_payload = sample_payload("src/other.rs", "@@ -8,1 +8,1 @@");
        let (merged_payload, merged_local) = merge_review_local_after_fetch(
            &wid,
            Some(&previous_payload),
            &local,
            next_payload.clone(),
            ctx(wid.clone(), true, false),
        );

        assert_eq!(merged_payload, next_payload);
        assert!(merged_local.hunk_states.is_empty());
        assert_eq!(merged_local.selected_file_path(&merged_payload), Some("src/other.rs"));
        assert_eq!(merged_local.selected_hunk, Some(0));
        assert!(merged_local.stale_notice.is_none());
    }

    #[test]
    fn review_state_worktree_rebind_detection_normalizes_equivalent_paths() {
        let cached = DeskReviewPayload {
            worktree_id: WorktreeId::new("/tmp/axis-review/./"),
            ..sample_payload("src/lib.rs", "@@ -4,2 +4,2 @@")
        };
        let binding = WorktreeBinding {
            root_path: "/tmp/axis-review".to_string(),
            ..binding("feature/rebind-normalized", false)
        };

        assert!(!review_payload_worktree_rebound(Some(&cached), &binding));
    }
}
