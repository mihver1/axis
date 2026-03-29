# Review Surface Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the desk-level file summary with an actionable right-side review surface that shows structured diff hunks, supports local hunk review state, and jumps directly into the editor without breaking daemon/app parity.

**Architecture:** Add a shared structured review payload to `axis-core`, build that payload from git-backed worktree inspection in `axis-agent-runtime`, expose the same payload through `axisd` and the app’s local fallback path, and keep selection plus local hunk-review markers inside `axis-app`. Preserve the existing compact desk summary, but derive it from the richer payload whenever full diff data is available and keep stale handling non-blocking.

**Tech Stack:** Rust 2021, GPUI, `serde`, git CLI, existing `axis-agent-runtime::WorktreeService`, Unix socket automation via `axisd`, targeted module growth in `apps/axis-app/src/review.rs`.

---

## Scope Locks

These decisions are locked before implementation starts:

1. V1 actions are local-only: `Mark reviewed`, `Needs follow-up`, `Clear`.
2. No git mutation in this wave: no `stage`, `unstage`, `discard`, or patch apply.
3. No GitHub review, PR comment, or CI integration.
4. No persistence of local hunk review state across app restarts in v1.
5. The canonical review scope is the current desk snapshot against the review base:
   `merge-base(base_branch, HEAD)` when `base_branch` exists, otherwise `HEAD`.
6. Untracked, binary, conflicted, and rename-only files are still reviewable entries even when they have no textual hunks.
7. Renamed files with textual edits must still render normal hunks for the edited content; only pure rename-only entries stay hunkless.
8. `apps/axis-cli` does not need a new user-facing mode for this task, but its raw JSON output for `review.summary` will change to the richer payload shape.

## Execution Guardrails

1. Use `@superpowers:test-driven-development` before every production code change.
2. Use `@superpowers:verification-before-completion` before claiming a task is done.
3. Do not create commits during execution unless the human explicitly asks for them. Replace commit checkpoints with `git diff` / `git status` review checkpoints unless told otherwise.
4. Keep `apps/axis-app/src/main.rs` focused on shell wiring and rendering glue; push pure review logic into `apps/axis-app/src/review.rs` or a narrowly-scoped sibling module if the split pays for itself immediately.

## Planned File Structure

### Shared review payload

- Modify: `crates/axis-core/src/lib.rs`
  Re-export the new shared review module.
- Modify: `crates/axis-core/src/worktree.rs`
  Keep `WorktreeBinding` and `ReviewSummary`; add only the smallest compatibility changes needed to work with the richer payload.
- Create: `crates/axis-core/src/review.rs`
  Stable serializable review payload types: file entries, hunks, diff lines, truncation flags, and change kinds.
- Create: `crates/axis-core/tests/review_records.rs`
  Serialization and shape tests for the shared review payload.

### Git-backed review payload construction

- Modify: `crates/axis-agent-runtime/src/lib.rs`
  Re-export any new public review helpers only if needed outside the crate.
- Modify: `crates/axis-agent-runtime/src/worktree.rs`
  Add canonical review-base resolution, raw git diff/status collection, and review payload construction.
- Create: `crates/axis-agent-runtime/src/review_diff.rs`
  Keep unified-diff parsing and truncation logic out of `worktree.rs`.
- Create: `crates/axis-agent-runtime/tests/review_diff.rs`
  Fixture and tiny-repo tests for review payload construction and parsing.
- Modify: `crates/axis-agent-runtime/tests/worktree_service.rs`
  Extend existing worktree tests where the new canonical review scope overlaps current helper behavior.

### Automation parity

- Modify: `apps/axisd/src/request_handler.rs`
  Replace the flat review payload with the richer structured payload.
- Create: `apps/axisd/tests/support/mod.rs`
  Shared daemon test helpers extracted from `control_plane_parity.rs` so the new review tests do not duplicate fragile socket/bootstrap wiring.
- Create: `apps/axisd/tests/review_surface_parity.rs`
  Focused daemon parity tests for `review.summary` payload shape, not mixed into unrelated attention/state coverage.
- Modify: `apps/axis-app/src/daemon_client.rs`
  Deserialize the richer review payload.
- Modify: `apps/axis-app/src/main.rs`
  Keep the in-app automation fallback for `review.summary` shape-compatible with the daemon path.

### App review state and UI

- Modify: `apps/axis-app/src/review.rs`
  Add pure helpers for summary projection, selection state, hunk review markers, stale payload retention, and small view models.
- Modify: `apps/axis-app/src/main.rs`
  Cache the richer payload per desk, add review panel open/close/select behavior, render the right-side panel, and wire diff-line navigation into existing editor-opening helpers.

## Task 1: Shared Review Payload Records

**Files:**
- Modify: `crates/axis-core/src/lib.rs`
- Modify: `crates/axis-core/src/worktree.rs`
- Create: `crates/axis-core/src/review.rs`
- Create: `crates/axis-core/tests/review_records.rs`

- [ ] **Step 1: Write the failing shared-payload test**

Create `crates/axis-core/tests/review_records.rs` with focused coverage for the shape you want to expose over automation:

```rust
use axis_core::review::{
    DeskReviewPayload, ReviewFileChangeKind, ReviewFileDiff, ReviewHunk, ReviewLine,
    ReviewLineKind,
};
use axis_core::worktree::{ReviewSummary, WorktreeId};

#[test]
fn desk_review_payload_round_trips_structured_hunks() {
    let payload = DeskReviewPayload {
        worktree_id: WorktreeId::new("wt-demo"),
        summary: ReviewSummary {
            files_changed: 2,
            uncommitted_files: 1,
            ready_for_review: true,
            last_inspected_at_ms: Some(123),
        },
        files: vec![ReviewFileDiff {
            path: "src/lib.rs".to_string(),
            old_path: None,
            change_kind: ReviewFileChangeKind::Modified,
            added_lines: 1,
            removed_lines: 1,
            truncated: false,
            hunks: vec![ReviewHunk {
                header: "@@ -4,2 +4,2 @@".to_string(),
                old_start: 4,
                old_lines: 2,
                new_start: 4,
                new_lines: 2,
                truncated: false,
                lines: vec![
                    ReviewLine::context(Some(4), Some(4), "fn demo() {"),
                    ReviewLine::removed(Some(5), None, "old_call();"),
                    ReviewLine::added(None, Some(5), "new_call();"),
                ],
            }],
        }],
        truncated: false,
    };

    let json = serde_json::to_value(&payload).unwrap();
    assert_eq!(json["files"][0]["change_kind"], "modified");
    assert_eq!(json["files"][0]["hunks"][0]["lines"][2]["kind"], "addition");

    let back: DeskReviewPayload = serde_json::from_value(json).unwrap();
    assert_eq!(back, payload);
}
```

- [ ] **Step 2: Run the new core test and watch it fail**

Run: `cargo test -p axis-core --test review_records -v`

Expected: FAIL because `axis_core::review` and the new review payload types do not exist yet.

- [ ] **Step 3: Create `crates/axis-core/src/review.rs`**

Define the smallest stable shared model:

```rust
pub struct DeskReviewPayload {
    pub worktree_id: WorktreeId,
    pub summary: ReviewSummary,
    pub files: Vec<ReviewFileDiff>,
    pub truncated: bool,
}

pub struct ReviewFileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub change_kind: ReviewFileChangeKind,
    pub added_lines: u32,
    pub removed_lines: u32,
    pub truncated: bool,
    pub hunks: Vec<ReviewHunk>,
}

pub struct ReviewHunk {
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub anchor_new_line: Option<u32>,
    pub truncated: bool,
    pub lines: Vec<ReviewLine>,
}

pub struct ReviewLine {
    pub kind: ReviewLineKind,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub jumpable: bool,
    pub text: String,
}
```

Use `snake_case` serde naming and keep constructors/helpers tiny and test-friendly.

- [ ] **Step 4: Re-export the review module cleanly**

Update `crates/axis-core/src/lib.rs` to expose `pub mod review;`.

Keep `ReviewSummary` in `crates/axis-core/src/worktree.rs`; only touch it if the richer payload needs shared helper docs or small compatibility adjustments.

- [ ] **Step 5: Re-run the core review-payload tests**

Run: `cargo test -p axis-core --test review_records -v`

Expected: PASS with the new structured payload round-tripping correctly.

- [ ] **Step 6: Run the existing shared-type regression check**

Run: `cargo test -p axis-core --test agent_records -v`

Expected: PASS with the pre-existing worktree and automation schema tests still green.

- [ ] **Step 7: Checkpoint the diff without committing**

Run:

```bash
git diff -- crates/axis-core/src/lib.rs crates/axis-core/src/worktree.rs \
  crates/axis-core/src/review.rs crates/axis-core/tests/review_records.rs
```

Expected: only the new shared review types and tests appear.

## Task 2: Git Review Payload Construction And Parsing

**Files:**
- Modify: `crates/axis-agent-runtime/src/lib.rs`
- Modify: `crates/axis-agent-runtime/src/worktree.rs`
- Create: `crates/axis-agent-runtime/src/review_diff.rs`
- Create: `crates/axis-agent-runtime/tests/review_diff.rs`
- Modify: `crates/axis-agent-runtime/tests/worktree_service.rs`

- [ ] **Step 1: Write the failing git-backed review tests**

Create `crates/axis-agent-runtime/tests/review_diff.rs` with one tiny-repo test per behavior:

```rust
#[test]
fn review_payload_merges_base_and_untracked_changes() {
    let payload = WorktreeService::review_payload(&wt_dir, Some("main"), ReviewPayloadLimits::default())
        .unwrap();

    assert!(payload.files.iter().any(|file| file.path == "src/lib.rs" && !file.hunks.is_empty()));
    assert!(payload.files.iter().any(|file| file.path == "scratch.txt"));
}

#[test]
fn review_payload_marks_binary_and_rename_only_entries_without_fake_hunks() {
    let payload = WorktreeService::review_payload(&wt_dir, Some("main"), ReviewPayloadLimits::default())
        .unwrap();

    let renamed = payload.files.iter().find(|file| file.path == "src/new_name.rs").unwrap();
    assert!(renamed.hunks.is_empty());
}
```

Add at least one parser fixture for `\ No newline at end of file`.
Reuse the tiny-repo harness patterns already present in `crates/axis-agent-runtime/tests/worktree_service.rs`
for `TempDir`, repo initialization, and `run_git(...)` helpers instead of inventing
another fixture style.

- [ ] **Step 2: Run the new runtime test and verify the red state**

Run: `cargo test -p axis-agent-runtime --test review_diff -v`

Expected: FAIL because `WorktreeService::review_payload` and the diff parser module do not exist yet.

- [ ] **Step 3: Create `crates/axis-agent-runtime/src/review_diff.rs`**

Keep raw unified-diff shaping out of `worktree.rs`.
The parser should own:

```rust
struct ParsedDiffFile { /* private */ }

fn parse_unified_diff(output: &str, limits: &ReviewPayloadLimits) -> Vec<ReviewFileDiff> { /* ... */ }
```

Handle:

1. normal hunks,
2. multiple hunks per file,
3. metadata lines,
4. deleted-file hunks,
5. rename-only and binary placeholders,
6. conflicted file entries,
7. rename-with-edits cases that still produce hunks,
8. truncation flags.

- [ ] **Step 4: Extend `WorktreeService` with canonical review payload construction**

Add a focused public entry point in `crates/axis-agent-runtime/src/worktree.rs`:

```rust
pub fn review_payload(
    root: impl AsRef<Path>,
    base_branch: Option<&str>,
    limits: ReviewPayloadLimits,
) -> anyhow::Result<DeskReviewPayload>
```

Implementation rules:

1. resolve the review base with `merge-base(base_branch, HEAD)` when possible;
2. fall back to `HEAD` if the base ref is missing locally;
3. if `HEAD` is unborn, build file-level entries from `git status --porcelain`;
4. use `git diff --unified=3` for tracked textual changes;
5. include untracked files as file-level entries even if no textual hunk exists;
6. mark `ReviewLine::jumpable` explicitly instead of making the UI infer it ad hoc;
7. compute `ReviewHunk::anchor_new_line` so removed rows can jump to the nearest surviving editor line;
8. keep the payload summary internally consistent with truncation.

- [ ] **Step 5: Re-export only the runtime surface you actually need**

If external callers only need `WorktreeService`, keep the parser private and avoid widening the crate API.
Only add a public `ReviewPayloadLimits` if multiple crates truly need to configure caps.

- [ ] **Step 6: Re-run the new runtime tests**

Run: `cargo test -p axis-agent-runtime --test review_diff -v`

Expected: PASS with merge-base, fallback, binary, rename, and awkward diff-row coverage green.

- [ ] **Step 7: Re-run the existing worktree regression tests**

Run: `cargo test -p axis-agent-runtime --test worktree_service -v`

Expected: PASS with create/attach/dirty/ahead behavior unchanged.

- [ ] **Step 8: Checkpoint the runtime diff**

Run:

```bash
git diff -- crates/axis-agent-runtime/src/lib.rs \
  crates/axis-agent-runtime/src/worktree.rs \
  crates/axis-agent-runtime/src/review_diff.rs \
  crates/axis-agent-runtime/tests/review_diff.rs \
  crates/axis-agent-runtime/tests/worktree_service.rs
```

Expected: only review-payload construction and parser work appear.

## Task 3: Daemon And App Fallback Review Payload Parity

**Files:**
- Modify: `apps/axisd/src/request_handler.rs`
- Create: `apps/axisd/tests/support/mod.rs`
- Create: `apps/axisd/tests/review_surface_parity.rs`
- Modify: `apps/axis-app/src/daemon_client.rs`
- Modify: `apps/axis-app/src/main.rs`

- [ ] **Step 1: Write the failing daemon parity test**

Before adding assertions, extract the reusable socket/bootstrap helpers from
`apps/axisd/tests/control_plane_parity.rs` into `apps/axisd/tests/support/mod.rs`
and consume them from both test files.

Then create `apps/axisd/tests/review_surface_parity.rs` with a focused payload assertion:

```rust
#[test]
fn review_summary_returns_structured_files_and_hunks() {
    let review = send_request(
        &socket_path,
        &AutomationRequest::DeskReviewSummary {
            worktree_id: worktree_id.clone(),
        },
    )
    .expect("review.summary should succeed");

    assert!(review.ok);
    let result = review.result.expect("payload should exist");
    assert!(result["files"].as_array().unwrap().iter().any(|file| {
        file["path"] == "src/lib.rs" && file["hunks"].as_array().map_or(false, |hunks| !hunks.is_empty())
    }));
}
```

Add one stale/partial case if the daemon can only refresh summary metadata and not the full rich diff.
Also add at least one case each for:

1. conflicted files,
2. rename-only entries,
3. rename-with-edits entries that still return hunks,
4. deleted files with textual hunks.

- [ ] **Step 2: Run the daemon parity test and confirm the shape mismatch**

Run: `cargo test -p axisd review_surface_parity -v`

Expected: FAIL because `review.summary` still returns only `changed_files` and `uncommitted_files`.

- [ ] **Step 3: Upgrade the daemon response to the shared structured payload**

Update `apps/axisd/src/request_handler.rs` so `AutomationRequest::DeskReviewSummary` builds a `DeskReviewPayload` via `WorktreeService::review_payload(...)`.

Do not keep a second bespoke JSON shape in the daemon.

- [ ] **Step 4: Update the app client and local automation fallback**

Modify `apps/axis-app/src/daemon_client.rs` so `DeskReviewResult` matches the richer payload.

Then update `SharedAutomationRequest::DeskReviewSummary` handling in `apps/axis-app/src/main.rs` so the local fallback path produces the same JSON shape when the daemon is unavailable.

- [ ] **Step 5: Keep the fallback summary projection behavior explicit**

When the app fallback succeeds with only compact summary metadata but the rich payload is stale, follow the spec exactly:

1. update the summary,
2. keep the previous rich payload,
3. surface a small stale notice.

Do not let the fallback silently clear the panel.

At the same time, reconcile the current summary semantics into one shared rule:

```rust
ready_for_review = !binding.dirty && payload_has_reviewable_entries
```

Add one focused test so truncation and the richer payload still project a
summary consistent with the visible file list.

- [ ] **Step 6: Add one daemon-vs-fallback parity assertion**

Use the same tiny fixture worktree to compare:

1. daemon `review.summary`,
2. app local fallback `review.summary`.

Normalize them into `DeskReviewPayload` and assert the same `summary`, `files`,
and `truncated` values where practical.

- [ ] **Step 7: Re-run the daemon parity test**

Run: `cargo test -p axisd review_surface_parity -v`

Expected: PASS with the richer review payload decoded and asserted directly.

- [ ] **Step 8: Re-run the existing daemon parity suite**

Run: `cargo test -p axisd control_plane_parity -v`

Expected: PASS with `state.current` and `attention.next` behavior still intact.

- [ ] **Step 9: Checkpoint daemon/app parity changes**

Run:

```bash
git diff -- apps/axisd/src/request_handler.rs \
  apps/axisd/tests/support/mod.rs \
  apps/axisd/tests/review_surface_parity.rs \
  apps/axis-app/src/daemon_client.rs \
  apps/axis-app/src/main.rs
```

Expected: the same review payload shape is visible on both daemon and app fallback paths.

## Task 4: App Review State, Summary Projection, And Selection Helpers

**Files:**
- Modify: `apps/axis-app/src/review.rs`
- Modify: `apps/axis-app/src/main.rs`

- [ ] **Step 1: Write the failing pure review-state tests**

Expand `apps/axis-app/src/review.rs` tests around the new state helpers:

```rust
#[test]
fn review_state_preserves_selected_hunk_when_refresh_keeps_same_identity() {
    let previous = ReviewPanelState::for_payload(sample_payload("src/lib.rs", "@@ -4,2 +4,2 @@"));
    let refreshed = refresh_review_panel_state(&previous, sample_payload("src/lib.rs", "@@ -4,2 +4,2 @@"));

    assert_eq!(refreshed.selected_file_path(), Some("src/lib.rs"));
    assert_eq!(refreshed.selected_hunk_header(), Some("@@ -4,2 +4,2 @@"));
}

#[test]
fn review_state_drops_stale_hunk_selection_when_ranges_change() {
    let previous = ReviewPanelState::for_payload(sample_payload("src/lib.rs", "@@ -4,2 +4,2 @@"));
    let refreshed = refresh_review_panel_state(&previous, sample_payload("src/lib.rs", "@@ -10,3 +10,4 @@"));

    assert_ne!(refreshed.selected_hunk_header(), Some("@@ -4,2 +4,2 @@"));
}
```

Also add one stale-rich-payload test.

- [ ] **Step 2: Run the targeted app review-state tests**

Run: `cargo test -p axis-app review_state_ -v`

Expected: FAIL because the new review panel state and refresh helpers do not exist yet.

- [ ] **Step 3: Expand `apps/axis-app/src/review.rs` with pure state helpers**

Keep non-rendering logic here:

```rust
pub(crate) enum HunkReviewState {
    Todo,
    Reviewed,
    FollowUp,
}

pub(crate) struct ReviewPanelState {
    pub payload: DeskReviewPayload,
    pub selected_file: usize,
    pub selected_hunk: Option<usize>,
    pub hunk_states: HashMap<ReviewHunkKey, HunkReviewState>,
    pub stale_notice: Option<String>,
}
```

Also add:

1. summary projection helpers from the richer payload,
2. file-level aggregate status,
3. `ReviewHunkKey` scoped to `workdesk_id + file path + old/new range + header`,
4. selection-preserving refresh helpers,
5. reset behavior when a workdesk is rebound or hunk identity changes materially,
6. disabled-action helpers for non-text entries.

- [ ] **Step 4: Cache the richer review payload on each workdesk**

Update `WorkdeskState` in `apps/axis-app/src/main.rs` to keep both:

1. the compact `review_summary`,
2. the last known structured review payload.

Keep panel-open state in `AxisShell`, consistent with session inspector and workspace palette.

- [ ] **Step 5: Wire refresh logic through the new helpers**

When a desk review refresh succeeds:

1. project `review_summary` from the rich payload,
2. refresh the cached payload,
3. preserve selection and local hunk markers when identities still match.

When rich diff refresh fails but compact summary succeeds:

1. keep the cached payload,
2. mark the panel stale,
3. do not drop local review progress.

If the review base ref is missing or `HEAD` is unborn, convert that state into a
small user-facing setup/runtime notice through the same review-state helpers.

- [ ] **Step 6: Re-run the targeted app review-state tests**

Run: `cargo test -p axis-app review_state_ -v`

Expected: PASS with selection retention, stale behavior, and file-level aggregate state covered.

- [ ] **Step 7: Checkpoint the app-state diff**

Run:

```bash
git diff -- apps/axis-app/src/review.rs apps/axis-app/src/main.rs
```

Expected: review state moves into pure helpers instead of sprawling across ad-hoc `main.rs` branches.

## Task 5: Review Panel UI And Jump-To-Editor Flow

**Files:**
- Modify: `apps/axis-app/src/review.rs`
- Modify: `apps/axis-app/src/main.rs`

- [ ] **Step 1: Write the failing GPUI review-panel tests**

Add narrow UI tests in `apps/axis-app/src/main.rs` for the user-visible loop:

```rust
#[gpui::test]
async fn review_panel_opens_from_reviewable_desk(cx: &mut TestAppContext) {
    // Follow the same window/update shape used by
    // attention_transition_enqueues_unread_notification and
    // opening_selected_workspace_search_result_focuses_requested_line:
    // arrange one desk with a cached DeskReviewPayload,
    // trigger the review entry point,
    // assert the panel is open and the first file is selected.
}

#[gpui::test]
async fn clicking_review_line_opens_editor_at_expected_line(cx: &mut TestAppContext) {
    // Reuse the editor-open and line-assertion pattern from
    // opening_selected_workspace_search_result_focuses_requested_line:
    // arrange one selected hunk with a jumpable diff line,
    // trigger the line action,
    // assert the editor opens and the cursor lands on the expected line.
}
```

Add one action-row test for `Mark reviewed`, and one non-text selection test
that proves the action row is disabled or empty for hunkless entries.

- [ ] **Step 2: Run the review-panel UI tests and verify the red state**

Run: `cargo test -p axis-app review_panel_ -v`

Expected: FAIL because the desk card has no `Review` CTA, the panel does not render, and diff-line actions are not wired.

- [ ] **Step 3: Add the desk-card entry point**

Update the desk rail card in `apps/axis-app/src/main.rs` so a reviewable desk exposes a clear `Review` action whenever the structured payload has at least one file entry.

Keep the compact summary intact; do not replace it with the full panel inline.

- [ ] **Step 4: Render the right-side review panel**

Follow the existing right-overlay pattern used by the session inspector and workspace palette:

1. file list on the left edge of the panel body,
2. selected hunk diff in the main pane,
3. action row with local-only buttons,
4. empty or stale states that read as intentional.

Prefer small render helpers in `apps/axis-app/src/review.rs` when the same row/status formatting is reused.

- [ ] **Step 5: Reuse existing editor-opening helpers for diff navigation**

Use the same surface-opening and line-jump path already used by workspace search:

1. open the file in an editor surface if needed,
2. additions jump by the new-side line number,
3. removals jump by `anchor_new_line` or the nearest surviving new-side line,
4. context lines jump directly when a line number exists,
5. preserve panel state after navigation.

Add one focused removal-line assertion:

1. arrange a selected hunk with a removed row,
2. trigger the line action,
3. assert the editor lands on the hunk anchor or nearest surviving line,
4. do not leave removal navigation implicit.

Do not build a second editor-launch path just for review diffs.

- [ ] **Step 6: Disable hunk actions for non-text entries**

If the selected file has no textual hunks:

1. keep the selection,
2. show the explanatory file-level state,
3. render the action row as disabled or empty,
4. do not fake hunk review markers.

- [ ] **Step 7: Re-run the review-panel UI tests**

Run: `cargo test -p axis-app review_panel_ -v`

Expected: PASS with the review panel opening, selection working, jump-to-editor wired, and hunk state actions behaving locally.

- [ ] **Step 8: Run the full app regression pass**

Run: `cargo test -p axis-app -q`

Expected: PASS with the new review-surface tests green and existing app behavior unaffected.

- [ ] **Step 9: Final verification sweep**

Run:

```bash
cargo test -p axis-core --test review_records -v
cargo test -p axis-agent-runtime --test review_diff -v
cargo test -p axisd review_surface_parity -v
cargo test -p axis-app review_state_ -v
cargo test -p axis-app review_panel_ -v
```

Expected: PASS across shared records, git diff shaping, daemon parity, review state, and UI navigation.

- [ ] **Step 10: Final diff/status checkpoint**

Run:

```bash
git status --short
git diff -- crates/axis-core/src/lib.rs crates/axis-core/src/worktree.rs \
  crates/axis-core/src/review.rs crates/axis-core/tests/review_records.rs \
  crates/axis-agent-runtime/src/lib.rs crates/axis-agent-runtime/src/worktree.rs \
  crates/axis-agent-runtime/src/review_diff.rs \
  crates/axis-agent-runtime/tests/review_diff.rs \
  crates/axis-agent-runtime/tests/worktree_service.rs \
  apps/axisd/src/request_handler.rs apps/axisd/tests/review_surface_parity.rs \
  apps/axis-app/src/daemon_client.rs apps/axis-app/src/review.rs \
  apps/axis-app/src/main.rs
```

Expected: the review surface lands as one coherent stack: shared model, git-backed payload, daemon/app parity, local review state, and UI overlay.
