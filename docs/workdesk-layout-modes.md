# Workdesk Layout Modes

## Summary

axis should keep the freedom of a spatial workdesk without forcing every
session into a chaotic freeform layout.

The right model is not three separate workspace types.
It is one workdesk with multiple layout lenses:

1. `free`
   Spatial arrangement and exploration.
2. `grid`
   Structured traversal across near-fullscreen panes.
3. `classic-split`
   Dense tiling for focused implementation sessions.

All three modes should operate on the same pane entities, terminal sessions,
and workdesk identity.
Only layout, camera behavior, and navigation rules should change.

## Product Principle

axis should be:

1. Free for arranging.
2. Grid for traversing.
3. Split for grinding.

This keeps the spatial thesis intact while giving the user stronger systems for
finding, operating, and revisiting context.

## Problem Statement

Pure canvas freedom is powerful early in a session, but it degrades once a desk
contains enough panes:

1. Windows drift into arbitrary positions and sizes.
2. Navigation cost grows with pane count.
3. Spatial memory becomes weak when panes are almost, but not quite, aligned.
4. Common workflows such as "go to the pane on the right" become unreliable.

The system needs stronger structure without collapsing back into "just another
split manager."

## Design Thesis

The workdesk should have a stable source of truth:

1. Pane identity.
2. Process identity.
3. Focus history.
4. Per-pane runtime state.

Layout modes should be derived projections over that same desk.
Switching modes must not create or destroy panes, restart processes, or force a
destructive translation of the user's workspace.

## Core Model

### Pane Identity

Each pane keeps:

1. Stable pane id.
2. Kind: `shell`, `agent`, future `editor`, `browser`, `logs`.
3. Runtime process/session state.
4. User metadata such as title and intent.

### Workdesk Identity

Each workdesk keeps:

1. A pane collection.
2. A current `layout_mode`.
3. Per-mode layout state.
4. Focus history.
5. Camera state for spatial modes.

### Per-Mode Layout State

Each workdesk should preserve independent state for each mode:

1. `free_layout`
   Explicit pane rectangles in world space.
2. `grid_layout`
   Pane ordering, adjacency graph, active cell, viewport index, expose state.
3. `split_layout`
   Tiling tree or split graph with sizing ratios.

This lets the user switch between modes without losing manual work in another
mode.

## Layout Modes

### `free`

`free` is the current spatial mode.
It should remain the least constrained mode.

#### Intent

1. Rough arrangement.
2. Exploration.
3. Temporary clustering.
4. Multi-context sessions where relative placement matters.

#### Rules

1. Panes may be any size.
2. Panes may overlap.
3. Panes may be placed anywhere on the desk.
4. Camera pans and zooms continuously.

#### Soft Constraints

`free` should eventually add optional order without becoming rigid:

1. Edge snapping.
2. Alignment guides.
3. Magnetic spacing.
4. Smart "drop near active pane" placement.

### `grid`

`grid` is the first structured mode we should implement.
It should feel like a directional workspace browser rather than a tiling
window manager.

#### Intent

1. Reduce chaos once a desk has many panes.
2. Make directional navigation reliable.
3. Keep panes visually large and easy to read.
4. Turn the workdesk into a sequence of strong contexts instead of a wall of
   tiny windows.

#### Mental Model

Each pane occupies a cell in a grid-like navigation graph.
The active pane is shown almost fullscreen.
Nearby panes are represented as directional hints around the viewport.

The user should be able to think:

1. "Go right to logs."
2. "Go down to the agent."
3. "Show me the whole desk."

#### Rules

1. Each pane is mapped to one cell.
2. Pane frames stay axis-aligned.
3. The active pane fills most of the viewport.
4. Navigation is discrete, not freeform.
5. Camera transitions are animated between cells.

#### Default Sizing

Grid panes should be slightly smaller than the viewport:

1. Leave a consistent margin around the active pane.
2. Preserve enough background to show the mode is still a workdesk.
3. Keep room for directional hints and mode chrome.

#### Navigation

Primary navigation should be:

1. `Cmd+Shift+Left`
2. `Cmd+Shift+Right`
3. `Cmd+Shift+Up`
4. `Cmd+Shift+Down`

Behavior:

1. Move to the nearest pane in that direction.
2. If no pane exists, do nothing.
3. Update focus.
4. Animate the viewport to the new active pane.

#### Directional Hints

The grid viewport should show lightweight hints at screen edges:

1. Left hint if there are panes to the left.
2. Right hint if there are panes to the right.
3. Top hint if there are panes above.
4. Bottom hint if there are panes below.

Each hint should include:

1. Title.
2. Pane kind.
3. Count if multiple panes exist in that direction.

This should feel closer to map navigation than to tabs.

#### Expose

`Expose` should be a transient command within `grid`, not a separate persistent
mode.

Behavior:

1. Fit all panes of the current desk into the viewport.
2. Show all pane cards at once.
3. Allow click or keyboard selection.
4. Return to the previous active pane zoom level after selection.

Expose is the escape hatch for "I know it is somewhere on this desk, show me
everything."

#### Entering Grid

When switching from `free` to `grid`, the system should:

1. Project existing panes into a stable grid order.
2. Preserve relative left-right and top-bottom relationships where possible.
3. Pick the currently focused pane as the initial active cell.

The first projection does not need to be perfect.
It does need to be predictable.

### `classic-split`

`classic-split` is the "workhorse mode."
It should feel familiar to users coming from `tmux`, `i3`, or editor splits.

#### Intent

1. Focused implementation.
2. Small sets of panes.
3. Long editing sessions.
4. Stable dense layouts.

#### Rules

1. Panes are arranged in a split tree or tiling graph.
2. No overlaps.
3. All available viewport space is allocated.
4. Resizing one pane redistributes space through the tree.

#### Use Cases

1. Shell left, editor center, logs bottom.
2. Agent right, tests bottom.
3. Tight inner loop without camera motion.

This mode is valuable, but it should follow `grid` in implementation priority.

## Why Modes Must Be Lenses, Not Separate Worlds

If modes become separate workspace types, the system becomes fragile:

1. Pane state must be copied or translated.
2. Runtime process identity gets complicated.
3. The user loses trust when mode switches rearrange everything destructively.
4. Features such as notifications, persistence, and desk sharing become harder.

Using layout lenses keeps the platform coherent:

1. One workdesk.
2. One set of panes.
3. Multiple views over the same desk.

## UX Flows

### Flow 1: Free to Grid

1. User spreads shells and agents around in `free`.
2. Desk becomes messy.
3. User switches to `grid`.
4. Active pane becomes the main viewport card.
5. Nearby panes appear as directional hints.
6. User traverses the desk with arrow shortcuts.

### Flow 2: Grid Expose

1. User is in `grid`.
2. They forget where a pane lives.
3. They trigger `Expose`.
4. All panes zoom out into a fit-to-screen overview.
5. They click the desired pane.
6. View animates back into focused `grid`.

### Flow 3: Grid to Split

1. User has identified a stable subset of panes they care about.
2. They switch to `classic-split`.
3. Those panes become a denser, operational workspace.
4. The original `free` arrangement still exists if they switch back later.

## Architecture

### Recommended Types

The app should grow toward four concepts:

1. `PaneGraph`
   Stable pane collection and metadata.
2. `LayoutEngine`
   Computes pane rectangles for the current mode.
3. `NavigationModel`
   Computes focus movement and directional neighbors.
4. `CameraController`
   Owns viewport position, zoom, and transitions.

### Suggested Rust Shapes

```rust
enum LayoutMode {
    Free,
    Grid,
    ClassicSplit,
}

struct WorkdeskLayoutState {
    active_mode: LayoutMode,
    free: FreeLayoutState,
    grid: GridLayoutState,
    split: SplitLayoutState,
}
```

```rust
struct FreeLayoutState {
    panes: HashMap<PaneId, Rect>,
    camera: CameraState,
}

struct GridLayoutState {
    order: Vec<PaneId>,
    active: Option<PaneId>,
    neighbors: HashMap<PaneId, GridNeighbors>,
    expose_open: bool,
}

struct SplitLayoutState {
    root: SplitNodeId,
    ratios: HashMap<SplitNodeId, f32>,
}
```

These are illustrative, not final APIs.

### Layout Engine Contract

Each layout engine should return:

1. Computed pane rectangles.
2. Optional camera target.
3. Optional directional hints.
4. Mode-specific overlays such as expose.

This keeps rendering separate from placement logic.

## Grid Projection Strategy

The first `free -> grid` projection can be deliberately simple.

Suggested algorithm:

1. Compute pane centers in world space.
2. Sort by `y`, then cluster into rows using a threshold.
3. Sort within each row by `x`.
4. Create a logical matrix from those rows.
5. Derive directional neighbors by nearest pane center in each direction.

This will not be perfect for all desks, but it is sufficient for v1 of `grid`.

Later we can improve with:

1. K-means or density-based clustering.
2. User-pinned rows and columns.
3. Manual reorder operations.

## Commands

### Mode Switching

1. `Toggle Free`
2. `Toggle Grid`
3. `Toggle Split`

We may later compress this into a mode switcher or command palette item.

### Grid Navigation

1. `FocusLeft`
2. `FocusRight`
3. `FocusUp`
4. `FocusDown`
5. `ExposeOpen`
6. `ExposeClose`
7. `ExposeSelect`

### Camera Commands

1. `RecenterActivePane`
2. `FitCurrentDesk`
3. `ReturnFromExpose`

## Visual Language

### Free

1. Full spatial background.
2. Minimal constraints.
3. Strong sense of desk openness.

### Grid

1. Active pane dominates.
2. Edge hints preview neighbors.
3. Camera motion reinforces direction.
4. Expose uses compact cards with enough metadata to pick quickly.

### Split

1. Reduced background.
2. Strong pane borders.
3. Compact chrome.
4. Clear resize affordances.

## Implementation Phases

### Phase 1: Grid Mode

1. Add `layout_mode` to `WorkdeskState`.
2. Add `GridLayoutState`.
3. Implement `free -> grid` projection.
4. Render the active pane nearly fullscreen.
5. Add directional hints.
6. Add `Cmd+Shift+Arrow` traversal.

### Phase 2: Expose

1. Add transient expose overlay for `grid`.
2. Fit all panes into the viewport.
3. Allow click selection.
4. Restore prior grid focus after selection.

### Phase 3: Split Mode

1. Add `SplitLayoutState`.
2. Introduce split tree model.
3. Add resize and rebalance behavior.
4. Support switching from grid to split and back.

## Acceptance Criteria

The layout mode system is successful when:

1. A desk can switch between `free` and `grid` without restarting panes.
2. The user can navigate a multi-pane desk directionally in `grid`.
3. `Expose` reliably surfaces every pane and returns to focused view.
4. Switching modes preserves per-mode layout state.
5. The system feels more ordered without losing the spatial identity of axis.

## Open Questions

1. Should `grid` allow manual swapping of pane positions?
2. Should mode be per-desk or global across all workdesks?
3. Should `split` derive from the currently visible subset or the full desk?
4. Do we need a fourth mode later for stacks or tab groups?
5. When a pane is spawned in `grid`, should it become the active pane or enter
   as a hinted neighbor?

## Recommendation

The next implementation target should be:

1. `layout_mode` in workdesk state.
2. `grid` as the first structured mode.
3. Directional neighbor hints.
4. `Expose` immediately after basic grid traversal works.

`classic-split` should follow once we validate that `grid` already solves the
"too much freedom" problem in real use.
