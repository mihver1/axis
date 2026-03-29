# Review Surface Design

## Summary

`axis` already has a usable desk-level review summary: branch state, changed file
count, and a coarse "ready for review" signal.

The next step should make that loop actionable without prematurely turning the
product into a full Git client or approval engine.

The recommended design is a right-side review panel that shows:

1. changed files for the active workdesk;
2. parsed unified diff hunks for the selected file;
3. lightweight hunk review state;
4. direct jump-to-editor navigation from diff lines.

This first version should stop short of:

1. staging or discarding changes;
2. GitHub or remote review state;
3. CI status;
4. provider-driven approvals;
5. persistent team review workflows.

## Problem Statement

The current review surface answers "did anything change in this desk?" but not
"what exactly changed, and have I worked through it yet?"

That leaves an awkward gap in the desk loop:

1. the rail can tell a user a desk is ready or dirty;
2. the user still has to leave the review loop to inspect actual diffs;
3. agent-driven review work has no first-class place to track which hunks were
   checked and which need follow-up.

This is especially visible in an agent IDE product thesis.
If a workdesk is supposed to hold the implementation, verification, and review
context together, then the first review UI cannot stop at file names forever.

## Product Outcome

After this feature, an `axis` user should be able to:

1. open a worktree-backed desk;
2. see that the desk has reviewable changes;
3. open a review panel from the desk itself;
4. browse changed files and hunks without leaving the app;
5. jump from a diff line into the real editor buffer at the closest relevant
   line;
6. mark a hunk as reviewed or needing follow-up while working through the desk.

If that loop feels coherent and fast, the feature is successful even without
any git-mutation or merge automation.

## Non-Goals

This design explicitly avoids:

1. `stage`, `unstage`, `discard`, or patch-apply controls;
2. automatic comment threads or PR review submission;
3. CI or remote check integration;
4. integrating hunk state with the agent runtime approval model;
5. building a generic diff editor abstraction;
6. persisting review state across app restarts in the first version;
7. solving every large-diff performance issue beyond a sensible truncation cap.

## Recommended Approach

Three implementation shapes are plausible:

### 1. App-Only Diff Parsing

`axis-app` could shell out to `git diff`, parse the output, and render the
result without touching shared types.

This is the fastest path, but it repeats logic that should eventually live in
the shared control-plane model.

### 2. Shared Review Model With Shared Diff Shaping

This is the recommended option.

`axis-core` owns stable review-oriented diff types.
The worktree/git layer shapes a common parsed payload.
`axisd` and `axis-app` fallback both expose the same data shape.
`axis-app` stays responsible for:

1. selection state;
2. transient hunk review markers;
3. panel rendering;
4. jump-to-editor actions.

This keeps the product loop consistent with the already-completed
control-plane-parity work and creates a cleaner bridge to later structured
approvals.

### 3. Synthetic Diff Buffers Reusing The Editor

The diff could be represented as editor-like buffers and rendered through more
of the existing editor path.

This sounds attractive, but it would over-couple the first review surface to
editor internals and make hunk-level state and action controls harder to reason
about.

## Core Model

The review model should represent a structured diff tree instead of a flat file
list.

### Canonical Review Scope

The rich review payload must use one explicit comparison rule so the desk
summary, changed-file list, and unified diff cannot drift apart.

The recommended rule is:

1. compare the current desk filesystem state against a review base;
2. use `merge-base(base_branch, HEAD)` as that base when `base_branch` exists;
3. otherwise use `HEAD` as the base;
4. add untracked files from `git status --porcelain` as file-level entries even
   when they have no textual diff yet.

This keeps the v1 review surface focused on "what would a human review in this
desk right now?" rather than splitting committed and uncommitted changes into
separate review modes.

The first version should not visually separate staged from unstaged changes.
It should present the current desk snapshot against the review base.

### Shared Review Types

`crates/axis-core` should add stable, serialization-friendly types for:

1. a desk review payload;
2. per-file diff metadata;
3. per-hunk metadata;
4. per-line diff rows;
5. truncation metadata where the payload is intentionally capped.

The model should be stable enough to flow through:

1. daemon responses;
2. app fallback paths;
3. future CLI inspection commands.

### File-Level Diff Model

Each changed file should carry:

1. repository-relative path;
2. change kind when available;
3. whether the file is binary or rename-only;
4. a list of parsed hunks;
5. summary counts for added and removed lines;
6. whether diff content was truncated.

Binary and rename-only entries should remain file-level records with readable
labels instead of fake line-level output.

### Hunk-Level Diff Model

Each hunk should carry enough information for navigation and local review state:

1. a stable header string;
2. old and new range metadata;
3. parsed diff lines in display order;
4. a best-effort anchor line for editor navigation.

The local review-state key for v1 should use:

1. stable `workdesk_id`;
2. file path;
3. hunk old/new range metadata;
4. hunk header.

That key should be desk-local on purpose.
If two desks point at the same worktree, they may still represent different
human review contexts and should not silently share transient progress.

If a desk is rebound to another worktree, or a refresh produces materially
different range metadata for a hunk, the local state should reset instead of
trying to guess continuity.

### Line-Level Diff Model

Each diff line should include:

1. line kind: context, addition, removal, or metadata;
2. display text;
3. old and/or new line numbers when relevant;
4. whether the row can jump into the editor.

That is enough for a readable unified diff without requiring editor-style text
infrastructure.

## Data Flow

The current desk review refresh loop already pulls summary data from the daemon
or local worktree helpers.
The new design should deepen that same path instead of creating a separate
review-only subsystem.

### Diff Fetch Path

When a desk has a `WorktreeBinding`, the review sync path should:

1. prefer daemon-provided review payloads;
2. fall back to local worktree inspection if the daemon is unavailable;
3. derive the desk review summary from the richer payload when full diff data is
   available;
4. cache the last known diff payload for stale/error states.

The fetch path should remain tolerant of daemon outages because the app already
has an established local fallback model for review summaries.

If only the compact summary can be refreshed but the rich payload fails, the app
should:

1. update the compact summary;
2. keep the previous rich diff payload;
3. mark the diff panel as stale with a non-blocking notice.

### Git Data Source

The worktree layer should shell out to `git diff --unified=3` for textual diff
content and reuse existing worktree binding inspection for branch and dirty
state.

The parsing layer should turn raw git output into the shared diff model before
it reaches the UI layer.

The exact git inputs should follow the canonical review scope:

1. text diff against the review base for tracked files;
2. status inspection for untracked, rename-only, binary, and conflicted paths;
3. rename detection enabled where practical so file identity is clearer in the
   panel.

If rename detection reports a renamed file with textual edits, it should still
produce normal hunks for the edited content rather than being forced into a
rename-only placeholder state.

If the configured base ref is missing locally, the app should fall back to
`HEAD` as the review base and surface a small setup notice.
If `HEAD` is unborn, the panel should fall back to file-level entries from
status inspection and explain that textual review is limited until the first
commit exists.

### UI State

`axis-app` should keep transient review-panel state separate from the parsed
payload itself.

That local state should track:

1. whether the review panel is open;
2. the selected file;
3. the selected hunk;
4. local per-hunk review markers;
5. optional stale/error banner text.

This keeps the shared diff model clean while allowing the UI to preserve
selection and review progress across refreshes when the diff tree is still
compatible.

## UX Design

### Entry Point

The compact desk review summary remains in the rail card.
When the desk has any reviewable changed-file entry, the desk should expose a
clear `Review` action from that surface.

The desk card should continue to answer:

1. branch or worktree identity;
2. dirty versus ready state;
3. changed file count.

The review panel answers the deeper "show me the actual diff" question, even if
some entries are binary, rename-only, untracked, or otherwise non-textual.

### Review Panel Layout

The first review surface should use a right-side panel, consistent with the
session inspector and workspace palette.

The panel should have three visual zones:

1. a file list with compact per-file status;
2. a diff view centered on the selected file and hunk;
3. a small action row for local hunk review state.

This gives the user a clear left-to-right review loop without introducing full
window routing or modal transitions.

### Selection Behavior

Opening the panel should:

1. select the first changed file by default;
2. select that file's first hunk when one exists;
3. keep the last valid selection across refreshes when possible.

Clicking a file selects it and moves focus to its first hunk.
Clicking a hunk selects it and brings it into view.

### Navigation Into The Editor

Clicking a diff line should jump to the closest meaningful editor location:

1. additions prefer the new-side line number;
2. removals use the nearest surviving anchor line;
3. context lines jump directly when a line number exists.

The review panel should open the real editor surface for that file if it is not
already active.

If the path cannot be opened as an editor surface, the panel should preserve
selection and show a small runtime notice instead of dropping the review state.

### Local Hunk Actions

The first actionable controls should be local review-state only:

1. `Mark reviewed`;
2. `Needs follow-up`;
3. `Clear`.

These states should tint the file list and selected hunk chrome so a user can
work through the desk in a visible order.

They should not mutate git state or claim provider/runtime semantics.

If the selected file has no textual hunk, the action row should stay visible but
disabled or empty rather than pretending file-level entries support hunk state.

## Error Handling And Edge Cases

### Stale Data

If a refresh fails, the app should keep the last known diff payload and surface
a small stale indicator instead of clearing the review panel.

This mirrors the current review summary behavior and avoids jarring flicker.

### Clean Or Undiverged Desks

If a desk has no diff content, the review panel should show a clear empty state:

1. clean working tree;
2. not yet diverged from base;
3. no reviewable hunks yet.

It should not render an empty shell that looks broken.

### Merge Conflicts, Deletions, And Untracked Files

The first version should set one-line policies for awkward but common cases:

1. conflicted files are reviewable file entries with a conflict label even if
   hunk rendering is partial;
2. deleted files keep file-level identity and any available textual hunks;
3. untracked files appear as reviewable file entries even when the payload can
   only show file-level metadata.

### Large Diffs

The first version should place explicit caps on:

1. number of files;
2. number of hunks per file;
3. number of rendered rows per hunk or payload.

If truncation happens, the payload and UI should say so explicitly.
Responsive behavior matters more than perfect completeness in v1.

Summary counts and file-list counts should remain internally consistent even
when diff rows are truncated.

### Binary And Rename-Only Changes

These should render as file entries with explanatory labels such as
"binary file changed" or "rename without textual diff".
The UI should not invent line rows where git did not provide meaningful text.

## Testing Strategy

The feature should not push all logic into `apps/axis-app/src/main.rs`.

### Parser Tests

Diff parsing should be covered with focused fixture-style tests for:

1. normal unified diff parsing;
2. multiple hunks in one file;
3. binary or rename-only cases;
4. truncation handling;
5. no-newline markers and other awkward unified-diff rows.

Fixture strategy should be explicit:

1. use pure parser fixtures or golden diff text for row-shaping logic;
2. use tiny temporary git repos where git-command behavior itself is the thing
   under test.

### Reducer And State Tests

Pure review selection and review-state helpers should cover:

1. preserving selected file and hunk across compatible refreshes;
2. dropping invalid selection when the diff changes materially;
3. applying and clearing local hunk review state;
4. deriving file-level status from hunk-level markers.

The test guidance should stay about unit-testable helpers, not assume a
specific reducer framework.

### Control-Plane Parity Tests

The richer review payload should extend the same parity mindset already used for
automation and daemon integration.

Tests should cover:

1. daemon response shape for the richer review payload;
2. parity between daemon-driven and app-fallback review shaping where practical;
3. summary projection consistency from the richer payload;
4. stale rich-payload behavior when compact summary refresh succeeds but full
   diff fetch does not.

### UI Tests

`gpui` tests should stay narrow and product-oriented:

1. opening the review panel from a desk;
2. selecting a hunk updates the visible target;
3. clicking a diff line opens or focuses the editor at the expected line;
4. marking a hunk updates visible state.

## Future Extensions

If this design works, it creates a clean path for the next wave:

1. provider- or runtime-driven approvals can reuse hunk identity instead of
   replacing the whole model;
2. CLI and daemon inspection can expose structured diff data, not just summary
   counts;
3. lightweight review automation can later attach to stable review payloads.

Those should remain follow-on steps.
The job of this feature is to make review inside a workdesk real and usable,
not to finish the entire approval system early.
