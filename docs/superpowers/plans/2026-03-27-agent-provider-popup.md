# Agent Provider Popup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a custom in-app popup that appears before every explicit UI-driven agent launch, lists all registered agent providers, disables unavailable ones, and starts the selected available provider instead of silently using the default profile.

**Architecture:** Keep provider launch availability in the runtime/binary-resolution layer, expose a stable UI-facing provider option snapshot from `AgentRuntimeBridge`, and keep popup interaction state in a focused app module. In `axis-app`, intercept the two explicit UI launch paths (`SpawnAgentPane` and stack-menu `Agent`), open a centered modal overlay, then continue launch through an explicit "spawn agent with profile" path that preserves the existing runtime-backed terminal/session wiring.

**Tech Stack:** Rust 2021, GPUI, existing `axis-agent-runtime` bin resolver + session bridge, existing `AxisShell` custom menu/overlay patterns, existing `FakeProvider` test adapter.

---

## Planned File Structure

### Runtime availability plumbing

- Modify: `crates/axis-agent-runtime/src/bin_resolver.rs`
  Add a public resolution result that reports both the argv to use and whether the configured/default binary is locally available.
- Modify: `crates/axis-agent-runtime/src/lib.rs`
  Re-export the new public resolution type and helper for `axis-app`.

### App runtime bridge

- Modify: `apps/axis-app/src/agent_sessions.rs`
  Add a UI-facing provider option snapshot, populate it from runtime resolution data, and expose it through `AgentRuntimeBridge`.

### Popup state

- Create: `apps/axis-app/src/agent_provider_popup.rs`
  Hold popup state, launch target state, and pure selection helpers that are easy to test without rendering.

### App integration and rendering

- Modify: `apps/axis-app/src/main.rs`
  Add popup state to `AxisShell`, inject runtime in tests, intercept explicit UI launch actions, render the centered overlay, add dismiss/select handlers, and route selection into an explicit "spawn agent with profile" path.

### Tests

- Test in: `crates/axis-agent-runtime/src/bin_resolver.rs`
  Cover local binary availability and unavailable reasons.
- Test in: `apps/axis-app/src/agent_sessions.rs`
  Cover bridge provider option snapshots.
- Test in: `apps/axis-app/src/agent_provider_popup.rs`
  Cover popup selection and empty-state logic.
- Test in: `apps/axis-app/src/main.rs`
  Cover shortcut/menu interception, explicit selected-profile launch, and popup dismissal side effects.

## Scope Locks

These decisions are locked before implementation starts:

1. The popup is custom GPUI UI, not a system dialog.
2. `SpawnAgentPane` and stack-menu `Agent` are intercepted; template-created desks are unchanged in this slice.
3. All registered providers remain visible in the popup.
4. Unavailable providers are disabled, not hidden.
5. The selected profile is used through the existing runtime-backed agent surface flow; CLI/socket automation is unchanged.

## Task 1: Runtime Provider Availability Snapshots

**Files:**
- Modify: `crates/axis-agent-runtime/src/bin_resolver.rs`
- Modify: `crates/axis-agent-runtime/src/lib.rs`
- Modify: `apps/axis-app/src/agent_sessions.rs`

- [ ] **Step 1: Write the failing runtime-resolution tests in `crates/axis-agent-runtime/src/bin_resolver.rs`**

```rust
#[test]
fn provider_command_marks_missing_env_override_unavailable() {
    let env_name = "AXIS_TEST_PROVIDER_BIN_OVERRIDE";
    let missing = std::env::temp_dir().join(format!(
        "axis-missing-provider-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock should be available")
            .as_nanos()
    ));
    let missing_string = missing.to_string_lossy().to_string();
    let _guard = EnvVarGuard::set(env_name, Some(&missing_string));

    let resolved = resolve_provider_command_from_env_or_default(env_name, "codex");

    assert_eq!(resolved.argv, vec![missing_string]);
    assert!(!resolved.available);
    assert_eq!(
        resolved.unavailable_reason.as_deref(),
        Some("Configured path is not executable")
    );
}

#[test]
fn provider_command_marks_missing_default_binary_unavailable() {
    let env_name = "AXIS_TEST_PROVIDER_BIN_OVERRIDE";
    let _guard = EnvVarGuard::set(env_name, None);
    let empty_path = std::ffi::OsString::new();

    let resolved =
        resolve_provider_command_from_path_and_dirs(env_name, "codex", Some(empty_path.as_os_str()), &[]);

    assert_eq!(resolved.argv, vec!["codex".to_string()]);
    assert!(!resolved.available);
    assert_eq!(
        resolved.unavailable_reason.as_deref(),
        Some("Binary was not found on PATH")
    );
}
```

- [ ] **Step 2: Run the runtime-resolution tests to verify they fail**

Run: `cargo test -p axis-agent-runtime provider_command_ -v`

Expected: FAIL because `resolve_provider_command_from_env_or_default`, `resolve_provider_command_from_path_and_dirs`, `available`, and `unavailable_reason` do not exist yet.

- [ ] **Step 3: Implement the public resolution result in `crates/axis-agent-runtime/src/bin_resolver.rs`**

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderCommandResolution {
    pub argv: Vec<String>,
    pub available: bool,
    pub unavailable_reason: Option<String>,
}

pub fn resolve_provider_command_from_env_or_default(
    env_name: &str,
    default_binary: &str,
) -> ProviderCommandResolution {
    resolve_provider_command_from_path_and_dirs(
        env_name,
        default_binary,
        env::var_os("PATH").as_deref(),
        &fallback_search_dirs(),
    )
}

pub fn resolve_provider_command_from_path_and_dirs(
    env_name: &str,
    default_binary: &str,
    path_env: Option<&OsStr>,
    fallback_dirs: &[PathBuf],
) -> ProviderCommandResolution {
    if let Some(override_bin) = provider_bin_override(env_name) {
        let available = is_executable(Path::new(&override_bin));
        return ProviderCommandResolution {
            argv: vec![override_bin],
            available,
            unavailable_reason: (!available)
                .then(|| "Configured path is not executable".to_string()),
        };
    }

    if let Some(path) = resolve_binary_from_path_and_dirs(default_binary, path_env, fallback_dirs) {
        return ProviderCommandResolution {
            argv: vec![path.to_string_lossy().into_owned()],
            available: true,
            unavailable_reason: None,
        };
    }

    ProviderCommandResolution {
        argv: vec![default_binary.to_string()],
        available: false,
        unavailable_reason: Some("Binary was not found on PATH".to_string()),
    }
}

pub fn provider_base_argv_from_env_or_default(env_name: &str, default_binary: &str) -> Vec<String> {
    resolve_provider_command_from_env_or_default(env_name, default_binary).argv
}
```

- [ ] **Step 4: Re-export the new helper from `crates/axis-agent-runtime/src/lib.rs`**

```rust
pub use bin_resolver::{
    provider_base_argv_from_env_or_default, resolve_provider_command_from_env_or_default,
    ProviderCommandResolution,
};
```

- [ ] **Step 5: Write the failing bridge snapshot tests in `apps/axis-app/src/agent_sessions.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axis_agent_runtime::adapters::fake::FakeProvider;
    use std::sync::Arc;

    #[test]
    fn provider_options_keep_unavailable_profiles_visible() {
        let mut registry = ProviderRegistry::new();
        registry.register("alpha", Arc::new(FakeProvider::with_standard_script()));
        registry.register_with_metadata(
            "beta",
            Arc::new(FakeProvider::with_standard_script()),
            Some("basic lifecycle only"),
        );

        let bridge = AgentRuntimeBridge::with_registry_and_options(
            "alpha",
            registry,
            vec![
                ProviderProfileOption {
                    profile_id: "alpha".to_string(),
                    capability_note: None,
                    available: true,
                    unavailable_reason: None,
                },
                ProviderProfileOption {
                    profile_id: "beta".to_string(),
                    capability_note: Some("basic lifecycle only".to_string()),
                    available: false,
                    unavailable_reason: Some("Not installed".to_string()),
                },
            ],
        );

        let options = bridge.provider_options();
        assert_eq!(options.len(), 2);
        assert!(options[0].available);
        assert!(!options[1].available);
        assert_eq!(options[1].profile_id, "beta");
        assert_eq!(options[1].unavailable_reason.as_deref(), Some("Not installed"));
    }
}
```

- [ ] **Step 6: Run the bridge snapshot test to verify it fails**

Run: `cargo test -p axis-app provider_options_keep_unavailable_profiles_visible -v`

Expected: FAIL because `ProviderProfileOption`, `with_registry_and_options`, and `provider_options()` do not exist yet.

- [ ] **Step 7: Implement provider option snapshots in `apps/axis-app/src/agent_sessions.rs`**

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProviderProfileOption {
    pub profile_id: String,
    pub capability_note: Option<String>,
    pub available: bool,
    pub unavailable_reason: Option<String>,
}

struct BridgeInner {
    default_profile_id: String,
    provider_options: Vec<ProviderProfileOption>,
    manager: SessionManager,
    daemon: DaemonClient,
    daemon_records: HashMap<AgentSessionId, AgentSessionRecord>,
    daemon_revision: u64,
    desk_cwd: HashMap<u64, String>,
    surface_to_session: HashMap<SurfaceRuntimeKey, AgentSessionId>,
}

fn provider_option(
    profile_id: &str,
    capability_note: Option<&str>,
    resolution: &axis_agent_runtime::ProviderCommandResolution,
) -> ProviderProfileOption {
    ProviderProfileOption {
        profile_id: profile_id.to_string(),
        capability_note: capability_note.map(str::to_string),
        available: resolution.available,
        unavailable_reason: resolution
            .unavailable_reason
            .clone()
            .or_else(|| (!resolution.available).then(|| "Not installed".to_string())),
    }
}

pub(crate) fn provider_options(&self) -> Vec<ProviderProfileOption> {
    self.inner
        .lock()
        .map(|guard| guard.provider_options.clone())
        .unwrap_or_default()
}

#[cfg(test)]
pub(crate) fn with_registry_and_options(
    default_profile_id: impl Into<String>,
    registry: ProviderRegistry,
    provider_options: Vec<ProviderProfileOption>,
) -> Self {
    Self {
        inner: Mutex::new(BridgeInner {
            default_profile_id: default_profile_id.into(),
            provider_options,
            manager: SessionManager::new(registry),
            daemon: DaemonClient::default(),
            daemon_records: HashMap::new(),
            daemon_revision: 0,
            desk_cwd: HashMap::new(),
            surface_to_session: HashMap::new(),
        }),
    }
}
```

- [ ] **Step 8: Build the default bridge option list in `AgentRuntimeBridge::new()`**

```rust
let codex_resolution =
    axis_agent_runtime::resolve_provider_command_from_env_or_default(CODEX_BIN_ENV, CODEX_PROFILE_ID);
let codex_base_argv = codex_resolution.argv.clone();
registry.register_with_metadata(
    CODEX_PROFILE_ID,
    std::sync::Arc::new(CodexProvider::with_base_argv(codex_base_argv)),
    None::<String>,
);

let claude_resolution = axis_agent_runtime::resolve_provider_command_from_env_or_default(
    CLAUDE_CODE_BIN_ENV,
    CLAUDE_CODE_PROFILE_ID,
);
let claude_base_argv = claude_resolution.argv.clone();
registry.register_with_metadata(
    CLAUDE_CODE_PROFILE_ID,
    std::sync::Arc::new(ProcessOnlyProvider::with_base_argv(
        CLAUDE_CODE_PROFILE_ID,
        claude_base_argv,
    )),
    Some(CLAUDE_CODE_CAPABILITY_NOTE),
);

let provider_options = vec![
    provider_option(CODEX_PROFILE_ID, None, &codex_resolution),
    provider_option(
        CLAUDE_CODE_PROFILE_ID,
        Some(CLAUDE_CODE_CAPABILITY_NOTE),
        &claude_resolution,
    ),
];
```

- [ ] **Step 9: Run the runtime and bridge verification**

Run: `cargo test -p axis-agent-runtime provider_command_ -v && cargo test -p axis-app provider_options_keep_unavailable_profiles_visible -v`

Expected: PASS with both missing-binary cases and bridge snapshot coverage green.

- [ ] **Step 10: If the user wants a checkpoint commit, use this message**

```bash
git add crates/axis-agent-runtime/src/bin_resolver.rs \
  crates/axis-agent-runtime/src/lib.rs \
  apps/axis-app/src/agent_sessions.rs
git commit -m "feat: expose provider availability for agent launch UI"
```

## Task 2: Popup State And Pure Selection Helpers

**Files:**
- Create: `apps/axis-app/src/agent_provider_popup.rs`
- Modify: `apps/axis-app/src/main.rs`

- [ ] **Step 1: Write the failing popup-state tests in `apps/axis-app/src/agent_provider_popup.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_sessions::ProviderProfileOption;
    use axis_core::{PaneId, Point as WorkdeskPoint};

    #[test]
    fn provider_popup_state_rejects_unavailable_profile_selection() {
        let state = AgentProviderPopupState::new(
            0,
            AgentLaunchTarget::StackIntoPane(PaneId::new(7)),
            vec![
                ProviderProfileOption {
                    profile_id: "codex".to_string(),
                    capability_note: None,
                    available: true,
                    unavailable_reason: None,
                },
                ProviderProfileOption {
                    profile_id: "claude-code".to_string(),
                    capability_note: Some("basic lifecycle only".to_string()),
                    available: false,
                    unavailable_reason: Some("Not installed".to_string()),
                },
            ],
        );

        assert!(state.allows_selection("codex"));
        assert!(!state.allows_selection("claude-code"));
        assert!(!state.allows_selection("missing"));
    }

    #[test]
    fn provider_popup_state_reports_empty_state_when_all_rows_disabled() {
        let state = AgentProviderPopupState::new(
            0,
            AgentLaunchTarget::NewPane {
                world_center: WorkdeskPoint::new(640.0, 360.0),
            },
            vec![ProviderProfileOption {
                profile_id: "codex".to_string(),
                capability_note: None,
                available: false,
                unavailable_reason: Some("Not installed".to_string()),
            }],
        );

        assert_eq!(
            state.empty_state_message(),
            Some("No installed agent backends found")
        );
    }
}
```

- [ ] **Step 2: Run the popup-state tests to verify they fail**

Run: `cargo test -p axis-app provider_popup_state_ -v`

Expected: FAIL because `AgentProviderPopupState`, `AgentLaunchTarget`, `new`, `allows_selection`, and `empty_state_message` do not exist yet.

- [ ] **Step 3: Implement popup state and selection helpers in `apps/axis-app/src/agent_provider_popup.rs`**

```rust
use crate::agent_sessions::ProviderProfileOption;
use axis_core::{PaneId, Point as WorkdeskPoint};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AgentLaunchTarget {
    NewPane { world_center: WorkdeskPoint },
    StackIntoPane(PaneId),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AgentProviderPopupState {
    pub desk_index: usize,
    pub target: AgentLaunchTarget,
    pub options: Vec<ProviderProfileOption>,
}

impl AgentProviderPopupState {
    pub(crate) fn new(
        desk_index: usize,
        target: AgentLaunchTarget,
        options: Vec<ProviderProfileOption>,
    ) -> Self {
        Self {
            desk_index,
            target,
            options,
        }
    }

    pub(crate) fn allows_selection(&self, profile_id: &str) -> bool {
        self.options
            .iter()
            .find(|option| option.profile_id == profile_id)
            .map(|option| option.available)
            .unwrap_or(false)
    }

    pub(crate) fn empty_state_message(&self) -> Option<&'static str> {
        self.options
            .iter()
            .all(|option| !option.available)
            .then_some("No installed agent backends found")
    }
}
```

- [ ] **Step 4: Register the new module in `apps/axis-app/src/main.rs`**

```rust
mod agent_provider_popup;
```

- [ ] **Step 5: Add popup state to `AxisShell` and a dedicated dismiss helper**

```rust
struct AxisShell {
    agent_runtime: agent_sessions::AgentRuntimeBridge,
    agent_provider_popup: Option<agent_provider_popup::AgentProviderPopupState>,
    last_agent_runtime_revision: u64,
    workdesk_menu: Option<WorkdeskContextMenu>,
    stack_surface_menu: Option<StackSurfaceMenu>,
    workdesk_editor: Option<WorkdeskEditorState>,
    automation_rx: Receiver<AutomationEnvelope>,
    focus_handle: FocusHandle,
}

fn dismiss_agent_provider_popup(&mut self) -> bool {
    self.agent_provider_popup.take().is_some()
}
```

- [ ] **Step 6: Initialize the popup state in `AxisShell::new_with_agent_runtime(...)`**

```rust
agent_provider_popup: None,
```

- [ ] **Step 7: Run popup-state verification**

Run: `cargo test -p axis-app provider_popup_state_ -v`

Expected: PASS with the new popup state helpers green.

- [ ] **Step 8: If the user wants a checkpoint commit, use this message**

```bash
git add apps/axis-app/src/agent_provider_popup.rs apps/axis-app/src/main.rs
git commit -m "feat: add agent provider popup state model"
```

## Task 3: Intercept Workspace Launch And Add Explicit Profile Spawn Path

**Files:**
- Modify: `apps/axis-app/src/main.rs`

- [ ] **Step 1: Write the failing GPUI test for shortcut interception in `apps/axis-app/src/main.rs`**

```rust
#[gpui::test]
async fn spawn_agent_shortcut_opens_popup_before_creating_pane(cx: &mut TestAppContext) {
    use std::sync::Arc;

    let window = cx.add_empty_window();
    let shell = window.build_entity(cx, |_, view_cx| {
        let mut registry = ProviderRegistry::new();
        registry.register("alpha", Arc::new(FakeProvider::with_standard_script()));
        let bridge = agent_sessions::AgentRuntimeBridge::with_registry_and_options(
            "alpha",
            registry,
            vec![agent_sessions::ProviderProfileOption {
                profile_id: "alpha".to_string(),
                capability_note: None,
                available: true,
                unavailable_reason: None,
            }],
        );

        AxisShell::new_with_agent_runtime(
            vec![blank_workdesk("Desk", "Summary")],
            0,
            ShortcutMap::default(),
            None,
            automation::start_automation_server_at(std::env::temp_dir().join(format!(
                "axis-popup-test-{}-{}.sock",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock should be available")
                    .as_nanos()
            )))
            .expect("automation server should start"),
            view_cx.focus_handle(),
            SharedString::from(""),
            SharedString::from(""),
            bridge,
        )
    });

    window
        .update(cx, |shell, window, view_cx| {
            let before = shell.active_workdesk().panes.len();
            assert!(shell.execute_shortcut_action(ShortcutAction::SpawnAgentPane, window, view_cx));
            assert_eq!(shell.active_workdesk().panes.len(), before);
            assert!(shell.agent_provider_popup.is_some());
        })
        .unwrap();
}
```

- [ ] **Step 2: Run the shortcut interception test to verify it fails**

Run: `cargo test -p axis-app spawn_agent_shortcut_opens_popup_before_creating_pane -v`

Expected: FAIL because `AxisShell::new_with_agent_runtime` does not exist and the shortcut still creates a pane immediately.

- [ ] **Step 3: Add the injected-runtime constructor to `apps/axis-app/src/main.rs`**

```rust
impl AxisShell {
    fn new(
        workdesks: Vec<WorkdeskState>,
        active_workdesk: usize,
        shortcuts: ShortcutMap,
        boot_notice: Option<SharedString>,
        automation_server: AutomationServer,
        focus_handle: FocusHandle,
        ghostty_vendor_dir: SharedString,
        ghostty_status: SharedString,
    ) -> Self {
        Self::new_with_agent_runtime(
            workdesks,
            active_workdesk,
            shortcuts,
            boot_notice,
            automation_server,
            focus_handle,
            ghostty_vendor_dir,
            ghostty_status,
            agent_sessions::AgentRuntimeBridge::new(),
        )
    }

    fn new_with_agent_runtime(
        workdesks: Vec<WorkdeskState>,
        active_workdesk: usize,
        shortcuts: ShortcutMap,
        boot_notice: Option<SharedString>,
        automation_server: AutomationServer,
        focus_handle: FocusHandle,
        ghostty_vendor_dir: SharedString,
        ghostty_status: SharedString,
        agent_runtime: agent_sessions::AgentRuntimeBridge,
    ) -> Self {
        let AutomationServer { receiver, socket_path } = automation_server;
        let clamped_active_workdesk = active_workdesk.min(workdesks.len().saturating_sub(1));
        let mut shell = Self {
            workdesks,
            active_workdesk: clamped_active_workdesk,
            next_workdesk_id: 1,
            next_workdesk_runtime_id: 1,
            agent_runtime,
            agent_provider_popup: None,
            last_agent_runtime_revision: 0,
            workdesk_menu: None,
            stack_surface_menu: None,
            workdesk_editor: None,
            automation_rx: receiver,
            focus_handle,
            automation_socket_path: SharedString::from(socket_path.display().to_string()),
            ghostty_vendor_dir,
            ghostty_status,
            shortcuts,
            shortcut_editor: ShortcutEditorState::default(),
            inspector_open: false,
            sidebar_collapsed: false,
            notifications_open: false,
            mock_notifications_unread: 3,
            visible_terminal_surfaces: HashSet::new(),
            cursor_blink_visible: true,
            last_cursor_blink_at: Instant::now(),
            last_daemon_sync_at: Instant::now(),
            persist_generation: 0,
            touchpad_pan_state: None,
            last_touchpad_pan_end: None,
        };
        shell.assign_workdesk_ids_to_workdesks();
        shell.assign_runtime_ids_to_workdesks();

        if let Some(notice) = boot_notice {
            if let Some(workdesk) = shell.workdesks.get_mut(shell.active_workdesk) {
                workdesk.runtime_notice = Some(notice);
            }
        }

        for workdesk in &mut shell.workdesks {
            boot_workdesk_terminals(workdesk);
        }

        for index in 0..shell.workdesks.len() {
            if let Err(error) = shell.sync_review_summary_for_desk(index) {
                if let Some(workdesk) = shell.workdesks.get_mut(index) {
                    workdesk.runtime_notice =
                        Some(SharedString::from(format!("review summary stale: {error}")));
                }
            }
        }

        if let Err(error) = shell.sync_daemon_runtime_state() {
            if let Some(workdesk) = shell.workdesks.get_mut(shell.active_workdesk) {
                workdesk.runtime_notice.get_or_insert_with(|| {
                    SharedString::from(format!("axisd sync failed: {error}"))
                });
            }
        }

        shell
    }
}
```

- [ ] **Step 4: Add popup-open methods and route the shortcut into them**

In `execute_shortcut_action(...)`, replace the `SpawnAgentPane` match arm with the arm shown below.

```rust
fn open_agent_provider_popup_for_new_pane(&mut self, window: &Window, cx: &mut Context<Self>) {
    let world_center = self.screen_to_world(self.viewport_center(window));
    let options = self.agent_runtime.provider_options();
    self.agent_provider_popup = Some(agent_provider_popup::AgentProviderPopupState::new(
        self.active_workdesk,
        agent_provider_popup::AgentLaunchTarget::NewPane { world_center },
        options,
    ));
    cx.notify();
}

ShortcutAction::SpawnAgentPane => {
    self.open_agent_provider_popup_for_new_pane(window, cx);
    true
}
```

- [ ] **Step 5: Run the shortcut interception test to verify it passes**

Run: `cargo test -p axis-app spawn_agent_shortcut_opens_popup_before_creating_pane -v`

Expected: PASS with pane count unchanged and popup state populated.

- [ ] **Step 6: Write the failing explicit-profile launch test in `apps/axis-app/src/main.rs`**

```rust
#[test]
fn popup_selection_starts_requested_provider_for_new_pane() {
    let mut registry = ProviderRegistry::new();
    registry.register(
        "alpha",
        std::sync::Arc::new(FakeProvider::with_standard_script()),
    );
    registry.register(
        "beta",
        std::sync::Arc::new(FakeProvider::with_standard_script()),
    );

    let bridge = agent_sessions::AgentRuntimeBridge::with_registry_and_options(
        "alpha",
        registry,
        vec![
            agent_sessions::ProviderProfileOption {
                profile_id: "alpha".to_string(),
                capability_note: None,
                available: true,
                unavailable_reason: None,
            },
            agent_sessions::ProviderProfileOption {
                profile_id: "beta".to_string(),
                capability_note: Some("basic lifecycle only".to_string()),
                available: true,
                unavailable_reason: None,
            },
        ],
    );

    let mut workdesks = vec![blank_workdesk("Desk", "Summary")];
    workdesks[0].runtime_id = 88;
    workdesks[0].metadata.cwd = std::env::current_dir()
        .expect("cwd should resolve")
        .display()
        .to_string();
    let mut active_workdesk = 0;

    let (_pane_id, surface_id) = AxisShell::spawn_agent_surface_on_workdesk_state_with_profile(
        &mut workdesks,
        &mut active_workdesk,
        0,
        None,
        "beta",
        WorkdeskPoint::new(640.0, 360.0),
        true,
        &bridge,
    )
    .expect("selected provider should launch a new agent pane");

    let record = bridge
        .session_for_surface(workdesks[0].runtime_id, surface_id)
        .expect("runtime session should exist");
    assert_eq!(record.provider_profile_id, "beta");

    shutdown_workdesk_terminals(&mut workdesks[0]);
}
```

- [ ] **Step 7: Run the explicit-profile launch test to verify it fails**

Run: `cargo test -p axis-app popup_selection_starts_requested_provider_for_new_pane -v`

Expected: FAIL because `spawn_agent_surface_on_workdesk_state_with_profile` does not exist and agent creation still only supports the default-profile path.

- [ ] **Step 8: Add the explicit profile spawn path in `apps/axis-app/src/main.rs`**

```rust
fn initialize_surface_runtime(
    desk: &mut WorkdeskState,
    pane_size: WorkdeskSize,
    pane_surface_count: usize,
    surface: &SurfaceRecord,
    editor: Option<EditorBuffer>,
    agent_bridge: &agent_sessions::AgentRuntimeBridge,
    agent_profile_id: Option<&str>,
) {
    match surface.kind {
        PaneKind::Shell | PaneKind::Agent => {
            desk.attach_terminal_session(
                surface.id,
                &surface.kind,
                &surface.title,
                terminal_grid_size_for_pane(pane_size, pane_surface_count),
            );
            if surface.kind == PaneKind::Agent {
                let cwd = desk
                    .worktree_binding
                    .as_ref()
                    .map(|b| b.root_path.as_str())
                    .unwrap_or_else(|| desk.metadata.cwd.as_str());
                if let Some(terminal) = desk.terminals.get(&surface.id) {
                    let result = match agent_profile_id {
                        Some(profile_id) => agent_bridge.start_agent_for_surface_with_profile(
                            desk.runtime_id,
                            &desk.workdesk_id,
                            surface.id,
                            cwd,
                            terminal,
                            profile_id,
                            vec![],
                        ),
                        None => agent_bridge.start_agent_for_surface(
                            desk.runtime_id,
                            &desk.workdesk_id,
                            surface.id,
                            cwd,
                            terminal,
                        ),
                    };
                    if let Err(error) = result {
                        desk.runtime_notice = Some(SharedString::from(format!(
                            "Agent runtime did not start: {error}"
                        )));
                    }
                }
            }
        }
        PaneKind::Editor => {
            if let Some(editor) = editor {
                desk.editors.insert(surface.id, editor);
            }
        }
        PaneKind::Browser => {}
    }
}

fn spawn_surface_on_workdesk_state_with_agent_profile(
    workdesks: &mut [WorkdeskState],
    active_workdesk: &mut usize,
    desk_index: usize,
    target_pane_id: Option<PaneId>,
    kind: PaneKind,
    title: Option<String>,
    url: Option<String>,
    file_path: Option<String>,
    focus: bool,
    agent_profile_id: Option<&str>,
    agent_bridge: &agent_sessions::AgentRuntimeBridge,
) -> Result<(PaneId, SurfaceId), String> {
    let editor_lookup_path = if kind == PaneKind::Editor {
        file_path.as_deref().map(canonical_path_string)
    } else {
        None
    };
    if let Some(canonical_path) = editor_lookup_path.as_deref() {
        if target_pane_id.is_none() {
            if let Some((pane_id, surface_id)) =
                Self::find_editor_surface_by_path(workdesks, desk_index, canonical_path)
            {
                if focus {
                    workdesks[desk_index].focus_surface(pane_id, surface_id);
                    *active_workdesk = desk_index;
                }
                return Ok((pane_id, surface_id));
            }
        }
    }

    let Some(desk) = workdesks.get_mut(desk_index) else {
        return Err(format!("workdesk {desk_index} was not found"));
    };
    let surface_id = SurfaceId::new(desk.next_surface_serial);
    desk.next_surface_serial += 1;
    let requested_file_path = file_path.or(editor_lookup_path);
    let (surface, editor) =
        Self::build_surface_record(surface_id, kind.clone(), title, url, requested_file_path)?;

    if let Some(pane_id) = target_pane_id {
        let pane = desk
            .pane_mut(pane_id)
            .ok_or_else(|| format!("pane {} was not found", pane_id.raw()))?;
        let pane_size = pane.size;
        pane.push_surface(surface.clone(), focus);
        let pane_surface_count = pane.surfaces.len();
        Self::initialize_surface_runtime(
            desk,
            pane_size,
            pane_surface_count,
            &surface,
            editor,
            agent_bridge,
            agent_profile_id,
        );
        desk.resize_terminals_for_pane(pane_id);
        if focus {
            desk.focus_surface(pane_id, surface_id);
            *active_workdesk = desk_index;
        }
        return Ok((pane_id, surface_id));
    }

    let size = default_size_for_kind(&kind);
    let pane_id = PaneId::new(desk.next_pane_serial);
    desk.next_pane_serial += 1;
    let cascade = 36.0 * (desk.panes.len() % 6) as f32;
    let position = desk
        .active_pane
        .and_then(|active_pane_id| {
            desk.panes
                .iter()
                .find(|pane| pane.id == active_pane_id)
                .map(|pane| {
                    let horizontal_offset = if desk.layout_mode == LayoutMode::Free {
                        42.0
                    } else {
                        pane.size.width + 96.0
                    };
                    WorkdeskPoint::new(
                        pane.position.x + horizontal_offset,
                        pane.position.y + 42.0 * ((desk.panes.len() % 3) as f32),
                    )
                })
        })
        .unwrap_or_else(|| WorkdeskPoint::new(80.0 + cascade, 96.0 + cascade));
    let pane = PaneRecord::new(pane_id, position, size, surface.clone(), None);
    desk.panes.push(pane);
    Self::initialize_surface_runtime(
        desk,
        size,
        1,
        &surface,
        editor,
        agent_bridge,
        agent_profile_id,
    );
    if focus {
        desk.focus_surface(pane_id, surface_id);
        *active_workdesk = desk_index;
    }
    desk.drag_state = DragState::Idle;
    Ok((pane_id, surface_id))
}

fn spawn_agent_surface_on_workdesk_state_with_profile(
    workdesks: &mut [WorkdeskState],
    active_workdesk: &mut usize,
    desk_index: usize,
    target_pane_id: Option<PaneId>,
    profile_id: &str,
    world_center: WorkdeskPoint,
    focus: bool,
    agent_bridge: &agent_sessions::AgentRuntimeBridge,
) -> Result<(PaneId, SurfaceId), String> {
    let previous_count = workdesks[desk_index].panes.len();
    let (pane_id, surface_id) = Self::spawn_surface_on_workdesk_state_with_agent_profile(
        workdesks,
        active_workdesk,
        desk_index,
        target_pane_id,
        PaneKind::Agent,
        None,
        None,
        None,
        focus,
        Some(profile_id),
        agent_bridge,
    )?;

    if target_pane_id.is_none() {
        if let Some(pane) = workdesks[desk_index]
            .panes
            .iter_mut()
            .find(|pane| pane.id == pane_id)
        {
            let cascade = 36.0 * (previous_count % 6) as f32;
            pane.position = WorkdeskPoint::new(
                world_center.x - pane.size.width * 0.5 + cascade,
                world_center.y - pane.size.height * 0.5 + cascade,
            );
        }
    }

    Ok((pane_id, surface_id))
}
```

- [ ] **Step 9: Run the explicit-profile launch test and the local app regression pass**

Run: `cargo test -p axis-app popup_selection_starts_requested_provider_for_new_pane -v && cargo test -p axis-app ensure_agent_runtime_for_surface_ -v`

Expected: PASS with the new pane using the selected provider and existing runtime-surface tests still green.

- [ ] **Step 10: If the user wants a checkpoint commit, use this message**

```bash
git add apps/axis-app/src/main.rs
git commit -m "feat: route workspace agent launch through provider selection"
```

## Task 4: Stack-Menu Integration, Overlay Rendering, And Dismiss Paths

**Files:**
- Modify: `apps/axis-app/src/main.rs`

- [ ] **Step 1: Write the failing stack-target test in `apps/axis-app/src/main.rs`**

```rust
#[test]
fn popup_selection_starts_requested_provider_for_stack_target() {
    let mut registry = ProviderRegistry::new();
    registry.register(
        "alpha",
        std::sync::Arc::new(FakeProvider::with_standard_script()),
    );
    registry.register(
        "beta",
        std::sync::Arc::new(FakeProvider::with_standard_script()),
    );
    let bridge = agent_sessions::AgentRuntimeBridge::with_registry_and_options(
        "alpha",
        registry,
        vec![
            agent_sessions::ProviderProfileOption {
                profile_id: "alpha".to_string(),
                capability_note: None,
                available: true,
                unavailable_reason: None,
            },
            agent_sessions::ProviderProfileOption {
                profile_id: "beta".to_string(),
                capability_note: None,
                available: true,
                unavailable_reason: None,
            },
        ],
    );

    let mut workdesks = vec![WorkdeskState::new(
        "Desk",
        "Summary",
        vec![single_surface_pane(
            1,
            "Shell",
            PaneKind::Shell,
            WorkdeskPoint::new(0.0, 0.0),
            WorkdeskSize::new(920.0, 560.0),
        )],
    )];
    workdesks[0].runtime_id = 91;
    workdesks[0].metadata.cwd = std::env::current_dir()
        .expect("cwd should resolve")
        .display()
        .to_string();
    let mut active_workdesk = 0;

    let (pane_id, surface_id) = AxisShell::spawn_agent_surface_on_workdesk_state_with_profile(
        &mut workdesks,
        &mut active_workdesk,
        0,
        Some(PaneId::new(1)),
        "beta",
        WorkdeskPoint::new(320.0, 240.0),
        true,
        &bridge,
    )
    .expect("selected provider should stack into the existing pane");

    assert_eq!(pane_id, PaneId::new(1));
    let record = bridge
        .session_for_surface(workdesks[0].runtime_id, surface_id)
        .expect("runtime session should exist");
    assert_eq!(record.provider_profile_id, "beta");

    shutdown_workdesk_terminals(&mut workdesks[0]);
}
```

- [ ] **Step 2: Run the stack-target test to verify it fails**

Run: `cargo test -p axis-app popup_selection_starts_requested_provider_for_stack_target -v`

Expected: FAIL because the explicit helper does not yet support stack launch routing all the way through the popup workflow.

- [ ] **Step 3: Write the failing dismissal test in `apps/axis-app/src/main.rs`**

```rust
#[gpui::test]
async fn dismissing_popup_keeps_pane_count_unchanged(cx: &mut TestAppContext) {
    use std::sync::Arc;

    let window = cx.add_empty_window();
    let shell = window.build_entity(cx, |_, view_cx| {
        let mut registry = ProviderRegistry::new();
        registry.register("alpha", Arc::new(FakeProvider::with_standard_script()));
        let bridge = agent_sessions::AgentRuntimeBridge::with_registry_and_options(
            "alpha",
            registry,
            vec![agent_sessions::ProviderProfileOption {
                profile_id: "alpha".to_string(),
                capability_note: None,
                available: true,
                unavailable_reason: None,
            }],
        );

        AxisShell::new_with_agent_runtime(
            vec![blank_workdesk("Desk", "Summary")],
            0,
            ShortcutMap::default(),
            None,
            automation::start_automation_server_at(std::env::temp_dir().join(format!(
                "axis-popup-dismiss-test-{}-{}.sock",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock should be available")
                    .as_nanos()
            )))
            .expect("automation server should start"),
            view_cx.focus_handle(),
            SharedString::from(""),
            SharedString::from(""),
            bridge,
        )
    });

    window
        .update(cx, |shell, window, view_cx| {
            assert!(shell.execute_shortcut_action(ShortcutAction::SpawnAgentPane, window, view_cx));
            let before = shell.active_workdesk().panes.len();
            assert!(shell.dismiss_agent_provider_popup());
            assert_eq!(shell.active_workdesk().panes.len(), before);
            assert!(shell.agent_provider_popup.is_none());
        })
        .unwrap();
}
```

- [ ] **Step 4: Run the dismissal test to verify it fails**

Run: `cargo test -p axis-app dismissing_popup_keeps_pane_count_unchanged -v`

Expected: FAIL until the popup is fully wired into the state transitions and dismissal path.

- [ ] **Step 5: Add stack popup entry and completion handlers in `apps/axis-app/src/main.rs`**

```rust
fn open_agent_provider_popup_for_stack(
    &mut self,
    pane_id: PaneId,
    cx: &mut Context<Self>,
) {
    let options = self.agent_runtime.provider_options();
    self.agent_provider_popup = Some(agent_provider_popup::AgentProviderPopupState::new(
        self.active_workdesk,
        agent_provider_popup::AgentLaunchTarget::StackIntoPane(pane_id),
        options,
    ));
    cx.notify();
}

fn complete_agent_provider_popup_selection(
    &mut self,
    profile_id: &str,
    cx: &mut Context<Self>,
) -> bool {
    let Some(popup) = self.agent_provider_popup.clone() else {
        return false;
    };
    if !popup.allows_selection(profile_id) {
        return false;
    }

    self.agent_provider_popup = None;
    let result = match popup.target {
        agent_provider_popup::AgentLaunchTarget::NewPane { world_center } => {
            Self::spawn_agent_surface_on_workdesk_state_with_profile(
                &mut self.workdesks,
                &mut self.active_workdesk,
                popup.desk_index,
                None,
                profile_id,
                world_center,
                true,
                &self.agent_runtime,
            )
            .map(|_| ())
        }
        agent_provider_popup::AgentLaunchTarget::StackIntoPane(pane_id) => {
            Self::spawn_agent_surface_on_workdesk_state_with_profile(
                &mut self.workdesks,
                &mut self.active_workdesk,
                popup.desk_index,
                Some(pane_id),
                profile_id,
                WorkdeskPoint::new(0.0, 0.0),
                true,
                &self.agent_runtime,
            )
            .map(|_| ())
        }
    };

    match result {
        Ok(()) => {
            self.request_persist(cx);
            cx.notify();
            true
        }
        Err(error) => {
            self.set_runtime_notice(error);
            cx.notify();
            false
        }
    }
}
```

- [ ] **Step 6: Change the stack-menu `Agent` action to open the popup instead of creating a surface immediately**

```rust
.child(workdesk_menu_item(
    "Agent",
    "Stack an agent beside this flow",
    rgb(0x7cc7ff).into(),
    cx.listener(move |this, _, _, cx| {
        this.dismiss_stack_surface_menu();
        this.open_agent_provider_popup_for_stack(pane_id, cx);
        cx.stop_propagation();
    }),
))
```

- [ ] **Step 7: Render the centered modal overlay in `apps/axis-app/src/main.rs`**

```rust
let agent_provider_popup = self.agent_provider_popup.as_ref().map(|popup| {
    let popup_width = 320.0;
    let popup_height = 220.0 + (popup.options.len() as f32 * 44.0);
    let left = ((viewport_width - popup_width) * 0.5).max(sidebar_width + 12.0);
    let top = ((viewport_height - popup_height) * 0.5).max(24.0);

    div()
        .absolute()
        .left(px(sidebar_width))
        .top(px(0.0))
        .right(px(0.0))
        .bottom(px(0.0))
        .bg(rgba(0x00000088))
        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
            if this.dismiss_agent_provider_popup() {
                cx.notify();
            }
        }))
        .child(
            div()
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(popup_width))
                .p_2()
                .flex()
                .flex_col()
                .gap_2()
                .bg(rgb(0x11181e))
                .border_1()
                .border_color(rgb(0x2c3944))
                .rounded_lg()
                .shadow_lg()
                .child(div().text_sm().child("Start Agent"))
                .children(popup.options.iter().map(|option| {
                    let disabled = !option.available;
                    let profile_id = option.profile_id.clone();
                    div()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(if disabled { rgb(0x141a20) } else { rgb(0x182129) })
                        .text_color(if disabled { rgb(0x6f7b85) } else { rgb(0xdce2e8) })
                        .when(!disabled, |row| {
                            row.cursor_pointer().on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.complete_agent_provider_popup_selection(&profile_id, cx);
                                    cx.stop_propagation();
                                }),
                            )
                        })
                        .child(div().text_sm().child(option.profile_id.clone()))
                        .when_some(option.capability_note.clone(), |row, note| {
                            row.child(div().text_xs().text_color(rgb(0x7f8a94)).child(note))
                        })
                        .when_some(option.unavailable_reason.clone(), |row, reason| {
                            row.child(div().text_xs().text_color(rgb(0xff9b88)).child(reason))
                        })
                }))
                .when_some(popup.empty_state_message(), |column, message| {
                    column.child(div().text_xs().text_color(rgb(0xff9b88)).child(message))
                })
                .child(workdesk_menu_item(
                    "Cancel",
                    "Close without creating an agent",
                    rgb(0x7f8a94).into(),
                    cx.listener(|this, _, _, cx| {
                        this.dismiss_agent_provider_popup();
                        cx.stop_propagation();
                    }),
                )),
        )
});
```

- [ ] **Step 8: Add `Esc` dismissal in the existing key handling path**

```rust
if key.keystroke.key == "escape" && self.dismiss_agent_provider_popup() {
    cx.notify();
    return;
}
```

- [ ] **Step 9: Run the popup integration verification**

Run: `cargo test -p axis-app popup_selection_starts_requested_provider_for_stack_target -v && cargo test -p axis-app dismissing_popup_keeps_pane_count_unchanged -v && cargo check -p axis-app`

Expected: PASS with stack launch using the selected provider, dismissal leaving pane count unchanged, and the app compiling cleanly.

- [ ] **Step 10: Run the full app regression pass**

Run: `cargo test -p axis-app -v`

Expected: PASS with existing runtime/session/layout tests still green after popup integration.

- [ ] **Step 11: Manual verification**

1. Launch `axis-app`.
2. Press `cmd-alt-n`.
3. Confirm the popup opens and no pane is created before selection.
4. Confirm all registered providers are listed.
5. Confirm unavailable providers are visible but disabled.
6. Choose an available provider and confirm the new agent pane starts on that provider.
7. Open the stack `+` menu, choose `Agent`, and confirm the same popup flow is used.
8. Dismiss with `Cancel`, outside click, and `Esc`, and confirm pane count stays unchanged.

- [ ] **Step 12: If the user wants a checkpoint commit, use this message**

```bash
git add apps/axis-app/src/main.rs
git commit -m "feat: add custom provider popup for agent launch"
```

## Self-Review Checklist

Before executing the plan, verify these points against the spec:

1. `SpawnAgentPane` is intercepted by the popup flow.
2. Stack-menu `Agent` is intercepted by the same popup flow.
3. Template-created desks remain unchanged in this slice.
4. All registered providers remain visible.
5. Unavailable providers are disabled with a reason.
6. The selected provider flows into runtime start through an explicit profile path.
7. `Cancel`, outside click, and `Esc` dismiss without creating a pane.
8. CLI/socket automation remains unchanged.
