# Agent Provider Popup Design

## Summary

`axis` should show a custom in-app popup before starting any UI-launched
agent session so the user can choose which registered agent profile to run on
this machine.

The popup should list all registered provider profiles, keep unavailable
profiles visible but disabled, and launch the chosen available profile through
the existing runtime-backed agent session flow.

## Problem Statement

Today, `axis` starts new agent panes immediately on the default provider
profile.

That causes three UX problems:

1. the user cannot choose between installed agent backends such as `codex` and
   `claude-code`;
2. the app does not expose which registered providers are currently usable on
   this machine;
3. the same "new agent" intent behaves too implicitly across shortcut-driven
   and menu-driven pane creation.

For the next slice, agent startup should become an explicit choice instead of
an invisible default.

## Product Outcome

After this change, the user flow should be:

1. trigger a new agent from the app UI;
2. see a custom popup that lists registered agent profiles;
3. pick an available profile;
4. launch the new agent pane or stacked agent surface on that profile.

Unavailable profiles remain visible for discoverability, but cannot be chosen.
Cancel closes the popup without creating or starting an agent.

## In Scope

This design covers:

1. `New agent pane` from the main workspace action and shortcut;
2. `Agent` from the stack surface add menu;
3. custom popup rendering inside `axis-app`;
4. runtime-backed profile inventory and machine-availability reporting;
5. launching via explicit provider profile after the popup choice.

## Out Of Scope

This design does not add:

1. new agent providers;
2. changes to CLI or socket automation when `provider_profile_id` is already
   provided explicitly;
3. a full settings screen for provider management;
4. richer provider health checks beyond local binary availability and existing
   capability notes;
5. system-native dialogs.

## UX Design

### Trigger Points

The popup opens before `axis` creates and starts an agent from these UI flows:

1. workspace action: `SpawnAgentPane`;
2. pane stack menu action: `Agent`.

This first slice keeps the change focused on explicit agent-launch actions.
Template-created desks and automation-triggered launches remain on their
current path and are not changed by this design.

### Popup Layout

The popup should be a custom in-app overlay, visually consistent with current
`axis` menus:

1. centered inside the viewport;
2. modal enough to capture the choice and dismiss on outside click;
3. small title such as `Start Agent`;
4. one selectable row per registered provider profile;
5. a secondary `Cancel` action.

Each provider row shows:

1. `profile_id`;
2. capability note when present;
3. availability state for this machine.

### Availability Rules

The popup should use the `mixed` behavior:

1. show all registered profiles;
2. enable rows for profiles whose configured executable resolves locally;
3. disable rows for profiles whose executable does not resolve locally;
4. label unavailable rows with a short reason such as `Not installed`.

### Selection Behavior

When the user selects an available profile:

1. close the popup;
2. continue the original action;
3. create the new agent pane or stacked surface;
4. start the runtime with the chosen `provider_profile_id`.

When the user cancels or dismisses the popup:

1. close the popup;
2. do not create a new agent pane;
3. do not start a runtime session;
4. leave the rest of the workspace unchanged.

## Technical Design

### Runtime Provider Inventory

The runtime already owns the provider registry, so it should remain the source
of truth for popup contents.

`axis-agent-runtime` should expose enough provider metadata for UI rendering:

1. `profile_id`;
2. `capability_note`;
3. availability on the current machine;
4. availability detail for disabled rows.

The recommended shape is a UI-facing profile snapshot such as
`ProviderProfileOption` or an extended `ProviderProfileMetadata`, returned from
`AgentRuntimeBridge`.

### Binary Availability

Current startup code resolves provider binaries for launch, but it does not
expose whether resolution succeeded.

The runtime should add a small public binary-resolution result so the app can
distinguish:

1. resolved executable path or explicit env override;
2. unresolved fallback binary name;
3. unavailable/missing provider binary.

That same logic should be used both for popup availability and actual launch
setup so the UI and runtime do not drift.

### App State

`AxisShell` should gain dedicated popup state, separate from workdesk and stack
menus.

The popup state should capture:

1. whether the popup is open;
2. target workdesk index;
3. launch target: new pane or stack into existing pane;
4. provider options snapshot to render.

A focused enum keeps the launch continuation explicit, for example:

1. `NewPane`;
2. `StackIntoPane(PaneId)`.

### Launch Flow Integration

The popup becomes a gate in front of the current UI launch flow.

For `SpawnAgentPane`:

1. open popup instead of immediately calling pane creation;
2. after selection, create the agent pane;
3. start runtime with the selected profile.

For `stack_surface_in_pane(..., PaneKind::Agent, ...)`:

1. open popup instead of immediately stacking the agent;
2. after selection, create the stacked agent surface;
3. start runtime with the selected profile.

To keep this change localized, the app should add one explicit path for
"spawn agent with profile" instead of overloading the default
`spawn_pane(PaneKind::Agent, ...)` path with hidden branching.

### Rendering

The popup should render as an overlay in the main view tree, similar in spirit
to the current custom context menus, but centered and modal rather than
anchored to a click position.

The overlay should support:

1. outside-click dismissal;
2. `Esc` dismissal;
3. clear disabled styling for unavailable providers;
4. click-to-select for available providers.

Arrow-key row navigation and `Enter` selection are out of scope for this slice.

## Error Handling

### No Available Providers

If all registered providers are unavailable:

1. still open the popup;
2. show all rows as disabled;
3. show a short empty-state hint such as `No installed agent backends found`;
4. keep `Cancel` available.

### Stale Availability

If the popup shows a profile as available but launch still fails:

1. close the popup;
2. surface the existing runtime error notice;
3. preserve the current workspace state without a partially running session.

### Missing Capability Note

Missing `capability_note` is not an error.
The row should still render with only `profile_id` plus availability state.

## Testing Strategy

### Runtime Tests

Add focused tests around provider option snapshots:

1. available profile resolves from env override or PATH;
2. unavailable profile remains listed but disabled;
3. profile ordering stays stable for rendering.

### App Tests

Add app-level tests for popup behavior:

1. `SpawnAgentPane` opens the popup instead of creating a pane immediately;
2. selecting a profile creates the pane and launches that explicit profile;
3. dismissing the popup leaves pane count unchanged;
4. stack-menu `Agent` follows the same popup flow.

### Manual Verification

Manual verification should cover:

1. `cmd-alt-n` opens the popup;
2. `codex` and `claude-code` both appear when registered;
3. unavailable providers are visible but disabled;
4. selecting an available profile starts the correct provider;
5. `Cancel`, outside click, and `Esc` dismiss without side effects.

## Success Criteria

This slice is complete when:

1. every UI-triggered explicit agent-launch action opens the custom popup;
2. the popup lists all registered providers;
3. unavailable providers are visible but disabled;
4. available selection starts the chosen provider instead of the default one;
5. cancel and dismiss paths leave the workspace unchanged.
