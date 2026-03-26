# ACP-First Agent Runtime Design

## Summary

`axis` should evolve from a spatial terminal prototype into an ACP-first
workspace for agent-driven development.

The next six-week wave should not try to match `Superset` or `Conductor`
feature-for-feature.
It should build the minimum orchestration baseline that makes `axis` feel
showable as its own product:

1. worktree-backed workdesks;
2. ACP-backed agent sessions as first-class panes;
3. attention routing for active agents;
4. compact operational metadata in the desk rail;
5. a minimal review surface for "what changed in this desk";
6. a local automation surface that can create and inspect agent sessions.

The key constraint is that `axis` must keep its spatial advantage.
The result should feel like a spatial operating system for live development
contexts, not just another worktree manager.

## Problem Statement

Today, `axis` already has the beginning of the right UX thesis:

1. `workdesk` is a real entity.
2. panes and surfaces are first-class.
3. `free`, `grid`, and `classic-split` are treated as layout lenses.
4. terminal panes are backed by live PTY sessions.

But the current agent story is still closer to "a terminal preset that runs an
agent command" than to a first-class agent runtime.

Against `Superset` and `Conductor`, the main gaps are:

1. no worktree-native execution boundary per desk;
2. no strong agent lifecycle model;
3. no reliable attention model for multiple active agents;
4. no compact review loop around a desk;
5. no deep automation surface for agent/session orchestration.

Against `Aizen`, the long-term gap is broader orchestration and governance, but
that is not the right target for this wave.

## Product Outcome For This Wave

At the end of this six-week effort, `axis` should support this demo:

1. Create an `Implementation` or `AgentReview` workdesk.
2. Bind that desk to a new or existing git worktree.
3. Spawn one shell pane and two agent panes on the same desk.
4. Run provider-backed agents through a shared ACP-oriented runtime contract.
5. See lifecycle and attention state in the rail and lightweight notifications.
6. Jump directly to the next agent that is waiting, blocked, or failed.
7. Inspect a minimal review surface for changed files, branch state, and desk
   readiness.
8. Drive the same flow through a local Unix socket and CLI that share the same
   command schema.

If that flow is reliable and feels coherent inside the spatial UI, the wave is
successful.

## Non-Goals

This wave should explicitly avoid:

1. a separate external ACP daemon;
2. team or multi-device orchestration;
3. full GitHub review, CI, and merge automation parity with orchestration-first
   tools;
4. browser embedding;
5. a plugin platform;
6. a full custom code editor;
7. broad refactoring unrelated to ACP, worktrees, or attention.

## UX Guardrail For This Wave

This wave does not try to solve every UI polish issue at once, but it should
include one explicit content-first pass so the demo does not read as purely
"prototype-coded".

Minimum UX scope:

1. keep the desk rail compact and operational;
2. move developer diagnostics out of always-visible chrome where possible;
3. keep notifications lightweight and event-driven instead of adding more
   permanent badges and panels.

Larger chrome reduction work can follow in the next wave.

## Core Architectural Decision

### Embedded ACP Host Inside `axis`

The recommended design is an embedded ACP host inside the local `axis` process
model.

The UX source of truth remains the `workdesk` and pane graph, but agent
execution should no longer be modeled as "just another PTY child process".
Instead, agent sessions become first-class runtime entities with:

1. provider identity;
2. lifecycle state;
3. attention state;
4. capabilities;
5. event history;
6. optional terminal attachment;
7. worktree and cwd binding.

This lets `axis` keep its current UI strengths while making agent runtime,
automation, and attention behavior predictable.

### Why Not An External Supervisor

A separate supervisor process could be cleaner long-term, but it is too much
surface area for this wave:

1. extra packaging and lifecycle complexity;
2. another control protocol to invent and stabilize;
3. more failure modes before the product loop is proven.

The six-week goal is to prove the product loop, not to finalize the ultimate
deployment topology.

## Core Model

### Worktree-Backed Workdesks

A `workdesk` remains the primary UX container, but it gains an optional
execution binding:

1. worktree root path;
2. branch name;
3. base branch;
4. ahead/behind summary;
5. setup state;
6. last review summary timestamp.

The desk is still spatial.
The worktree simply becomes the execution boundary for shells, agents, review,
and desk metadata.

### Agent Session

Each agent session should have its own record independent of the pane that
renders it.

Minimum fields:

1. stable `agent_session_id`;
2. provider profile id;
3. transport kind: `cli_wrapped` or `native_acp`;
4. attached `workdesk` and optional `surface_id`;
5. cwd;
6. lifecycle state;
7. attention state;
8. human-readable status text;
9. timestamps for started/updated/completed;
10. event log cursor or revision id.

### Normalized Lifecycle

The UI should not care whether the provider is `Claude Code`, `Codex`,
`Gemini CLI`, or a future native ACP agent.

All providers should normalize into one lifecycle:

1. `planned`
2. `starting`
3. `running`
4. `waiting`
5. `completed`
6. `failed`
7. `cancelled`

### Attention Model

Lifecycle alone is not enough.
`axis` needs a separate attention signal for routing the user's focus.

First-wave attention states:

1. `quiet`
2. `working`
3. `needs_input`
4. `needs_review`
5. `error`

This state should exist at:

1. agent session level;
2. pane/surface level;
3. workdesk aggregate level.

### Provider Profiles

The runtime should treat providers as named profiles rather than hard-coded
special cases.

First-wave baseline profiles:

1. `codex`
2. `claude-code`

`codex` is the canonical adapter for full first-wave lifecycle and attention
support.
`claude-code` is also a first-wave baseline provider, but may start with a narrower
process-level adapter in this implementation wave.

Stretch profile after the baseline proves out:

1. `gemini-cli`

Each profile defines:

1. launch command;
2. required environment checks;
3. default capability set;
4. adapter implementation;
5. optional setup hints for missing tools.

### Review Summary

The first review surface should stay intentionally small.

For the demo bar, it only needs to answer:

1. which branch or worktree is active;
2. whether the desk is dirty, ahead, or behind;
3. which files changed;
4. whether the desk is ready for human review.

Inline unified diff, GitHub review state, and CI checks are explicitly deferred.

## System Components

### `crates/axis-core`

Add shared types for:

1. worktree binding metadata;
2. agent session ids and records;
3. lifecycle and attention enums;
4. provider profile metadata;
5. desk review summary metadata.

This crate remains dependency-light and owns stable domain types only.

### `crates/axis-agent-runtime`

Add a new crate that owns:

1. ACP-oriented session manager;
2. provider registry;
3. session event bus and lifecycle transitions;
4. CLI-wrapped adapters for first-wave providers;
5. future native ACP transport boundary.

This crate is the main architectural addition in the wave.

### `crates/process-manager`

Extend process launching so the runtime can:

1. launch provider commands inside a worktree;
2. set predictable env vars and cwd;
3. choose PTY-backed or non-PTY process strategies where needed;
4. support stop/restart semantics cleanly.

### `crates/axis-terminal`

Keep terminal rendering responsibility here.
Agent panes may still render a terminal session, but the terminal becomes an
attachment to a richer agent runtime record instead of the whole model.

### `apps/axis-app`

`axis-app` stays the composition layer and should own:

1. workdesk templates and creation flows;
2. worktree-aware desk metadata;
3. attention badges and jump actions;
4. a compact operational rail;
5. a minimal review surface;
6. automation handlers that create/manage agent sessions;
7. the in-process Unix socket host for the shared automation schema.

New ACP-related logic should be added in focused modules rather than extending
the existing monolithic `main.rs` further.

### `apps/axis-cli`

Expose automation commands for:

1. create/select worktree-backed desks;
2. spawn ACP-backed agent sessions;
3. list session state;
4. set or clear review/attention state where appropriate;
5. inspect desk status.

`axis-cli` should remain a thin client over the same shared request schema used
by the local Unix socket host in `axis-app`.

## Execution Model

### Internal Contract

Internally, `axis` should speak one agent runtime contract modeled after ACP
concepts even when a provider does not offer native ACP.

This means the runtime always thinks in terms of:

1. session start;
2. capability declaration;
3. event stream;
4. status updates;
5. attention-worthy state changes;
6. stop/cancel;
7. completion or failure.

### First-Wave Provider Strategy

The first wave should support a hybrid provider model:

1. use CLI-backed agents now;
2. wrap them behind adapters that expose the normalized runtime contract;
3. keep the runtime ready for native ACP providers later.

This avoids waiting for perfect upstream ACP support before shipping the first
useful product loop.

### CLI Adapter MVP

The first implementation must keep provider signal extraction tightly bounded.

For the six-week wave:

1. use `codex` as the canonical CLI provider profile for the demo bar;
2. implement real lifecycle and attention derivation only for that provider;
3. allow additional provider profiles to launch with degraded process-level
   status until richer adapter logic is proven.

The canonical provider adapter may use a mix of:

1. process lifecycle;
2. known output markers;
3. explicit wrapper hooks;
4. operator actions such as "mark needs review".

It should not rely on brittle terminal scraping as the only signal source.
If richer events are unavailable, the adapter should fall back to the smallest
credible contract instead of pretending to be fully structured.

### Terminal Attachment

Agent sessions may expose an attached terminal surface for human visibility and
manual intervention.

Important constraint:
the terminal is not the source of truth for session state.

The source of truth is the agent runtime record.
The terminal is a rendered view over one execution channel.

## User Flow

The intended user flow for this wave is:

1. Create a desk from a template such as `Implementation`, `Debug`, or
   `AgentReview`.
2. Choose whether the desk binds to a new branch, existing branch, or existing
   working tree path.
3. `axis` creates or attaches the worktree and records its metadata on the
   desk.
4. `axis` opens a shell pane inside that worktree.
5. The user spawns one or more agent panes by selecting a provider profile.
6. The runtime starts agent sessions and emits normalized lifecycle and
   attention events.
7. The rail and desk chrome show branch, cwd, progress, and which pane needs
   the next action.
8. A small review surface answers "what changed in this desk?" without forcing
   the user into a separate orchestration product.

## Wave Phasing

The wave should ship in controlled slices instead of treating every subsystem as
equally critical on day one.

### Minimum Shippable Mid-Wave

The mid-wave bar is:

1. one worktree-backed desk;
2. one shell pane;
3. one canonical ACP-backed agent session;
4. desk-level lifecycle and attention state;
5. local automation via shared CLI/socket schema.

If later scope slips, this slice must still be demoable.

### Full Six-Week Bar

The full wave adds:

1. a second provider profile;
2. aggregate rail metadata and jump-to-attention actions;
3. a compact review surface;
4. template-driven desk creation for `Implementation`, `Debug`, and
   `AgentReview`.

## Error Handling

The runtime should make failure states explicit and recoverable.

### Provider Missing Or Misconfigured

If the provider command is not available:

1. fail before pane launch when possible;
2. show a clear install/config hint;
3. keep the desk usable for shell work.

### Worktree Creation Failure

If creating or attaching a worktree fails:

1. do not create a half-bound desk silently;
2. show a visible desk-level error;
3. offer retry or attach-existing fallback.

### Agent Crash Or Early Exit

If an agent exits unexpectedly:

1. preserve terminal history;
2. mark lifecycle `failed`;
3. raise desk attention;
4. keep the session inspectable.

### Stale Or Conflicted Worktree Metadata

Worktree metadata should be treated as refreshable state, not perfect truth.

If branch status, ahead/behind state, or review summary becomes stale:

1. show the last known state clearly;
2. allow explicit refresh;
3. surface merge-conflict or dirty-state failures at the desk level instead of
   burying them in provider-specific UI.

### ACP Capability Mismatch

If a provider lacks a desired capability:

1. degrade to the CLI-wrapped path when possible;
2. surface the capability gap in the session metadata;
3. avoid special-case UI logic outside the runtime layer.

## Testing Strategy

This wave should emphasize testable boundaries rather than UI-only confidence.

### Unit Tests

1. `axis-core` type behavior for worktree and session records;
2. lifecycle transitions and attention aggregation in the runtime;
3. provider profile validation and missing-tool checks;
4. worktree metadata parsing and review summary helpers.

### Integration Tests

1. temporary-repo tests for worktree creation and status collection;
2. fake-provider tests that simulate running, waiting, completion, and failure;
3. CLI tests for new automation commands and payloads.

### Notification Scope

Notifications in this wave should be in-app first:

1. desk rail badge changes;
2. lightweight toast on attention transitions;
3. optional later hook for desktop notifications if the in-app model proves
   useful.

The runtime should only emit notifications for meaningful transitions such as
`needs_input`, `needs_review`, and `error`.

### Manual Demo Script

Maintain one explicit smoke-demo script for the six-week bar:

1. create desk;
2. bind worktree;
3. launch shell;
4. launch two agents;
5. trigger attention;
6. inspect review summary;
7. jump to the blocked agent;
8. stop and restart one session.

## Success Criteria

The wave is complete when all of the following are true:

1. `axis` can create or attach a worktree-backed desk from the local app or
   CLI.
2. At least two provider-backed agent sessions can run on the same desk.
3. Agent panes are powered by the ACP-oriented runtime model, not by raw PTY
   state alone.
4. Desk-level and pane-level attention routing works for waiting and failed
   sessions.
5. The desk rail shows compact, operational metadata instead of descriptive
   chrome.
6. A review surface can summarize the desk's local git state.
7. The product is demoable without explaining away obvious orchestration gaps.

## Open Questions

The following decisions can stay open during implementation as long as the
runtime boundary is kept clean:

1. whether native ACP transport lands in this wave or the next one;
2. how much diff detail the first review surface should show;
3. whether desk setup scripts belong in `axis` proper or the automation layer;
4. how many provider-specific capabilities deserve custom UI in v1 of the
   runtime;
5. whether `claude-code` should remain process-only in the next wave or gain
   richer lifecycle and attention support.
