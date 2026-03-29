# axis v0 Prototype Scope

## Summary

axis v0 is a native desktop prototype for spatial, agent-oriented development under the `artel` umbrella.
It is not an IDE and not a terminal multiplexer replacement yet.
The goal of v0 is to prove that a `GPUI + libghostty-vt` stack can support a
scrollable workdesk with terminal-based tools and agent sessions as first-class
surfaces.

## Product Thesis

Traditional split panes punish parallel work by shrinking every view at once.
axis should let us place shells, editors, logs, and agents on a larger
workdesk, then navigate that space instead of continuously rebalancing splits.

The prototype should answer one question:

Can a native spatial workspace feel better than splits for running multiple
terminal-native development tasks in parallel?

## Primary Goals

1. Prove the core interaction model of a scrollable spatial workdesk.
2. Prove a custom terminal pane built with `libghostty-vt` rendered inside
   `GPUI`.
3. Prove that agent sessions can be treated as normal panes, not special-case
   modals or sidebars.
4. Keep the implementation narrow enough to bootstrap quickly on a fresh
   machine.

## Non-Goals

The following are explicitly out of scope for v0:

1. Embedded browser panes.
2. Rich text editor widgets or a custom code editor.
3. Persistent session restore.
4. Remote workspaces or collaboration.
5. Plugin systems, themes, or deep configuration.
6. Complex pane graphs, tabs, or nested workspaces.
7. Cross-platform support beyond getting macOS working first.

## User Experience Scope

The prototype must support the following happy path:

1. Launch the app into a single workdesk.
2. Pan around the workdesk.
3. Spawn a terminal pane at a position on the canvas.
4. Spawn a second pane for another shell or agent session.
5. Focus panes with mouse and keyboard.
6. Resize and move panes.
7. Interact with terminal applications inside panes, including terminal editors
   such as `nvim` or `hx`.
8. Visually distinguish the focused pane from background panes.

## Technical Scope

### Core Stack

1. `Rust` as the primary language.
2. `GPUI` for app shell, layout, input, and custom rendering.
3. `libghostty-vt` through a thin Rust FFI layer for terminal state, input
   encoding, and render state.
4. Native PTY management in Rust for shell and agent processes.

### Core Components

1. `app`
   Owns window setup, global actions, and workdesk state.
2. `workdesk`
   Owns camera offset, pan behavior, and pane collection.
3. `pane`
   Generic spatial container with frame, title, focus state, and resize logic.
4. `terminal_pane`
   Couples PTY lifecycle, terminal model, rendering, and input bridge.
5. `process_manager`
   Spawns shell and agent commands and wires them to PTYs.
6. `ghostty_sys`
   Unsafe FFI crate exposing only the subset of `libghostty-vt` needed by v0.

## Architectural Decisions

1. `GPUI` owns the entire window and render tree.
2. We use `libghostty-vt`, not the full `libghostty` app or surface embedding
   API.
3. Terminal panes render from terminal state inside our own UI layer rather
   than embedding a foreign native surface.
4. The first editor experience is terminal-native. Running `nvim` or `hx` inside
   a terminal pane is enough for v0.
5. Browser support is deferred. If needed for development flows, v0 can open an
   external browser.

## Minimal Feature Set

### Must Have

1. One native window.
2. Scrollable and pannable 2D workdesk.
3. Multiple movable and resizable panes.
4. Terminal pane creation from a command palette, hotkey, or simple button.
5. PTY-backed shell sessions.
6. Keyboard input, mouse selection, and resize propagation for terminal panes.
7. Basic pane chrome with title and focus ring.

### Nice to Have

1. Zoom in and out of the workdesk.
2. Simple pane snapping or alignment guides.
3. A dedicated "agent pane" preset that runs a configured command.
4. A lightweight minimap or recenter action.

## Out-of-Scope UX Questions

These questions are important but do not block v0:

1. Tabs versus freeform clustering.
2. Notifications and attention routing for agents.
3. Saved desks and workspace restore.
4. Embedded browser panes.
5. Multi-window coordination.

## Acceptance Criteria

v0 is successful if all of the following are true:

1. The app opens reliably on macOS.
2. A user can create at least two terminal panes on the workdesk.
3. Each terminal pane runs an interactive PTY program correctly.
4. Panes can be moved without breaking terminal interaction.
5. The workdesk can be panned without visual corruption or broken focus.
6. Running `nvim` in one pane and a shell command in another feels usable.

## First Milestones

### Milestone 1: Boot Window

1. GPUI app opens a window.
2. Workdesk renders a background and supports pan.
3. Static mock panes can be drawn and focused.

### Milestone 2: One Live Terminal

1. Rust PTY process launches a shell.
2. Terminal bytes flow into `libghostty-vt`.
3. Terminal output renders in one pane.
4. Keyboard input reaches the shell.

### Milestone 3: Spatial Multipane

1. Multiple terminal panes can be created.
2. Pane move and resize work.
3. Focus switching is reliable.

### Milestone 4: Agent Loop

1. A pane can launch a predefined agent command.
2. Agent output remains interactive and visible in parallel with other panes.
3. The workdesk model still feels coherent with at least three active panes.

## Open Questions

1. Whether v0 should start with pan only or pan plus zoom.
2. How much text selection we want to support in the first terminal pane cut.
3. Whether pane creation should begin as a keyboard-first command or a visible
   toolbar action.
4. Which agent command we use as the default smoke-test target.

## Next Document

The next useful artifact is the macOS bootstrap and repository plan:
[macos-bootstrap-plan.md](macos-bootstrap-plan.md)

The next product design artifact after bootstrap is the layout mode system:
[workdesk-layout-modes.md](workdesk-layout-modes.md)
