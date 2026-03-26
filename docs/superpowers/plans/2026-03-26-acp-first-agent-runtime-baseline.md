# ACP-First Agent Runtime Baseline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a six-week, demoable `axis` baseline where workdesks are backed by git worktrees, agent panes run through an ACP-oriented runtime, attention is routable, and the app exposes a minimal review loop without losing its spatial UX.

**Architecture:** Keep `apps/axis-app` as the UX shell and in-process Unix socket host, add a new `crates/axis-agent-runtime` crate for worktree and agent-session orchestration, and extend `crates/axis-core` with shared worktree/session/automation types. The first wave includes two baseline CLI providers: `codex` as the canonical full-lifecycle adapter and `claude-code` as a first-wave process-oriented adapter, while preserving room for future native ACP transport.

**Tech Stack:** Rust 2021, GPUI, `portable-pty`, Unix domain sockets, `serde`, git CLI, vendored `libghostty-vt`, targeted app module extraction from `apps/axis-app/src/main.rs`.

---

## Scope Locks

These decisions are locked before implementation starts:

1. Canonical first-wave provider: `codex`.
2. `claude-code` is also a first-wave baseline provider and ships in this wave through a process-only adapter.
3. Unix socket host remains inside `apps/axis-app`; `apps/axis-cli` is a thin client.
4. First review surface shows branch/worktree status, ahead/behind, dirty state, and file list only.
5. Notifications are in-app only for this wave: rail badges plus lightweight toast.
6. Native ACP transport is explicitly deferred unless the canonical adapter lands early.
7. `gemini-cli` is a stretch profile and is explicitly deferred from the baseline.

If one of these changes, update the spec and this plan before touching code.

## Planned File Structure

### Workspace and shared types

- Modify: `Cargo.toml`
  Add the new `crates/axis-agent-runtime` workspace member.
- Modify: `crates/axis-core/Cargo.toml`
  Add `serde` for shared serializable records.
- Modify: `crates/axis-core/src/lib.rs`
  Re-export new shared domain modules.
- Create: `crates/axis-core/src/agent.rs`
  Shared agent session ids, lifecycle, attention, provider profile metadata.
- Create: `crates/axis-core/src/worktree.rs`
  Worktree binding, branch status, review summary metadata.
- Create: `crates/axis-core/src/automation.rs`
  Shared automation request/response schema for socket and CLI.

### Agent runtime

- Create: `crates/axis-agent-runtime/Cargo.toml`
- Create: `crates/axis-agent-runtime/src/lib.rs`
- Create: `crates/axis-agent-runtime/src/session.rs`
  Session manager, state transitions, revision tracking.
- Create: `crates/axis-agent-runtime/src/provider.rs`
  Provider registry and adapter trait.
- Create: `crates/axis-agent-runtime/src/events.rs`
  Normalized runtime events and attention transitions.
- Create: `crates/axis-agent-runtime/src/worktree.rs`
  Worktree create/attach/status helpers using git.
- Create: `crates/axis-agent-runtime/src/adapters/mod.rs`
- Create: `crates/axis-agent-runtime/src/adapters/fake.rs`
  Deterministic adapter for tests and smoke flows.
- Create: `crates/axis-agent-runtime/src/adapters/codex.rs`
  Canonical provider adapter with real lifecycle and attention derivation.
- Create: `crates/axis-agent-runtime/src/adapters/process_only.rs`
  `claude-code` first-wave baseline path with process-level states only.

### Process and terminal integration

- Modify: `crates/process-manager/src/lib.rs`
  Add launch options for cwd/env/process strategy and clean stop/restart.
- Modify: `crates/axis-terminal/src/lib.rs`
  Attach terminal sessions to richer agent runtime records where needed.

### App integration

- Modify: `apps/axis-app/Cargo.toml`
  Add `axis-agent-runtime` and any small supporting deps.
- Modify: `apps/axis-app/src/main.rs`
  Reduce to composition and rendering glue; delegate ACP/worktree/review logic.
- Create: `apps/axis-app/src/automation.rs`
  Socket handler and request dispatch using shared schema.
- Create: `apps/axis-app/src/worktrees.rs`
  Desk-to-worktree binding helpers and UI state glue.
- Create: `apps/axis-app/src/agent_sessions.rs`
  Surface-to-session mapping and runtime coordination.
- Create: `apps/axis-app/src/attention.rs`
  Session -> pane -> desk aggregation and jump helpers.
- Create: `apps/axis-app/src/review.rs`
  Compact review surface data shaping and UI helpers.

### CLI and tests

- Modify: `apps/axis-cli/Cargo.toml`
  Add `axis-core` so the CLI can reuse shared request types.
- Modify: `apps/axis-cli/src/main.rs`
  Rebuild aliases and parsing around the shared automation schema.
- Create: `crates/axis-core/tests/agent_records.rs`
- Create: `crates/axis-agent-runtime/tests/session_lifecycle.rs`
- Create: `crates/axis-agent-runtime/tests/worktree_service.rs`
- Create: `crates/axis-agent-runtime/tests/fake_provider.rs`
- Create: `crates/axis-agent-runtime/tests/codex_adapter.rs`
- Add/modify tests in: `apps/axis-cli/src/main.rs`
- Add inline unit tests in: `apps/axis-app/src/attention.rs`, `apps/axis-app/src/review.rs`, `apps/axis-app/src/worktrees.rs`

### Demo and docs

- Modify: `justfile`
  Add a smoke/demo helper if it pays off.
- Create: `scripts/smoke-acp-demo.sh`
  Manual demo flow for the six-week bar.
- Modify: `docs/v0-prototype-scope.md`
  Only if the proven scope clearly changes after implementation.
- Modify: `docs/workdesk-layout-modes.md`
  Only if worktree-bound desks or attention routing require a doc update.

## Milestone Map

- End of week 2: shared domain model + runtime crate + fake provider + worktree service are testable.
- End of week 4: one canonical provider runs inside a worktree-backed desk and emits attention state into the app.
- End of week 6: minimal review surface, `claude-code` baseline provider, CLI/socket control, and smoke demo are all working.

### Task 1: Week 1 - Shared Domain Types And Schema

**Files:**
- Modify: `crates/axis-core/Cargo.toml`
- Modify: `crates/axis-core/src/lib.rs`
- Create: `crates/axis-core/src/agent.rs`
- Create: `crates/axis-core/src/worktree.rs`
- Create: `crates/axis-core/src/automation.rs`
- Test: `crates/axis-core/tests/agent_records.rs`

- [ ] **Step 1: Add `serde` to `crates/axis-core/Cargo.toml`**

Use only `serde` with `derive`; do not add `serde_json` here.

- [ ] **Step 2: Split shared agent types into `crates/axis-core/src/agent.rs`**

Start with the smallest stable surface:

```rust
pub enum AgentLifecycleState {
    Planned,
    Starting,
    Running,
    Waiting,
    Completed,
    Failed,
    Cancelled,
}

pub enum AgentAttentionState {
    Quiet,
    Working,
    NeedsInput,
    NeedsReview,
    Error,
}
```

- [ ] **Step 3: Create `crates/axis-core/src/worktree.rs`**

Define:

```rust
pub struct WorktreeBinding {
    pub root_path: String,
    pub branch: String,
    pub base_branch: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub dirty: bool,
}
```

Keep the first review summary lightweight: status + file list only.

- [ ] **Step 4: Create `crates/axis-core/src/automation.rs`**

Define a shared request/response schema for:

1. `worktree.create_or_attach`
2. `worktree.status`
3. `agent.start`
4. `agent.stop`
5. `agent.list`
6. `review.summary`
7. `attention.next`
8. `state.current`

- [ ] **Step 5: Re-export new modules from `crates/axis-core/src/lib.rs`**

Keep existing pane/workdesk types intact and additive.

- [ ] **Step 6: Write `crates/axis-core/tests/agent_records.rs`**

Cover:

1. lifecycle serialization round-trips;
2. attention serialization round-trips;
3. review summary defaults;
4. automation request encoding for agent start and worktree attach.

- [ ] **Step 7: Run the core test pass**

Run: `cargo test -p axis-core --test agent_records -v`

Expected: PASS with all shared type and schema tests green.

- [ ] **Step 8: Commit**

```bash
git add crates/axis-core/Cargo.toml crates/axis-core/src/lib.rs \
  crates/axis-core/src/agent.rs crates/axis-core/src/worktree.rs \
  crates/axis-core/src/automation.rs crates/axis-core/tests/agent_records.rs
git commit -m "feat: add shared ACP and worktree records"
```

### Task 2: Week 1-2 - Runtime Crate And Worktree Service

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/axis-agent-runtime/Cargo.toml`
- Create: `crates/axis-agent-runtime/src/lib.rs`
- Create: `crates/axis-agent-runtime/src/session.rs`
- Create: `crates/axis-agent-runtime/src/provider.rs`
- Create: `crates/axis-agent-runtime/src/events.rs`
- Create: `crates/axis-agent-runtime/src/worktree.rs`
- Create: `crates/axis-agent-runtime/src/adapters/mod.rs`
- Create: `crates/axis-agent-runtime/src/adapters/fake.rs`
- Test: `crates/axis-agent-runtime/tests/session_lifecycle.rs`
- Test: `crates/axis-agent-runtime/tests/worktree_service.rs`
- Test: `crates/axis-agent-runtime/tests/fake_provider.rs`

- [ ] **Step 1: Add `crates/axis-agent-runtime` to the workspace**

Make the new crate the home for runtime orchestration, not UI rendering.

- [ ] **Step 2: Create the runtime crate manifest**

Start with only the deps you need now: `anyhow`, `serde`, `axis-core`, and one
temp-repo test helper such as `tempfile`.

- [ ] **Step 3: Define the provider contract in `src/provider.rs`**

Keep the adapter trait narrow:

```rust
pub trait AgentAdapter {
    fn start(&self, request: StartAgentRequest) -> Result<StartedSession>;
    fn stop(&self, session_id: AgentSessionId) -> Result<()>;
    fn poll(&self, session_id: AgentSessionId) -> Result<Vec<RuntimeEvent>>;
}
```

`StartAgentRequest`, `StartedSession`, and `RuntimeEvent` should live in
`crates/axis-agent-runtime`, while stable ids and enums remain in `axis-core`.

- [ ] **Step 4: Implement the session manager in `src/session.rs`**

The manager should own:

1. session registry;
2. lifecycle transitions;
3. attention transitions;
4. revision tracking for UI refreshes.

- [ ] **Step 5: Implement worktree helpers in `src/worktree.rs`**

Support:

1. create new worktree from base branch;
2. attach existing path;
3. read branch name;
4. read ahead/behind;
5. detect dirty state;
6. collect changed file list.

- [ ] **Step 6: Add a deterministic fake provider in `src/adapters/fake.rs`**

It must be able to emit a scripted sequence:

1. `starting`
2. `running`
3. `needs_review`
4. `completed`

without spawning a real external tool.

- [ ] **Step 7: Write runtime tests**

Use:

1. `tests/session_lifecycle.rs` for state transitions;
2. `tests/worktree_service.rs` for temp-repo workflows;
3. `tests/fake_provider.rs` for scripted event sequences.

- [ ] **Step 8: Run the runtime test pass**

Run: `cargo test -p axis-agent-runtime -v`

Expected: PASS with fake-provider and worktree tests green.

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml crates/axis-agent-runtime
git commit -m "feat: scaffold ACP runtime and worktree service"
```

### Task 3: Week 2 - Process Launch Primitives And Canonical Adapter

**Files:**
- Modify: `crates/process-manager/Cargo.toml`
- Modify: `crates/process-manager/src/lib.rs`
- Modify: `crates/axis-agent-runtime/src/provider.rs`
- Modify: `crates/axis-agent-runtime/src/session.rs`
- Create: `crates/axis-agent-runtime/src/adapters/codex.rs`
- Test: inline tests in `crates/process-manager/src/lib.rs`
- Test: `crates/axis-agent-runtime/tests/codex_adapter.rs`

- [ ] **Step 1: Extend process launching in `crates/process-manager/src/lib.rs`**

Add a launch spec with explicit cwd, env, and strategy:

```rust
pub struct ProcessLaunchSpec {
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    pub use_pty: bool,
}
```

- [ ] **Step 2: Preserve the existing shell helpers and extract one pure launch helper**

Do not break `login_shell()` or the current terminal smoke path while adding the
new launch API.
The new helper should be unit-testable without spawning a real CLI.

- [ ] **Step 3: Implement `codex.rs` as the canonical adapter**

Use the smallest credible signal set:

1. process lifecycle;
2. explicit wrapper hooks if needed;
3. known output markers only where they are stable;
4. manual "mark needs review" fallback if richer signals are unavailable.

- [ ] **Step 4: Keep adapter logic out of UI code**

`apps/axis-app` should consume runtime state, not parse terminal output.

- [ ] **Step 5: Write `tests/codex_adapter.rs` with a fake child/process strategy by default**

Cover:

1. successful launch;
2. waiting/needs-review transition;
3. unexpected exit -> `failed`;
4. stop/cancel semantics.

Any smoke test that hits the real `codex` binary must be `#[ignore]` and gated
behind `CODEX_ADAPTER_TESTS=1`.

- [ ] **Step 6: Run focused verification**

Run: `cargo test -p axis-agent-runtime codex_adapter -v`

Expected: PASS with lifecycle mapping working for the canonical adapter.

- [ ] **Step 7: Run regression verification**

Run: `cargo test -p process-manager -v`

Expected: PASS with old shell/process behavior preserved.

- [ ] **Step 8: Commit**

```bash
git add crates/process-manager/Cargo.toml crates/process-manager/src/lib.rs \
  crates/axis-agent-runtime/src/provider.rs \
  crates/axis-agent-runtime/src/session.rs \
  crates/axis-agent-runtime/src/adapters/codex.rs \
  crates/axis-agent-runtime/tests/codex_adapter.rs
git commit -m "feat: add canonical agent adapter runtime"
```

### Task 4: Week 2-3 - Worktree-Backed Desks In The App

**Files:**
- Modify: `apps/axis-app/Cargo.toml`
- Modify: `apps/axis-app/src/main.rs`
- Modify: `crates/axis-terminal/src/lib.rs`
- Create: `apps/axis-app/src/worktrees.rs`
- Create: `apps/axis-app/src/agent_sessions.rs`
- Test: inline tests in `apps/axis-app/src/worktrees.rs`

- [ ] **Step 1: Add `axis-agent-runtime` to `apps/axis-app/Cargo.toml`**

Do not add more app-only dependencies than necessary.

- [ ] **Step 2: Declare `mod worktrees;` and `mod agent_sessions;` in `apps/axis-app/src/main.rs`**

Treat `main.rs` as the binary crate root and use `pub(crate)` visibility for
shared app-only types instead of assuming a library crate exists.

- [ ] **Step 3: Create `apps/axis-app/src/worktrees.rs`**

Move desk/worktree binding helpers out of `main.rs`.
Start with:

1. create desk from template;
2. bind desk to new or existing worktree;
3. refresh worktree metadata;
4. expose compact metadata for rendering.

- [ ] **Step 4: Create `apps/axis-app/src/agent_sessions.rs`**

This module should map:

1. `workdesk` -> runtime context;
2. `surface_id` -> `agent_session_id`;
3. runtime event revisions -> UI refresh state.

- [ ] **Step 5: Replace the raw agent-pane preset path**

Keep shell panes unchanged, but route new agent pane creation through the
runtime crate instead of directly treating it as a terminal preset.

- [ ] **Step 6: Modify `crates/axis-terminal/src/lib.rs` so terminal sessions can attach to agent session metadata**

Do not redesign terminal rendering.
Only add the minimum binding needed so an agent pane can remain terminal-visible
while runtime state lives outside the terminal model.

- [ ] **Step 7: Preserve current templates while making them worktree-aware**

At minimum, `Implementation`, `Debug`, and `AgentReview` should accept a
worktree binding and default shell/agent layout.

- [ ] **Step 8: Add inline tests for worktree binding helpers**

Cover:

1. desk metadata formatting;
2. attach-vs-create selection;
3. refresh behavior on missing worktree.

- [ ] **Step 9: Run app compile verification**

Run: `cargo check -p axis-app`

Expected: PASS with desk creation and agent-pane wiring compiling cleanly.

- [ ] **Step 10: Commit**

```bash
git add apps/axis-app/Cargo.toml apps/axis-app/src/main.rs \
  apps/axis-app/src/worktrees.rs apps/axis-app/src/agent_sessions.rs \
  crates/axis-terminal/src/lib.rs
git commit -m "feat: bind workdesks to runtime-backed worktrees"
```

### Task 5: Week 3-4 - Shared Automation Socket And CLI Commands

**Files:**
- Modify: `apps/axis-app/src/main.rs`
- Create: `apps/axis-app/src/automation.rs`
- Modify: `apps/axis-cli/Cargo.toml`
- Modify: `apps/axis-cli/src/main.rs`
- Modify: `crates/axis-core/src/automation.rs`
- Test: existing and new tests in `apps/axis-cli/src/main.rs`

- [ ] **Step 1: Declare `mod automation;` in `apps/axis-app/src/main.rs` and extract socket dispatch into `apps/axis-app/src/automation.rs`**

Keep `axis-app` as the in-process socket host.

- [ ] **Step 2: Switch automation handlers to the shared schema**

Do not keep parallel stringly-typed request definitions once the shared schema
exists.

- [ ] **Step 3: Implement first-wave methods**

Support:

1. `worktree.create_or_attach`
2. `worktree.status`
3. `agent.start`
4. `agent.stop`
5. `agent.list`
6. `review.summary`
7. `attention.next`
8. `state.current`

- [ ] **Step 4: Update `apps/axis-cli/src/main.rs`**

Map human-friendly aliases to the shared request types instead of duplicating
method strings by hand.

- [ ] **Step 5: Expand CLI tests**

Cover:

1. nested `key=value` parsing;
2. `agent.start` profile selection;
3. worktree attach/create flags;
4. review and attention commands.

- [ ] **Step 6: Run CLI verification**

Run: `cargo test -p axis-cli -v`

Expected: PASS with new command aliases and payload mapping green.

- [ ] **Step 7: Run app compile verification again**

Run: `cargo check -p axis-app`

Expected: PASS with socket and CLI sharing one schema.

- [ ] **Step 8: Commit**

```bash
git add apps/axis-app/src/main.rs apps/axis-app/src/automation.rs \
  apps/axis-cli/Cargo.toml apps/axis-cli/src/main.rs \
  crates/axis-core/src/automation.rs
git commit -m "feat: unify socket and CLI automation schema"
```

### Task 6: Week 4 - Attention Model And Content-First Rail

**Files:**
- Modify: `apps/axis-app/src/main.rs`
- Create: `apps/axis-app/src/attention.rs`
- Modify: `apps/axis-app/src/agent_sessions.rs`
- Test: inline tests in `apps/axis-app/src/attention.rs`

- [ ] **Step 1: Declare `mod attention;` in `apps/axis-app/src/main.rs` and create `apps/axis-app/src/attention.rs`**

Implement pure reducers that aggregate:

1. session attention -> pane state;
2. pane state -> desk state;
3. desk state -> jump target ordering.

- [ ] **Step 2: Add a jump-to-next-attention action**

Wire a shortcut and automation method that select the next pane in:

1. `needs_input`
2. `needs_review`
3. `error`

order.

- [ ] **Step 3: Add in-app toast notifications for meaningful transitions**

Only trigger on:

1. `needs_input`
2. `needs_review`
3. `error`

Do not toast every lifecycle change.

- [ ] **Step 4: Perform the minimum content-first cleanup**

Move always-visible diagnostics such as vendor/runtime debug text into the
inspector or a dev-only path.
Keep the rail dense and operational: branch, cwd, progress, unread/attention.

- [ ] **Step 5: Add inline tests for attention aggregation**

Cover:

1. multiple sessions on one desk;
2. error winning over working;
3. jump ordering stability.

- [ ] **Step 6: Run app verification**

Run: `cargo test -p axis-app attention -v`

Expected: PASS with pure attention reducers tested.

- [ ] **Step 7: Run full app compile verification**

Run: `cargo check -p axis-app`

Expected: PASS with attention UI wiring compiling cleanly.

- [ ] **Step 8: Commit**

```bash
git add apps/axis-app/src/main.rs apps/axis-app/src/agent_sessions.rs \
  apps/axis-app/src/attention.rs
git commit -m "feat: add agent attention routing and compact rail"
```

### Task 7: Week 5 - Minimal Review Surface And Claude Code First-Wave Provider

**Files:**
- Modify: `apps/axis-app/src/main.rs`
- Create: `apps/axis-app/src/review.rs`
- Create: `crates/axis-agent-runtime/src/adapters/process_only.rs`
- Modify: `crates/axis-agent-runtime/src/adapters/mod.rs`
- Modify: `crates/axis-agent-runtime/src/provider.rs`
- Test: inline tests in `apps/axis-app/src/review.rs`

- [ ] **Step 1: Declare `mod review;` in `apps/axis-app/src/main.rs` and create `apps/axis-app/src/review.rs`**

Keep the view model tight:

```rust
pub struct DeskReviewSummaryView {
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub dirty: bool,
    pub changed_files: Vec<String>,
    pub ready_for_review: bool,
}
```

- [ ] **Step 2: Render the first review surface**

Do not build a full diff viewer.
The UI only needs branch state, file list, and a readable "ready for review"
signal.

- [ ] **Step 3: Implement `process_only.rs`**

This adapter should support launch/stop/basic lifecycle so `claude-code` ships
as a real first-wave provider without promising structured attention semantics
yet.

- [ ] **Step 4: Register `claude-code` cleanly as a first-wave provider**

It must appear in provider selection with a visible capability note such as
"basic lifecycle only".

- [ ] **Step 5: Add review reducer tests**

Cover:

1. dirty desk not ready for review;
2. clean changed-files state ready for review;
3. stale metadata refresh behavior.

- [ ] **Step 6: Run runtime verification**

Run: `cargo test -p axis-agent-runtime -v`

Expected: PASS with both `codex` and `claude-code` baseline adapters covered.

- [ ] **Step 7: Run app verification**

Run: `cargo test -p axis-app review -v`

Expected: PASS with review summary reducers green.

- [ ] **Step 8: Commit**

```bash
git add apps/axis-app/src/main.rs apps/axis-app/src/review.rs \
  crates/axis-agent-runtime/src/adapters/mod.rs \
  crates/axis-agent-runtime/src/provider.rs \
  crates/axis-agent-runtime/src/adapters/process_only.rs
git commit -m "feat: add review surface and claude-code baseline profile"
```

### Task 8: Week 6 - Demo Hardening And Smoke Script

**Files:**
- Modify: `justfile`
- Create: `scripts/smoke-acp-demo.sh`
- Modify: `docs/v0-prototype-scope.md` (only if scope wording must change)
- Modify: `docs/workdesk-layout-modes.md` (only if proven behavior differs)

- [ ] **Step 1: Create `scripts/smoke-acp-demo.sh`**

Script the demo flow:

1. create or attach worktree desk;
2. launch shell pane;
3. launch canonical provider;
4. launch `claude-code`;
5. trigger attention;
6. fetch review summary;
7. jump to next attention target.

- [ ] **Step 2: Add one `just` target if the script proves useful**

Example:

```make
smoke-acp:
    bash scripts/smoke-acp-demo.sh
```

- [ ] **Step 3: Run the full workspace compile pass**

Run: `cargo check --workspace`

Expected: PASS with the new runtime crate and app integration compiling
end-to-end.

- [ ] **Step 4: Run the targeted test pass**

Run: `cargo test -p axis-core -p axis-agent-runtime -p axis-cli -v`

Expected: PASS for core runtime and automation coverage.

- [ ] **Step 5: Rehearse the demo manually**

Verify the actual product bar:

1. worktree-backed `Implementation` desk;
2. one shell pane;
3. one `codex` agent with full attention flow;
4. one `claude-code` agent as a first-wave baseline provider with degraded lifecycle;
5. desk rail shows operational metadata;
6. review surface shows file list and ready signal;
7. CLI/socket commands can drive the same loop.

- [ ] **Step 6: Update docs only where implementation changed meaning**

Do not rewrite product docs just to match internal naming churn.

- [ ] **Step 7: Commit**

```bash
git add justfile scripts/smoke-acp-demo.sh
# Add docs only if they actually changed during implementation.
git commit -m "docs: capture ACP demo flow and updated desk behavior"
```

## Cut Line If The Schedule Slips

If the wave starts slipping after Task 5, cut in this order:

1. desktop notification hooks;
2. richer `claude-code` lifecycle polish;
3. extra desk templates beyond `Implementation`;
4. any review UI beyond file list and ready signal;
5. native ACP transport experiments.

Do **not** cut:

1. shared domain types;
2. runtime crate;
3. one canonical provider with real lifecycle/attention;
4. worktree-backed desks;
5. shared CLI/socket schema;
6. attention routing.

## Execution Notes

1. Keep new behavior additive until each task's tests pass.
2. Do not let new ACP logic expand `apps/axis-app/src/main.rs` further; every
   new subsystem should land in a focused module.
3. Favor pure reducers and service objects for testability.
4. Treat the fake provider as part of the product infrastructure, not a throwaway
   test helper.
5. Prefer one credible end-to-end loop over many half-wired provider features.
