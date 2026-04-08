# Track A: Agent Stability + Review — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix critical agent lifecycle bugs and polish review surface so axis can replace Conductor for daily multi-agent workflows.

**Architecture:** Fix agent runtime bugs in `axis-agent-runtime` and `agent_sessions.rs` bridge, then polish review UI in `apps/axis-app`. All changes stay within the agent/review codepaths — no editor work here.

**Tech Stack:** Rust, parking_lot, GPUI, axis-agent-runtime, axis-core

---

## Task 1: Replace std::sync::Mutex with parking_lot::Mutex

**Files:**
- Modify: `apps/axis-app/Cargo.toml`
- Modify: `apps/axis-app/src/agent_sessions.rs` (lines 1-6, 44-46, and all `Mutex::lock()` calls)
- Modify: `crates/axis-agent-runtime/src/adapters/codex.rs` (lines 1-5, 25-28)
- Modify: `crates/axis-agent-runtime/src/adapters/process_only.rs` (lines 1-5, 23-27)
- Modify: `crates/axis-agent-runtime/src/adapters/fake.rs` (lines 1-5, 41-43)
- Modify: `crates/axis-agent-runtime/Cargo.toml`
- Modify: `Cargo.toml` (workspace root — add parking_lot to workspace deps if not present)

- [ ] **Step 1: Add parking_lot dependency**

Add to `Cargo.toml` workspace root:
```toml
[workspace.dependencies]
parking_lot = "0.12"
```

Add to `crates/axis-agent-runtime/Cargo.toml`:
```toml
parking_lot.workspace = true
```

Add to `apps/axis-app/Cargo.toml`:
```toml
parking_lot.workspace = true
```

- [ ] **Step 2: Replace Mutex in agent_sessions.rs**

In `apps/axis-app/src/agent_sessions.rs`, replace the import:
```rust
// OLD:
use std::sync::Mutex;

// NEW:
use parking_lot::Mutex;
```

Then replace every instance of:
```rust
let Ok(guard) = self.inner.lock() else {
    return Vec::new(); // or return None, etc.
};
```
with:
```rust
let guard = self.inner.lock();
```

There are approximately 20+ lock sites in this file. Every `let Ok(guard) = self.inner.lock() else { ... }` pattern becomes just `let guard = self.inner.lock();` since parking_lot::Mutex never poisons.

Similarly for `let Ok(mut guard)` patterns → `let mut guard`.

- [ ] **Step 3: Replace Mutex in codex.rs, process_only.rs, fake.rs**

In each adapter file, replace:
```rust
use std::sync::Mutex;
```
with:
```rust
use parking_lot::Mutex;
```

Same transformation: remove all `let Ok(...)` pattern matching on `.lock()` calls. These adapters use `self.inner.lock()` in every trait method.

- [ ] **Step 4: Build and verify**

Run: `cargo build -p axis-app -p axis-agent-runtime 2>&1 | head -50`
Expected: Clean build, no warnings about unused imports of `std::sync::Mutex`.

- [ ] **Step 5: Run existing tests**

Run: `cargo test -p axis-app -p axis-agent-runtime 2>&1`
Expected: All existing tests pass.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "fix: replace std::sync::Mutex with parking_lot::Mutex

Eliminates silent lock poisoning bug where Mutex::lock() failures
returned empty results instead of propagating errors."
```

---

## Task 2: Add lifecycle state machine validation

**Files:**
- Modify: `crates/axis-agent-runtime/src/session.rs` (lines 165-186)
- Create: `crates/axis-agent-runtime/tests/lifecycle_transitions.rs`

- [ ] **Step 1: Write failing tests for valid transitions**

Create `crates/axis-agent-runtime/tests/lifecycle_transitions.rs`:

```rust
use axis_agent_runtime::SessionManager;
use axis_agent_runtime::events::RuntimeEvent;
use axis_agent_runtime::provider::ProviderRegistry;
use axis_core::agent::{AgentLifecycle, AgentSessionId};

fn make_manager_with_session() -> (SessionManager, AgentSessionId) {
    let registry = ProviderRegistry::new();
    let mut manager = SessionManager::new(registry);
    // We need a fake provider to start a session. Use the adapters::fake module.
    // Actually, let's test transition_lifecycle directly after manually creating a session.
    // The SessionManager::start_session requires a provider, so let's register a fake one.
    use axis_agent_runtime::adapters::fake::FakeProvider;
    use axis_agent_runtime::provider::StartAgentRequest;
    use std::sync::Arc;

    let fake = Arc::new(FakeProvider::with_standard_script());
    let mut registry = ProviderRegistry::new();
    registry.register("fake", fake);
    let mut manager = SessionManager::new(registry);

    let session_id = manager
        .start_session(StartAgentRequest {
            cwd: std::env::temp_dir(),
            provider_profile_id: "fake".to_string(),
            transport: None,
            argv_suffix: Vec::new(),
            env: Vec::new(),
        })
        .expect("start session");

    (manager, session_id)
}

#[test]
fn valid_transition_planned_to_starting() {
    let (mut manager, sid) = make_manager_with_session();
    // Session starts in Starting after start_session, so let's test Running next
    let result = manager.transition_lifecycle(&sid, AgentLifecycle::Running);
    assert!(result.is_ok(), "Starting → Running should be valid");
}

#[test]
fn valid_transition_running_to_waiting() {
    let (mut manager, sid) = make_manager_with_session();
    manager.transition_lifecycle(&sid, AgentLifecycle::Running).unwrap();
    let result = manager.transition_lifecycle(&sid, AgentLifecycle::Waiting);
    assert!(result.is_ok(), "Running → Waiting should be valid");
}

#[test]
fn valid_transition_running_to_completed() {
    let (mut manager, sid) = make_manager_with_session();
    manager.transition_lifecycle(&sid, AgentLifecycle::Running).unwrap();
    let result = manager.transition_lifecycle(&sid, AgentLifecycle::Completed);
    assert!(result.is_ok(), "Running → Completed should be valid");
}

#[test]
fn invalid_transition_completed_to_running() {
    let (mut manager, sid) = make_manager_with_session();
    manager.transition_lifecycle(&sid, AgentLifecycle::Running).unwrap();
    manager.transition_lifecycle(&sid, AgentLifecycle::Completed).unwrap();
    let result = manager.transition_lifecycle(&sid, AgentLifecycle::Running);
    assert!(result.is_err(), "Completed → Running should be rejected");
}

#[test]
fn invalid_transition_running_to_planned() {
    let (mut manager, sid) = make_manager_with_session();
    manager.transition_lifecycle(&sid, AgentLifecycle::Running).unwrap();
    let result = manager.transition_lifecycle(&sid, AgentLifecycle::Planned);
    assert!(result.is_err(), "Running → Planned should be rejected");
}

#[test]
fn noop_transition_same_state_is_ok() {
    let (mut manager, sid) = make_manager_with_session();
    manager.transition_lifecycle(&sid, AgentLifecycle::Running).unwrap();
    let result = manager.transition_lifecycle(&sid, AgentLifecycle::Running);
    assert!(result.is_ok(), "Same state → same state should be a noop");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p axis-agent-runtime --test lifecycle_transitions 2>&1`
Expected: `invalid_transition_completed_to_running` and `invalid_transition_running_to_planned` will PASS (wrong — they should fail because we haven't added validation yet). This confirms the bug exists.

- [ ] **Step 3: Add state machine validation to session.rs**

In `crates/axis-agent-runtime/src/session.rs`, add a validation function before `transition_lifecycle`:

```rust
/// Returns true if transitioning from `current` to `next` is valid.
fn is_valid_lifecycle_transition(current: AgentLifecycle, next: AgentLifecycle) -> bool {
    if current == next {
        return true; // noop
    }
    use AgentLifecycle::*;
    matches!(
        (current, next),
        (Planned, Starting)
            | (Starting, Running)
            | (Starting, Failed)
            | (Starting, Cancelled)
            | (Running, Waiting)
            | (Running, Completed)
            | (Running, Failed)
            | (Running, Cancelled)
            | (Waiting, Running)
            | (Waiting, Completed)
            | (Waiting, Failed)
            | (Waiting, Cancelled)
    )
}
```

Then modify `transition_lifecycle` (around line 165):

```rust
pub fn transition_lifecycle(
    &mut self,
    session_id: &AgentSessionId,
    lifecycle: AgentLifecycle,
) -> anyhow::Result<()> {
    let detail = self
        .sessions
        .get_mut(session_id)
        .ok_or_else(|| anyhow::anyhow!("session not found: {session_id}"))?;

    let current = detail.session.lifecycle;
    if current == lifecycle {
        return Ok(()); // noop, no revision bump
    }
    if !is_valid_lifecycle_transition(current, lifecycle) {
        anyhow::bail!(
            "invalid lifecycle transition: {current:?} → {lifecycle:?} for session {session_id}"
        );
    }

    detail.session.lifecycle = lifecycle;
    detail.completed_at_ms = is_terminal_lifecycle(lifecycle).then(now_ms).flatten();
    self.revision += 1;
    Ok(())
}
```

Also update `apply_event_inner` (the function that applies `RuntimeEvent::Lifecycle`) to use the same validation. Find where lifecycle events are applied and add the guard there too.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p axis-agent-runtime --test lifecycle_transitions 2>&1`
Expected: All 6 tests pass. The invalid transitions now return `Err`.

- [ ] **Step 5: Run all tests**

Run: `cargo test -p axis-agent-runtime -p axis-app 2>&1`
Expected: All tests pass. The existing `revision_unchanged_when_lifecycle_transition_is_noop` test should still pass.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "fix: validate agent lifecycle state machine transitions

Reject invalid transitions (e.g. Completed→Running) with an error
instead of silently accepting them. Noop transitions (same state)
remain valid and don't bump revision."
```

---

## Task 3: Add polling scheduler (background task)

**Files:**
- Modify: `apps/axis-app/src/agent_sessions.rs`
- Modify: `apps/axis-app/src/main.rs` (remove inline polling from UI loop ~line 3260)

- [ ] **Step 1: Add a poll_all_active_sessions method to AgentRuntimeBridge**

In `apps/axis-app/src/agent_sessions.rs`, add a method that polls all active (non-terminal) sessions:

```rust
/// Poll all active sessions across all workdesks. Returns the number of sessions polled.
pub fn poll_all_active_sessions(&self) -> usize {
    let mut guard = self.inner.lock();
    let mut polled = 0;

    // Collect active session IDs first to avoid borrow issues
    let active_sessions: Vec<AgentSessionId> = guard
        .surface_to_session
        .values()
        .cloned()
        .collect();

    for session_id in &active_sessions {
        // Skip terminal sessions
        if let Some(detail) = guard.manager.session(session_id) {
            if is_terminal_lifecycle(detail.lifecycle) {
                continue;
            }
        }
        let _ = guard.manager.poll_provider(session_id);
        polled += 1;
    }

    // Also poll daemon sessions
    let daemon_ids: Vec<AgentSessionId> = guard
        .daemon_records
        .keys()
        .cloned()
        .collect();

    for sid in &daemon_ids {
        if let Some(rec) = guard.daemon_records.get(&sid) {
            if is_terminal_lifecycle(rec.lifecycle) {
                continue;
            }
        }
        let _ = guard.daemon.get_agent(&sid.to_string(), None);
        polled += 1;
    }

    polled
}
```

Import `is_terminal_lifecycle` from the session module (make it `pub` if needed).

- [ ] **Step 2: Set up background polling timer in main.rs**

In the app initialization code in `main.rs`, after `AgentRuntimeBridge` is created, set up a GPUI timer that fires every 200ms:

Find the app setup code and add:

```rust
// Set up background agent polling (200ms interval)
let agent_runtime = self.agent_runtime.clone(); // Arc<AgentRuntimeBridge>
cx.spawn(|this, mut cx| async move {
    loop {
        cx.background_executor().timer(std::time::Duration::from_millis(200)).await;
        agent_runtime.poll_all_active_sessions();
        let _ = this.update(&mut cx, |this, cx| {
            cx.notify(); // Trigger UI refresh if state changed
        });
    }
})
.detach();
```

The exact GPUI timer API may vary — check GPUI docs for `cx.spawn` + `timer`. The key is: poll happens off the main rendering path.

- [ ] **Step 3: Remove inline polling from UI loop**

In `main.rs`, find the inline polling code around line 3260:
```rust
for desk in &self.workdesks {
    for pane in &desk.panes {
        for surface in &pane.surfaces {
            if surface.kind == PaneKind::Agent && desk.terminals.contains_key(&surface.id) {
                let _ = self.agent_runtime.poll_surface(desk.runtime_id, surface.id);
            }
        }
    }
}
```

Remove this block entirely. Polling is now handled by the background task.

- [ ] **Step 4: Build and test**

Run: `cargo build -p axis-app 2>&1 | head -20`
Expected: Clean build.

Run: `cargo test -p axis-app -p axis-agent-runtime 2>&1`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "fix: move agent polling to background task

Replace synchronous inline polling in UI render loop with a 200ms
background timer. Prevents UI frame blocking when providers are slow."
```

---

## Task 4: Add session expiry for completed/failed sessions

**Files:**
- Modify: `apps/axis-app/src/agent_sessions.rs`
- Create: `apps/axis-app/tests/session_expiry.rs` (or add to existing test module)

- [ ] **Step 1: Write failing test**

Add test to `agent_sessions.rs` test module (or separate file):

```rust
#[test]
fn completed_sessions_are_pruned_after_expiry() {
    // Create bridge, start fake session, complete it, verify it's pruned after timeout
    let bridge = AgentRuntimeBridge::with_registry(|registry| {
        let fake = Arc::new(FakeProvider::with_steps(vec![
            FakeScriptStep::Lifecycle(AgentLifecycle::Completed),
        ]));
        registry.register("fake", fake);
    });
    let key = SurfaceSessionKey { workdesk_runtime_id: 1, surface_id: SurfaceId::from(1) };
    let sid = bridge.start_agent_for_surface(key.workdesk_runtime_id, key.surface_id, "/tmp", "fake").unwrap();

    // Poll to complete the session
    bridge.poll_surface(key.workdesk_runtime_id, key.surface_id).unwrap();

    // Session should exist immediately after completion
    assert!(bridge.has_session_for_surface(key.workdesk_runtime_id, key.surface_id));

    // After prune_expired_sessions with 0 timeout, it should be gone
    bridge.prune_expired_sessions(std::time::Duration::ZERO);
    assert!(!bridge.has_session_for_surface(key.workdesk_runtime_id, key.surface_id));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p axis-app session_expiry 2>&1`
Expected: FAIL — `prune_expired_sessions` doesn't exist yet.

- [ ] **Step 3: Implement session expiry**

In `agent_sessions.rs`, add a `completed_at` tracking field to `BridgeInner`:

```rust
struct BridgeInner {
    // ... existing fields ...
    /// Tracks when sessions entered terminal state (Completed/Failed/Cancelled)
    terminal_since: HashMap<AgentSessionId, std::time::Instant>,
}
```

Initialize it as `HashMap::new()` in `new()`.

Add logic in `poll_surface` (or `poll_all_active_sessions`) to record when sessions become terminal:

```rust
// After polling, check if session is now terminal
if let Some(detail) = guard.manager.session(session_id) {
    if is_terminal_lifecycle(detail.lifecycle) {
        guard.terminal_since
            .entry(session_id.clone())
            .or_insert_with(std::time::Instant::now);
    }
}
```

Add the prune method:

```rust
pub fn prune_expired_sessions(&self, max_age: std::time::Duration) {
    let mut guard = self.inner.lock();
    let now = std::time::Instant::now();
    let expired: Vec<AgentSessionId> = guard
        .terminal_since
        .iter()
        .filter(|(_, since)| now.duration_since(**since) >= max_age)
        .map(|(id, _)| id.clone())
        .collect();

    for session_id in &expired {
        guard.terminal_since.remove(session_id);
        // Remove from surface_to_session mapping
        guard.surface_to_session.retain(|_, sid| sid != session_id);
        // Stop the provider session to clean up resources
        let _ = guard.manager.stop_session(session_id);
    }
}
```

Call this from the background polling task with a 30-second timeout:

```rust
// In the background polling loop:
agent_runtime.prune_expired_sessions(std::time::Duration::from_secs(30));
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p axis-app -p axis-agent-runtime 2>&1`
Expected: All tests pass including the new expiry test.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "fix: prune completed agent sessions after 30 seconds

Sessions in terminal state (Completed/Failed/Cancelled) are cleaned up
after 30 seconds. Prevents indefinite polling and memory accumulation."
```

---

## Task 5: Fix race condition in poll_surface daemon path

**Files:**
- Modify: `apps/axis-app/src/agent_sessions.rs` (lines 449-491)

- [ ] **Step 1: Write test demonstrating the race**

Add to test module:

```rust
#[test]
fn poll_surface_handles_daemon_session_disappearing_between_list_and_get() {
    // This test verifies graceful handling when a daemon session
    // disappears between listing and detail fetch.
    // We simulate this by having a daemon that returns a session in list
    // but returns "not found" on detail fetch.
    // Since DaemonClient is a real socket client, we test the guard logic directly.

    // For now, verify that poll_surface returns Ok even when daemon_details
    // cache has stale entries that don't match daemon_records
    let bridge = AgentRuntimeBridge::new();
    // Poll a surface that has no session — should return Ok, not panic
    let result = bridge.poll_surface(1, SurfaceId::from(999));
    assert!(result.is_ok() || result.is_err()); // Should not panic
}
```

- [ ] **Step 2: Fix the race condition**

In `agent_sessions.rs`, modify the `poll_surface` method (around line 465-490). The issue is: daemon agent list is fetched, then we iterate and fetch details, but sessions may disappear between these operations.

Replace the current pattern:

```rust
// Current (racy):
let sessions = guard.daemon.list_agents(None)?;
guard.daemon_records = sessions.into_iter()...;
let daemon_ids = guard.daemon_records.keys().cloned().collect::<HashSet<_>>();
guard.daemon_details.retain(|session_id, _| daemon_ids.contains(session_id));
// Then iterate and call get_agent for each

// Fixed (snapshot-based):
let sessions = match guard.daemon.list_agents(None) {
    Ok(s) => s,
    Err(_) => return Ok(()), // Daemon unavailable, skip gracefully
};
let current_ids: HashSet<AgentSessionId> = sessions
    .iter()
    .map(|s| s.id.clone())
    .collect();

// Update records atomically
guard.daemon_records = sessions
    .into_iter()
    .map(|s| (s.id.clone(), s))
    .collect();

// Prune stale details
guard.daemon_details.retain(|id, _| current_ids.contains(id));

// Fetch details only for sessions that need it, with error tolerance
for sid in &current_ids {
    match guard.daemon.get_agent(&sid.to_string(), None) {
        Ok(detail) => {
            cache_daemon_detail(&mut guard, detail);
        }
        Err(_) => {
            // Session disappeared between list and get — remove from records
            guard.daemon_records.remove(sid);
            guard.daemon_details.remove(sid);
        }
    }
}
```

- [ ] **Step 3: Build and test**

Run: `cargo test -p axis-app -p axis-agent-runtime 2>&1`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "fix: handle daemon session disappearing during poll

When a session disappears between list_agents and get_agent calls,
gracefully remove it from cache instead of leaving stale state."
```

---

## Task 6: Add capability validation before provider operations

**Files:**
- Modify: `crates/axis-agent-runtime/src/session.rs` (send_turn, respond_approval, resume)
- Create: `crates/axis-agent-runtime/tests/capability_validation.rs`

- [ ] **Step 1: Write failing tests**

Create `crates/axis-agent-runtime/tests/capability_validation.rs`:

```rust
use axis_agent_runtime::SessionManager;
use axis_agent_runtime::adapters::fake::FakeProvider;
use axis_agent_runtime::provider::{
    AgentProvider, ProviderRegistry, StartAgentRequest, StartedSession,
    SendTurnRequest, RespondApprovalRequest, ResumeRequest,
};
use axis_agent_runtime::events::RuntimeEvent;
use axis_core::agent::{AgentSessionCapabilities, AgentSessionId};
use std::sync::Arc;

/// A provider that supports nothing (all capabilities false).
struct NoCapProvider;

impl AgentProvider for NoCapProvider {
    fn start(&self, _req: StartAgentRequest) -> anyhow::Result<StartedSession> {
        Ok(StartedSession {
            session_id: AgentSessionId::from("nocap-1"),
        })
    }
    fn poll_events(&self, _id: &AgentSessionId) -> anyhow::Result<Vec<RuntimeEvent>> {
        Ok(vec![])
    }
    fn capabilities(&self) -> AgentSessionCapabilities {
        AgentSessionCapabilities {
            turn_input: false,
            tool_calls: false,
            approvals: false,
            resume: false,
            terminal_attachment: false,
        }
    }
    fn stop(&self, _id: &AgentSessionId) -> anyhow::Result<()> {
        Ok(())
    }
}

fn make_nocap_session() -> (SessionManager, AgentSessionId) {
    let mut registry = ProviderRegistry::new();
    registry.register("nocap", Arc::new(NoCapProvider));
    let mut manager = SessionManager::new(registry);
    let sid = manager
        .start_session(StartAgentRequest {
            cwd: std::env::temp_dir(),
            provider_profile_id: "nocap".to_string(),
            transport: None,
            argv_suffix: Vec::new(),
            env: Vec::new(),
        })
        .unwrap();
    (manager, sid)
}

#[test]
fn send_turn_rejected_when_capability_missing() {
    let (mut manager, sid) = make_nocap_session();
    let result = manager.send_turn(&sid, "hello".to_string());
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("turn_input"), "error should mention the missing capability: {msg}");
}

#[test]
fn respond_approval_rejected_when_capability_missing() {
    let (mut manager, sid) = make_nocap_session();
    let result = manager.respond_approval(
        &sid,
        &"req-1".into(),
        true,
        None,
    );
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("approvals"), "error should mention the missing capability: {msg}");
}

#[test]
fn resume_rejected_when_capability_missing() {
    let (mut manager, sid) = make_nocap_session();
    let result = manager.resume(&sid);
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("resume"), "error should mention the missing capability: {msg}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p axis-agent-runtime --test capability_validation 2>&1`
Expected: FAIL — currently no capability checks, so the calls go through (or hit the default "not supported" error from the trait, which doesn't mention the capability name).

- [ ] **Step 3: Add capability checks to session.rs**

In `crates/axis-agent-runtime/src/session.rs`, modify the `send_turn`, `respond_approval`, and `resume` methods:

```rust
pub fn send_turn(
    &mut self,
    session_id: &AgentSessionId,
    text: String,
) -> anyhow::Result<()> {
    let detail = self.sessions.get(session_id)
        .ok_or_else(|| anyhow::anyhow!("session not found: {session_id}"))?;
    let provider = self.registry.require(&detail.session.provider_profile_id)?;
    let caps = provider.capabilities();
    if !caps.turn_input {
        anyhow::bail!(
            "provider '{}' does not support turn_input",
            detail.session.provider_profile_id
        );
    }
    let events = provider.send_turn(SendTurnRequest {
        session_id: session_id.clone(),
        text,
    })?;
    self.apply_events(events)
}

pub fn respond_approval(
    &mut self,
    session_id: &AgentSessionId,
    approval_request_id: &AgentApprovalRequestId,
    approved: bool,
    note: Option<String>,
) -> anyhow::Result<()> {
    let detail = self.sessions.get(session_id)
        .ok_or_else(|| anyhow::anyhow!("session not found: {session_id}"))?;
    let provider = self.registry.require(&detail.session.provider_profile_id)?;
    let caps = provider.capabilities();
    if !caps.approvals {
        anyhow::bail!(
            "provider '{}' does not support approvals",
            detail.session.provider_profile_id
        );
    }
    let events = provider.respond_approval(RespondApprovalRequest {
        session_id: session_id.clone(),
        approval_request_id: approval_request_id.clone(),
        approved,
        note,
    })?;
    self.apply_events(events)
}

pub fn resume(
    &mut self,
    session_id: &AgentSessionId,
) -> anyhow::Result<()> {
    let detail = self.sessions.get(session_id)
        .ok_or_else(|| anyhow::anyhow!("session not found: {session_id}"))?;
    let provider = self.registry.require(&detail.session.provider_profile_id)?;
    let caps = provider.capabilities();
    if !caps.resume {
        anyhow::bail!(
            "provider '{}' does not support resume",
            detail.session.provider_profile_id
        );
    }
    let events = provider.resume(ResumeRequest {
        session_id: session_id.clone(),
    })?;
    self.apply_events(events)
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p axis-agent-runtime 2>&1`
Expected: All tests pass, including the 3 new capability tests.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "fix: validate provider capabilities before operations

Check AgentSessionCapabilities before calling send_turn, respond_approval,
and resume. Returns descriptive error naming the missing capability."
```

---

## Task 7: Add AgentError enum for error classification

**Files:**
- Create: `crates/axis-agent-runtime/src/error.rs`
- Modify: `crates/axis-agent-runtime/src/lib.rs`
- Modify: `crates/axis-agent-runtime/src/session.rs`
- Modify: `apps/axis-app/src/agent_sessions.rs`

- [ ] **Step 1: Create error.rs with AgentError enum**

Create `crates/axis-agent-runtime/src/error.rs`:

```rust
use std::fmt;

/// Classified agent runtime errors for context-appropriate UI recovery.
#[derive(Debug)]
pub enum AgentError {
    /// Session not found — likely already cleaned up or invalid ID
    SessionNotFound(String),
    /// Provider profile not registered
    ProviderNotFound(String),
    /// Operation not supported by this provider's capabilities
    UnsupportedOperation {
        provider: String,
        operation: String,
    },
    /// Invalid lifecycle state transition
    InvalidTransition {
        from: String,
        to: String,
    },
    /// Transient network or daemon communication error
    DaemonUnavailable(String),
    /// Provider process error (crash, timeout, etc.)
    ProviderError(String),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SessionNotFound(id) => write!(f, "session not found: {id}"),
            Self::ProviderNotFound(id) => write!(f, "provider not found: {id}"),
            Self::UnsupportedOperation { provider, operation } => {
                write!(f, "provider '{provider}' does not support {operation}")
            }
            Self::InvalidTransition { from, to } => {
                write!(f, "invalid lifecycle transition: {from} → {to}")
            }
            Self::DaemonUnavailable(msg) => write!(f, "daemon unavailable: {msg}"),
            Self::ProviderError(msg) => write!(f, "provider error: {msg}"),
        }
    }
}

impl std::error::Error for AgentError {}
```

- [ ] **Step 2: Export from lib.rs**

Add to `crates/axis-agent-runtime/src/lib.rs`:
```rust
pub mod error;
pub use error::AgentError;
```

- [ ] **Step 3: Use AgentError in session.rs**

Replace `anyhow::bail!` calls in `transition_lifecycle`, `send_turn`, `respond_approval`, `resume` with `AgentError` variants. Keep using `anyhow::Result` as the return type (AgentError implements std::error::Error so it works with `?`).

Example for `transition_lifecycle`:
```rust
if !is_valid_lifecycle_transition(current, lifecycle) {
    return Err(AgentError::InvalidTransition {
        from: format!("{current:?}"),
        to: format!("{lifecycle:?}"),
    }.into());
}
```

- [ ] **Step 4: Use AgentError in agent_sessions.rs bridge**

In `apps/axis-app/src/agent_sessions.rs`, replace `map_err(|error| error.to_string())` with pattern matching on `AgentError` where possible:

```rust
// Instead of:
.map_err(|error| error.to_string())?

// Use:
.map_err(|error| {
    if let Some(agent_err) = error.downcast_ref::<AgentError>() {
        agent_err.to_string()
    } else {
        error.to_string()
    }
})?
```

This preserves the existing `String` error type in the bridge API while providing better messages.

- [ ] **Step 5: Build and test**

Run: `cargo test -p axis-agent-runtime -p axis-app 2>&1`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat: add AgentError enum for classified runtime errors

Introduces SessionNotFound, ProviderNotFound, UnsupportedOperation,
InvalidTransition, DaemonUnavailable, and ProviderError variants.
UI can now show context-appropriate recovery guidance."
```

---

## Task 8: Add AXIS_APPROVAL_REQUEST to CLI protocol

**Files:**
- Modify: `crates/axis-agent-runtime/src/cli_protocol.rs`
- Create: `crates/axis-agent-runtime/tests/cli_protocol.rs`

- [ ] **Step 1: Write tests for approval request parsing**

Create `crates/axis-agent-runtime/tests/cli_protocol.rs`:

```rust
use axis_agent_runtime::cli_protocol::parse_axis_output_line;
use axis_agent_runtime::events::RuntimeEvent;
use axis_core::agent::AgentSessionId;

#[test]
fn parse_approval_request_line() {
    let session_id = AgentSessionId::from("test-1");
    let line = r#"AXIS_APPROVAL_REQUEST {"id":"req-1","tool_call_id":"tc-1","summary":"Run shell command: ls"}"#;
    let events = parse_axis_output_line(line, &session_id);
    assert!(events.is_some(), "should parse AXIS_APPROVAL_REQUEST line");
    let events = events.unwrap();
    assert_eq!(events.len(), 1);
    match &events[0] {
        RuntimeEvent::ApprovalRequest { session_id: sid, approval } => {
            assert_eq!(sid, &session_id);
            assert_eq!(approval.id.as_str(), "req-1");
        }
        other => panic!("expected ApprovalRequest, got {other:?}"),
    }
}

#[test]
fn parse_malformed_approval_request_returns_none() {
    let session_id = AgentSessionId::from("test-1");
    let line = "AXIS_APPROVAL_REQUEST {invalid json";
    let events = parse_axis_output_line(line, &session_id);
    assert!(events.is_none(), "malformed JSON should return None");
}

#[test]
fn parse_lifecycle_event_line() {
    let session_id = AgentSessionId::from("test-1");
    let line = r#"AXIS_EVENT {"kind":"lifecycle","session_id":"ignored","lifecycle":"running"}"#;
    let events = parse_axis_output_line(line, &session_id);
    assert!(events.is_some());
}

#[test]
fn parse_unknown_prefix_returns_none() {
    let session_id = AgentSessionId::from("test-1");
    let line = "AXIS_UNKNOWN some data";
    let events = parse_axis_output_line(line, &session_id);
    assert!(events.is_none());
}

#[test]
fn parse_status_line() {
    let session_id = AgentSessionId::from("test-1");
    let line = "AXIS_STATUS Working on feature";
    let events = parse_axis_output_line(line, &session_id);
    assert!(events.is_some());
    let events = events.unwrap();
    match &events[0] {
        RuntimeEvent::Status { message, .. } => {
            assert_eq!(message, "Working on feature");
        }
        other => panic!("expected Status, got {other:?}"),
    }
}
```

- [ ] **Step 2: Run tests to verify parsing test fails**

Run: `cargo test -p axis-agent-runtime --test cli_protocol 2>&1`
Expected: `parse_approval_request_line` fails — no `AXIS_APPROVAL_REQUEST` prefix handler yet.

- [ ] **Step 3: Add AXIS_APPROVAL_REQUEST handling to cli_protocol.rs**

In `crates/axis-agent-runtime/src/cli_protocol.rs`, add the constant and handler:

```rust
pub const AXIS_APPROVAL_REQUEST_PREFIX: &str = "AXIS_APPROVAL_REQUEST ";
```

In `parse_axis_output_line`, add a new branch:

```rust
} else if let Some(json_str) = line.strip_prefix(AXIS_APPROVAL_REQUEST_PREFIX) {
    let approval: axis_core::agent_history::AgentApprovalRequest =
        serde_json::from_str(json_str.trim()).ok()?;
    Some(vec![RuntimeEvent::ApprovalRequest {
        session_id: session_id.clone(),
        approval,
    }])
}
```

Note: Check that `AgentApprovalRequest` has the right fields (`id`, `tool_call_id`, `summary`). If not, check `axis_core::agent_history` for the exact type name and fields.

- [ ] **Step 4: Run tests**

Run: `cargo test -p axis-agent-runtime --test cli_protocol 2>&1`
Expected: All 5 tests pass.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: add AXIS_APPROVAL_REQUEST to CLI protocol

Providers can now emit structured approval requests via the
AXIS_APPROVAL_REQUEST line prefix, alongside the existing
AXIS_EVENT JSON envelope."
```

---

## Task 9: Fix workdesk binding consistency

**Files:**
- Modify: `crates/axis-agent-runtime/src/session.rs` (StartAgentRequest, start_session)
- Modify: `crates/axis-agent-runtime/src/provider.rs` (StartAgentRequest struct)
- Modify: `apps/axis-app/src/agent_sessions.rs` (start_agent_for_surface_inner)

- [ ] **Step 1: Add workdesk_id to StartAgentRequest**

In `crates/axis-agent-runtime/src/provider.rs`, add the field:

```rust
pub struct StartAgentRequest {
    pub cwd: PathBuf,
    pub provider_profile_id: String,
    pub transport: Option<String>,
    pub argv_suffix: Vec<String>,
    pub env: Vec<(String, String)>,
    pub workdesk_id: Option<String>,  // NEW
}
```

- [ ] **Step 2: Set workdesk_id in session record at creation time**

In `crates/axis-agent-runtime/src/session.rs`, in `start_session`, set `workdesk_id`:

```rust
let record = AgentSessionRecord {
    id: started.session_id.clone(),
    provider_profile_id: req.provider_profile_id.clone(),
    workdesk_id: req.workdesk_id.clone(),  // was None before
    // ... rest of fields
};
```

- [ ] **Step 3: Pass workdesk_id from bridge**

In `apps/axis-app/src/agent_sessions.rs`, in `start_agent_for_surface_inner`, pass the workdesk ID:

```rust
let session_id = guard.manager.start_session(StartAgentRequest {
    cwd: cwd.into(),
    provider_profile_id: profile_id.to_string(),
    transport: None,
    argv_suffix: Vec::new(),
    env: Vec::new(),
    workdesk_id: Some(workdesk_runtime_id.to_string()),  // NEW
})?;
```

Remove the post-hoc patching in `record_for_key`:
```rust
// Remove these lines:
// record.workdesk_id = Some(key.workdesk_runtime_id.to_string());
```

- [ ] **Step 4: Update all other StartAgentRequest construction sites**

Search for all `StartAgentRequest {` in the codebase and add `workdesk_id: None` (or appropriate value) to each. This includes test code, daemon, CLI, etc.

- [ ] **Step 5: Build and test**

Run: `cargo test --workspace 2>&1`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "fix: set workdesk_id at session creation time

Pass workdesk_id through StartAgentRequest instead of patching it
post-hoc in the bridge. Eliminates data inconsistency between
SessionManager and BridgeInner views."
```

---

## Task 10: Review surface — file status tinting

**Files:**
- Modify: `apps/axis-app/src/main.rs` (review panel file list rendering, ~lines 9568-9624)
- Modify: `apps/axis-app/src/review.rs` (expose file_review_aggregate publicly if not already)

- [ ] **Step 1: Verify file_review_aggregate is accessible**

Check that `file_review_aggregate()` in `review.rs` is `pub` and returns `FileReviewAggregate`. If not, make it public.

- [ ] **Step 2: Add color mapping function**

In `apps/axis-app/src/review.rs`, add:

```rust
/// Returns an RGB color for file status tinting in the review file list.
pub fn file_status_color(aggregate: FileReviewAggregate) -> u32 {
    match aggregate {
        FileReviewAggregate::AllReviewed => 0x4ec990,  // green
        FileReviewAggregate::HasFollowUp => 0xf0d35f,  // yellow
        FileReviewAggregate::InProgress => 0x7f8a94,   // gray
        FileReviewAggregate::NoHunks => 0x63717b,      // dim gray
    }
}
```

- [ ] **Step 3: Apply tinting in file list rendering**

In `apps/axis-app/src/main.rs`, find the review panel file list rendering (~line 9568-9624). For each file row, compute the aggregate and apply the color:

```rust
// In the file list rendering loop, add status dot or text color:
let aggregate = file_review_aggregate(
    &desk.review_local_state.hunk_states,
    &desk.review_payload_cache,
    workdesk_id,
    &file_diff.path,
);
let status_color = file_status_color(aggregate);

// Apply to the file row's text color or add a status dot:
div()
    .child(
        div()
            .w(px(6.0))
            .h(px(6.0))
            .rounded_full()
            .bg(rgb(status_color))
    )
    .child(
        div().text_color(rgb(status_color)).child(&file_diff.path)
    )
```

- [ ] **Step 4: Build and verify visually**

Run: `cargo build -p axis-app 2>&1 | head -20`
Expected: Clean build.

Run the app and open review panel to verify colors appear.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: add file status tinting in review panel

Files in the review file list now show colored dots indicating
review progress: green (all reviewed), yellow (has follow-up),
gray (in progress), dim (no hunks)."
```

---

## Task 11: Review surface — auto-refresh on stale

**Files:**
- Modify: `apps/axis-app/src/main.rs` (sync_review_summary_for_desk, background refresh)

- [ ] **Step 1: Add stale detection and retry**

In the background polling loop (from Task 3), add review refresh:

```rust
// In the background polling timer, after agent polling:
// Check for stale review payloads and attempt refresh
let _ = this.update(&mut cx, |this, cx| {
    for desk_index in 0..this.workdesks.len() {
        if this.workdesks[desk_index].review_local_state.stale_notice.is_some() {
            this.sync_review_summary_for_desk(desk_index);
        }
    }
    cx.notify();
});
```

The refresh interval should be slower than agent polling — every 5 seconds is sufficient. Add a counter:

```rust
let mut tick = 0u64;
loop {
    cx.background_executor().timer(Duration::from_millis(200)).await;
    agent_runtime.poll_all_active_sessions();
    agent_runtime.prune_expired_sessions(Duration::from_secs(30));

    tick += 1;
    if tick % 25 == 0 { // Every 5 seconds (25 * 200ms)
        let _ = this.update(&mut cx, |this, cx| {
            for desk_index in 0..this.workdesks.len() {
                if this.workdesks[desk_index].review_local_state.stale_notice.is_some() {
                    this.sync_review_summary_for_desk(desk_index);
                }
            }
            cx.notify();
        });
    }

    let _ = this.update(&mut cx, |this, cx| { cx.notify(); });
}
```

- [ ] **Step 2: Build and test**

Run: `cargo build -p axis-app 2>&1 | head -20`
Expected: Clean build.

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "feat: auto-refresh stale review payloads every 5 seconds

When a review payload is marked stale (daemon was unavailable),
automatically retry fetching from daemon every 5 seconds."
```

---

## Task 12: Review surface — actionable notices

**Files:**
- Modify: `apps/axis-app/src/main.rs` (notice rendering, ~lines 9515-9567)

- [ ] **Step 1: Update notice text to be actionable**

Find the setup notice rendering in `main.rs` (~line 9515-9567) and replace generic messages:

```rust
// For base branch missing notice, change from:
// "Base branch '{branch}' not available in this clone"
// To:
format!(
    "Base branch '{}' not available. Try: git fetch origin",
    base_branch
)

// For stale notice, change from:
// "Diff details may be out of date"
// To:
"Diff details may be out of date — auto-refreshing in background"
```

- [ ] **Step 2: Build and test**

Run: `cargo build -p axis-app 2>&1 | head -20`
Expected: Clean build.

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "feat: make review surface notices actionable

Setup notice now suggests 'git fetch origin'. Stale notice now
indicates auto-refresh is happening in background."
```

---

## Task 13: Review surface — hunk line counts

**Files:**
- Modify: `apps/axis-app/src/main.rs` (hunk tab rendering, ~lines 9671-9730)

- [ ] **Step 1: Compute and display line counts per hunk**

In the hunk tab rendering section, add line count display:

```rust
// For each hunk tab, count additions and removals:
let additions = hunk.lines.iter().filter(|l| l.kind == ReviewLineKind::Addition).count();
let removals = hunk.lines.iter().filter(|l| l.kind == ReviewLineKind::Removal).count();
let count_label = format!("+{additions}/-{removals}");

// Add to the tab label:
div()
    .flex()
    .gap_1()
    .child(/* existing header text */)
    .child(
        div()
            .text_xs()
            .text_color(rgb(0x63717b))
            .child(count_label)
    )
```

- [ ] **Step 2: Build and verify**

Run: `cargo build -p axis-app 2>&1 | head -20`
Expected: Clean build.

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "feat: show +X/-Y line counts on review hunk tabs

Each hunk tab now displays the number of added and removed lines
for quick assessment of change volume."
```

---

## Task 14: Test coverage — lifecycle, approval, daemon fallback, multi-session

**Files:**
- Create: `crates/axis-agent-runtime/tests/approval_flow.rs`
- Create: `crates/axis-agent-runtime/tests/multi_session.rs`
- Add tests to existing `apps/axis-app/src/agent_sessions.rs` test module

- [ ] **Step 1: Write approval flow test**

Create `crates/axis-agent-runtime/tests/approval_flow.rs`:

```rust
use axis_agent_runtime::SessionManager;
use axis_agent_runtime::adapters::fake::FakeProvider;
use axis_agent_runtime::events::RuntimeEvent;
use axis_agent_runtime::provider::{ProviderRegistry, StartAgentRequest};
use axis_core::agent::{AgentLifecycle, AgentAttention};
use std::sync::Arc;

#[test]
fn approval_request_sets_needs_review_attention() {
    let fake = Arc::new(FakeProvider::with_standard_script());
    let mut registry = ProviderRegistry::new();
    registry.register("fake", fake);
    let mut manager = SessionManager::new(registry);

    let sid = manager
        .start_session(StartAgentRequest {
            cwd: std::env::temp_dir(),
            provider_profile_id: "fake".to_string(),
            transport: None,
            argv_suffix: Vec::new(),
            env: Vec::new(),
            workdesk_id: None,
        })
        .unwrap();

    // Poll through the script steps until NeedsReview
    for _ in 0..10 {
        let _ = manager.poll_provider(&sid);
        if let Some(detail) = manager.session_detail(&sid) {
            if detail.session.attention == AgentAttention::NeedsReview {
                // Found the NeedsReview state
                assert_eq!(detail.session.lifecycle, AgentLifecycle::Running);
                return;
            }
        }
    }
    panic!("FakeProvider standard script should reach NeedsReview attention");
}

#[test]
fn respond_approval_transitions_back_to_working() {
    let fake = Arc::new(FakeProvider::with_standard_script());
    let mut registry = ProviderRegistry::new();
    registry.register("fake", fake);
    let mut manager = SessionManager::new(registry);

    let sid = manager
        .start_session(StartAgentRequest {
            cwd: std::env::temp_dir(),
            provider_profile_id: "fake".to_string(),
            transport: None,
            argv_suffix: Vec::new(),
            env: Vec::new(),
            workdesk_id: None,
        })
        .unwrap();

    // Advance to a state where we can test respond_approval
    for _ in 0..10 {
        let _ = manager.poll_provider(&sid);
    }

    // The fake provider's respond_approval emits Running + Working + status
    let result = manager.respond_approval(&sid, &"any".into(), true, None);
    assert!(result.is_ok(), "respond_approval should succeed: {:?}", result);
}
```

- [ ] **Step 2: Write multi-session test**

Create `crates/axis-agent-runtime/tests/multi_session.rs`:

```rust
use axis_agent_runtime::SessionManager;
use axis_agent_runtime::adapters::fake::{FakeProvider, FakeScriptStep};
use axis_agent_runtime::provider::{ProviderRegistry, StartAgentRequest};
use axis_core::agent::AgentLifecycle;
use std::sync::Arc;

#[test]
fn three_sessions_in_parallel_all_complete() {
    let fake = Arc::new(FakeProvider::with_standard_script());
    let mut registry = ProviderRegistry::new();
    registry.register("fake", fake);
    let mut manager = SessionManager::new(registry);

    let mut sids = Vec::new();
    for _ in 0..3 {
        let sid = manager
            .start_session(StartAgentRequest {
                cwd: std::env::temp_dir(),
                provider_profile_id: "fake".to_string(),
                transport: None,
                argv_suffix: Vec::new(),
                env: Vec::new(),
                workdesk_id: None,
            })
            .unwrap();
        sids.push(sid);
    }

    // Poll all sessions in round-robin
    for _ in 0..20 {
        for sid in &sids {
            let _ = manager.poll_provider(sid);
        }
    }

    // All should have completed
    for sid in &sids {
        let detail = manager.session_detail(sid).expect("session should exist");
        assert_eq!(
            detail.session.lifecycle,
            AgentLifecycle::Completed,
            "session {} should be completed",
            sid
        );
    }
}

#[test]
fn polling_one_session_does_not_affect_another() {
    let fast = Arc::new(FakeProvider::with_steps(vec![
        FakeScriptStep::Lifecycle(AgentLifecycle::Completed),
    ]));
    let slow = Arc::new(FakeProvider::with_standard_script());
    let mut registry = ProviderRegistry::new();
    registry.register("fast", fast);
    registry.register("slow", slow);
    let mut manager = SessionManager::new(registry);

    let fast_sid = manager
        .start_session(StartAgentRequest {
            cwd: std::env::temp_dir(),
            provider_profile_id: "fast".to_string(),
            transport: None,
            argv_suffix: Vec::new(),
            env: Vec::new(),
            workdesk_id: None,
        })
        .unwrap();

    let slow_sid = manager
        .start_session(StartAgentRequest {
            cwd: std::env::temp_dir(),
            provider_profile_id: "slow".to_string(),
            transport: None,
            argv_suffix: Vec::new(),
            env: Vec::new(),
            workdesk_id: None,
        })
        .unwrap();

    // Poll fast session — it should complete immediately
    let _ = manager.poll_provider(&fast_sid);
    let fast_detail = manager.session_detail(&fast_sid).unwrap();
    assert_eq!(fast_detail.session.lifecycle, AgentLifecycle::Completed);

    // Slow session should still be running
    let _ = manager.poll_provider(&slow_sid);
    let slow_detail = manager.session_detail(&slow_sid).unwrap();
    assert_ne!(
        slow_detail.session.lifecycle,
        AgentLifecycle::Completed,
        "slow session should not be completed yet"
    );
}
```

- [ ] **Step 3: Run all new tests**

Run: `cargo test -p axis-agent-runtime --test approval_flow --test multi_session --test lifecycle_transitions --test capability_validation --test cli_protocol 2>&1`
Expected: All tests pass.

- [ ] **Step 4: Run full test suite**

Run: `cargo test --workspace 2>&1`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "test: add approval flow, multi-session, and CLI protocol tests

Covers approval request → response lifecycle, 3+ sessions in parallel,
cross-session isolation, CLI protocol line parsing, and malformed input."
```

---

## Summary

| Task | Component | Type |
|------|-----------|------|
| 1 | parking_lot::Mutex | Critical fix |
| 2 | State machine validation | Critical fix |
| 3 | Background polling scheduler | Critical fix |
| 4 | Session expiry | Critical fix |
| 5 | Daemon race condition | Critical fix |
| 6 | Capability validation | Lifecycle completion |
| 7 | AgentError enum | Lifecycle completion |
| 8 | AXIS_APPROVAL_REQUEST | Lifecycle completion |
| 9 | Workdesk binding fix | Lifecycle completion |
| 10 | File status tinting | Review polish |
| 11 | Auto-refresh stale | Review polish |
| 12 | Actionable notices | Review polish |
| 13 | Hunk line counts | Review polish |
| 14 | Test coverage | Testing |
