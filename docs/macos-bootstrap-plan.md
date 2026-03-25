# Canvas macOS Bootstrap Plan

## Purpose

This document describes how we should bootstrap the repository and local
development environment for the first `GPUI + libghostty-vt` prototype on
macOS.

The goal is not to optimize everything up front.
The goal is to get from a fresh laptop to a repeatable path where `just run`
opens a native window and we can march toward one live terminal pane.

## Assumptions

1. The first development target is macOS.
2. The laptop is fresh and currently only has Homebrew installed.
3. We want a reproducible toolchain, not a one-off local setup.
4. We are building against a pinned Ghostty revision rather than chasing `main`
   on every machine.

## Bootstrap Outcome

Bootstrap is successful when all of the following are true:

1. A new machine can install the required toolchain in a predictable order.
2. The repository has a clear workspace layout.
3. `cargo check` works from the repository root.
4. `just run` opens the app window.
5. The codebase is already structured for a future `TerminalPane`.

## Toolchain Strategy

### Rust

1. Use `rustup`.
2. Commit `rust-toolchain.toml` to pin the channel.
3. Prefer the stable toolchain unless `GPUI` forces a specific newer compiler.

### Zig

1. Treat Zig as a pinned dependency, not a floating global tool.
2. Install the exact Zig version required by the pinned Ghostty revision.
3. If Homebrew's default `zig` formula does not match the required version, use
   an official Zig binary or a version manager instead of silently drifting.

### Xcode

1. Install the full `Xcode.app`, not just Command Line Tools.
2. Point the active developer directory at `Xcode.app`.
3. Ensure the macOS SDK is available before touching the build.
4. Accept the Xcode license before the first `GPUI` build.

## Recommended Machine Setup Order

1. Install `Xcode.app`.
2. Run `xcode-select --switch /Applications/Xcode.app/Contents/Developer`.
3. Install base packages with Homebrew:
   `git`, `cmake`, `ninja`, `pkg-config`, `just`, and `rustup-init`.
4. Install Rust through `rustup`.
5. Install the pinned Zig version.
6. Verify that `clang`, `cargo`, and `zig` all resolve correctly in the shell.
7. Run `sudo xcodebuild -license accept` before the first `cargo check` that
   pulls in `GPUI`.

## Repository Layout

The initial repository layout should stay narrow:

```text
.
├── Cargo.toml
├── .cargo/
│   └── config.toml
├── rust-toolchain.toml
├── justfile
├── docs/
│   ├── v0-prototype-scope.md
│   └── macos-bootstrap-plan.md
├── apps/
│   └── canvas-app/
├── crates/
│   ├── canvas-core/
│   ├── canvas-terminal/
│   ├── process-manager/
│   └── ghostty-sys/
├── vendor/
│   └── ghostty/
└── scripts/
    └── bootstrap-macos.sh
```

## Crate Responsibilities

### `apps/canvas-app`

Owns the GPUI app entrypoint, window setup, global actions, and workdesk shell.

### `crates/canvas-core`

Owns workdesk state, pane geometry, focus state, commands, and simple domain
types that should not depend on FFI details.

### `crates/canvas-terminal`

Owns terminal pane behavior, terminal rendering integration, and the bridge
between PTY bytes, terminal state, and GPUI drawing.

### `crates/process-manager`

Owns PTY allocation, child process spawning, resize propagation, and lifecycle
management for shell and agent processes.

### `crates/ghostty-sys`

Owns the raw FFI boundary for `libghostty-vt` and nothing else.
This crate should stay small and intentionally boring.

## FFI Strategy

1. Vendor Ghostty into `vendor/ghostty` at a pinned commit.
2. Build against the vendored revision so every machine uses the same headers.
3. Keep the first FFI surface tiny and only expose the subset needed for:
   terminal creation, input encoding, writing bytes, resizing, and render state.
4. Prefer checked-in generated bindings or a very small handwritten extern layer
   over an ambitious build-time binding pipeline on day one.
5. Put all `unsafe` calls behind a narrow safe wrapper in `canvas-terminal`
   before they touch the rest of the app.

## Build and Developer UX

The root should eventually expose these commands:

1. `just doctor`
   Verifies `xcode-select`, `cargo`, `zig`, and expected SDK/tool versions.
2. `just check`
   Runs `cargo check --workspace`.
3. `just run`
   Launches the desktop app.
4. `just fmt`
   Runs formatting.

The repository may also use a project-local `DEVELOPER_DIR` fallback in
`.cargo/config.toml` so Cargo-based builds prefer the full `Xcode.app`
toolchain on macOS.

## Bootstrap Milestones

### Milestone 1: Skeleton Workspace

1. Create the Cargo workspace and crate layout.
2. Add `justfile`.
3. Add `rust-toolchain.toml`.
4. Add a minimal `apps/canvas-app` that opens a GPUI window.

### Milestone 2: Static Workdesk

1. Render a basic background.
2. Render one or two mock panes.
3. Support focus and panning.

### Milestone 3: PTY Path

1. Add `process-manager`.
2. Spawn a login shell in a PTY.
3. Confirm read and write loops work outside the UI first.

### Milestone 4: Ghostty Bridge

1. Add `ghostty-sys`.
2. Link the minimal `libghostty-vt` subset.
3. Feed PTY bytes into terminal state.
4. Produce renderable terminal output for a single pane.

### Milestone 5: Live Terminal Pane

1. Render terminal content inside GPUI.
2. Route keyboard input into the terminal pane.
3. Handle focus and resize correctly.

## Things We Should Not Do Yet

1. Do not split the repository into many micro-crates.
2. Do not add browser embedding to the bootstrap path.
3. Do not build a custom text editor before terminal panes are working.
4. Do not chase cross-platform packaging before macOS is stable.
5. Do not over-automate the setup before the manual bootstrap path is proven.

## First Bootstrap Script Responsibilities

The initial `scripts/bootstrap-macos.sh` should eventually do only four things:

1. Check for required tools.
2. Explain what is missing in plain language.
3. Prepare local project folders if needed.
4. Stop before making surprising system-wide changes.

This script should help developers, not replace understanding.

## Open Decisions

1. Whether we vendor Ghostty as a submodule or subtree.
2. Whether bindings are checked in or generated during bootstrap.
3. Whether we want `just` alone or `mise` plus `just` for tool pinning.
4. Whether `canvas-terminal` should own the safe wrapper or whether that should
   live in a dedicated higher-level crate later.

## References

1. [GPUI docs](https://docs.rs/gpui/latest/gpui/)
2. [GPUI site](https://www.gpui.rs/)
3. [Ghostty README](https://raw.githubusercontent.com/ghostty-org/ghostty/main/README.md)
4. [Ghostty About](https://ghostty.org/docs/about)
5. [ghostty.h](https://raw.githubusercontent.com/ghostty-org/ghostty/main/include/ghostty.h)
6. [Ghostling](https://github.com/ghostty-org/ghostling)
7. [libghostty render state docs](https://libghostty.tip.ghostty.org/group__render.html)
