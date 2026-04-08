# Axis → Conductor Replacement Design

**Date:** 2026-04-07
**Goal:** Make axis a daily-driver replacement for Conductor + Zed/Cursor.
**Strategy:** Parallel tracks — agent stability and editor evolution — converging at Cursor ACP provider.

## Current State

- **Spatial workdesk**: Stable, performant. Pan/zoom canvas is a killer feature — keep as-is.
- **Terminal panes**: Reliable, used daily as terminal replacement.
- **Agent lifecycle**: ~75% of spec. Critical bugs: lock poisoning, race conditions, no polling scheduler, completed sessions polled forever.
- **Review surface**: ~95% of v1 spec. Working diff parsing and panel UI, but file status not visually rendered, no auto-refresh.
- **Editor**: Basic editing works. Line-based model, hand-coded lexical highlighting (tree-sitter imported but unused), no find-replace, no tabs, no LSP.

## Architecture: Parallel Tracks

```
Track A (Agent + Review)         Track B (Editor)
────────────────────────         ───────────────
A1: Critical agent fixes         B1: Buffer model upgrade
A2: Lifecycle completion         B2: Editor UI essentials
A3: Review polish                B3: Tree-sitter integration
A4: Test coverage                B4: LSP foundation
         │                              │
         └──────── Merge Point ─────────┘
                       │
              C1: Cursor ACP provider
              C2: Review ↔ Editor integration
              C3: Unified attention flow
```

Tracks A and B do not overlap in crates and can be worked on simultaneously.

## Track A: Agent Stability + Review

### A1: Critical Agent Fixes

**Goal:** Make agents stable for daily use with multiple parallel sessions.

1. **Lock poisoning** — Switch to `parking_lot::Mutex` (non-poisoning) in `agent_sessions.rs`. This eliminates the entire class of silent-empty-return bugs without adding `.expect()` noise everywhere.

2. **State machine validation** — Add transition guard in `session.rs`. Valid transitions: `Planned→Starting→Running→Waiting→Completed/Failed/Cancelled`. Reject invalid transitions with logging. Current code accepts any transition silently (`detail.session.lifecycle = lifecycle`).

3. **Polling scheduler** — Move polling out of the synchronous UI loop (`main.rs:3260`) into a background task. Default interval: 200ms, configurable 100-500ms. Current inline polling blocks UI frames if any provider is slow (>16ms).

4. **Session expiry** — Stop polling Completed/Failed sessions after 30 seconds. Currently they accumulate indefinitely and consume polling cycles forever.

5. **Race condition in poll_surface** — `agent_sessions.rs:465-490` fetches daemon agent list and details non-atomically. Use snapshot-based approach or retry logic.

### A2: Agent Lifecycle Completion

1. **Capability validation** — Check `AgentSessionCapabilities` before calling `respond_approval()`, `send_turn()`, `resume()`. Return clear error: "provider X does not support Y".

2. **Approval flow** — Define `AXIS_APPROVAL_REQUEST` line prefix in CLI protocol. Currently approval requests only arrive via full `AXIS_EVENT` JSON, which is unreliable.

3. **Error classification** — Replace `map_err(|e| e.to_string())` with `AgentError` enum:
   - `ProviderNotFound` — user config issue
   - `NetworkTimeout` — transient, retry
   - `SessionCompleted` — state machine issue
   - `UnsupportedOperation` — capability mismatch

   UI shows context-appropriate recovery steps based on error variant.

4. **Daemon health + reconnect** — Periodic health check. Re-sync sessions on daemon restart. Explicit indicator in UI showing which backend (local/daemon) is active.

5. **Workdesk binding consistency** — Fix post-hoc patching of `workdesk_id` in `record_for_key()`. `SessionManager` should know `workdesk_id` from creation, not get it patched in by the bridge.

### A3: Review Surface Polish

1. **File status tinting** — `file_review_aggregate()` already computes AllReviewed/HasFollowUp/InProgress. Render color in file list: green (reviewed), yellow (follow-up), gray (in progress).

2. **Auto-refresh** — When payload is stale, attempt daemon reconnect and diff update. Retry with backoff, non-blocking.

3. **Actionable notices** — Replace generic messages with actionable guidance:
   - "Base branch not available" → "Run `git fetch origin` or configure base branch"
   - Include action button where possible.

4. **Hunk line counts** — Show `+X/-Y` next to hunk tabs for quick size assessment.

### A4: Test Coverage

Minimum target test suites:

- **Lifecycle state machine**: All valid transitions + rejection of invalid ones
- **Approval flow**: request → response → decision → lifecycle update
- **Daemon fallback**: daemon up → start via daemon; daemon down → fallback to local
- **Multi-session**: 3+ sessions in parallel, no polling starvation
- **CLI protocol**: All marker types (AXIS_EVENT, AXIS_ATTENTION, AXIS_STATUS, AXIS_CMD), malformed JSON rejection
- **Worktree service**: create, attach, refresh, review payload generation

## Track B: Editor Evolution

### B1: Buffer Model Upgrade

**Goal:** Replace line-based String model with a structure that handles large files and supports LSP.

**Solution: Rope-based buffer** (like Zed/xi-editor).

1. Introduce `Rope` type via `ropey` crate (battle-tested, good Rust ecosystem integration). Replace `text: String` + `line_starts: Vec<usize>`.

2. Undo/redo via operations (insert/delete) instead of full-text snapshots. Remove the 64 snapshot / 8MB limit.

3. Add `document_version: u64` — incremented on each edit. Required for LSP document sync.

4. Add `TextDelta` type (range + replacement text). Every mutation produces a delta. Foundation for `textDocument/didChange` incremental sync.

5. Preserve API compatibility: `line_text()`, `line_count()`, `offset_for_line_col()` remain but operate over rope internally.

### B2: Editor UI Essentials

**Goal:** Reach "no need to open Zed for small edits" level.

1. **Find-replace** — Extend current Cmd+F:
   - Add replace field (Cmd+H)
   - Replace one / Replace all
   - Case sensitivity toggle
   - Regex toggle (optional, can defer)

2. **Visual tabs** — Surface stack exists in pane model but has no UI. Render tab bar above pane with filename + dirty indicator (●). Click to switch, middle-click to close.

3. **Go-to-line dialog** — Cmd+G, text input, jump. `move_active_editor_to_line()` already exists, needs UI only.

4. **File picker (Cmd+P)** — Fuzzy search over files in worktree. Use existing workspace palette as foundation, add file index.

5. **Keyboard shortcuts** (macOS standard Cmd modifier):
   - Cmd+D (duplicate line)
   - Cmd+/ (toggle comment)
   - Cmd+Shift+K (delete line)
   - Option+Up/Down (move line)
   - Cmd+[ / ] (indent/outdent)

### B3: Tree-sitter Integration

**Goal:** Replace hand-coded lexical highlighting with tree-sitter.

Tree-sitter is already in `Cargo.toml` but unused. Steps:

1. Connect tree-sitter parsers for daily languages: Rust, TypeScript, JavaScript, JSON, TOML, YAML, Markdown, Go, Python, Swift.

2. Incremental parsing — tree-sitter accepts text edits and updates tree without full reparse. Integrate with `TextDelta` from B1.

3. Highlight queries — use standard `highlights.scm` from nvim-treesitter or Zed.

4. Replace `highlight_line()` — walk tree-sitter captures for visible lines instead of keyword matching.

5. Remove hand-coded keyword lists and comment/string detection.

### B4: LSP Foundation (Architecture)

**Goal:** Infrastructure for LSP without implementing all features. Enough that completion and go-to-definition can be added later without rewriting.

1. **Language Server Manager** — New crate `axis-lsp`:
   - Spawn/stop LSP servers per language per worktree
   - JSON-RPC transport over stdio
   - Server capability negotiation

2. **Async bridge** — GPUI is synchronous for rendering. Channel-based approach:
   - Editor sends requests into channel
   - Background task communicates with LSP
   - Responses arrive via callback / GPUI notification
   - Similar pattern to existing agent polling

3. **Document sync** — Connect `TextDelta` from B1 to `textDocument/didOpen`, `didChange`, `didClose`. Incremental sync.

4. **Completion UI skeleton** — Popup on completion items. Arrow navigation, Enter to apply. No complex filtering/scoring — LSP server handles that.

5. **Diagnostics rendering** — On `publishDiagnostics`:
   - Squiggly underlines on affected ranges
   - Gutter icons (error/warning)
   - Hover tooltip with message

**Not in B4 scope:** code actions, rename across files, workspace symbols, formatting.

## Merge Point

### C1: Cursor ACP Provider

**Prerequisites:** A1-A2 complete, B1-B2 minimum.

1. **Adapter** — New `CursorProvider` in `axis-agent-runtime/src/adapters/cursor.rs`. Implements `AgentProvider` trait. Maps Cursor's ACP protocol to axis lifecycle/attention/event model.

2. **Provider profile** — Add `"cursor"` to `ProviderRegistry`. Bin resolver for Cursor binary. Configuration via environment or settings.

3. **Capability mapping** — Define which capabilities Cursor supports (turns, approvals, terminal attachment). Fill `AgentSessionCapabilities` accordingly.

4. **Provider picker UI** — Cursor appears in agent provider popup alongside Codex and Claude Code.

### C2: Review ↔ Editor Integration

1. **Diff view in editor** — Clicking a file in review panel opens it in editor surface with change highlighting (green/red gutter annotations). Inline annotations first, not side-by-side.

2. **Jump from review to editor** — Already works via `editor_jump_line_for_review_row()`, but with the new editor it will be more precise (go-to-line + scroll + highlight).

3. **Hunk navigation in editor** — Cmd+] / Cmd+[ to jump between hunks in an open file. Editor knows about review state via shared workdesk context.

### C3: Unified Attention Flow

1. **Normalized attention** — All providers map their signals to the unified `AgentAttention` enum (Quiet, Working, NeedsInput, NeedsReview, Error). Cursor adapter does this mapping.

2. **Attention in sidebar** — Sidebar shows all active sessions with status. "Jump to next needing attention" works cross-provider.

3. **Notification escalation** — If an agent waits for input >2 minutes, visual escalation (badge, optional sound). Configurable per-user.

## Non-Goals (Explicit)

- **Cross-platform** — macOS only
- **Team collaboration** — single-user, no session sharing
- **GitHub PR/CI integration** — review stays local git only
- **Plugin system** — no third-party extension API
- **Remote development** — everything local
- **Vi/Emacs keybindings** — standard macOS shortcuts only
- **ACP host mode** — axis does not act as server for external clients
- **Full Zed/Cursor feature parity** — editor level B + LSP foundation, not full IDE replacement
- **Browser embedding** — no webview surfaces
