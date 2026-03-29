# axis

`axis` is a native Rust workspace for spatial, agent-oriented development on macOS.
Today it is best understood as an experimental agent IDE: a GPUI desktop app with
workdesks, live terminal panes, agent panes, worktree-aware review signals, and a
local daemon/CLI control plane.

The project is usable for internal dogfooding, but it is still early. Expect rough
edges, evolving automation semantics, and a setup flow that is optimized for
contributors rather than end users.

## What Is Here Today

- `axis-app`: the desktop UI
- `axisd`: the local daemon/control-plane socket
- `axis`: the CLI client for daemon automation
- `axis-agent-runtime`: provider/session lifecycle plumbing
- `axis-terminal`: PTY + Ghostty-backed terminal rendering
- `axis-editor`: lightweight file-backed editor surface

## Current Platform

The primary target is macOS.

You should have:

- `/Applications/Xcode.app`
- Homebrew
- `git`
- `just`
- `cargo` / `rustc`
- `zig`

`rust-toolchain.toml` pins Rust `stable` with `clippy` and `rustfmt`, and
`.cargo/config.toml` points Cargo at the full Xcode toolchain on macOS.

## Quick Start

1. Verify the local toolchain:

```bash
just doctor
```

2. Check the workspace:

```bash
just check
```

3. Run the test suite:

```bash
just test
```

4. Launch the app:

```bash
just run
```

The app will open even if `axisd` is not running, but the full daemon-backed
automation loop is better with the daemon available.

## Full Local Loop

For the current intended setup, run the daemon and app side by side:

Terminal 1:

```bash
cargo run -p axisd
```

Terminal 2:

```bash
just run
```

Terminal 3:

```bash
cargo run -p axis-cli -- state
```

Additional CLI examples:

```bash
cargo run -p axis-cli -- list-agents
cargo run -p axis-cli -- next-attention
cargo run -p axis-cli -- ensure-gui
```

`axisd` is the authoritative local control plane for CLI automation. The GUI can
fall back to local runtime paths when the daemon is unavailable, but socket-based
automation should be expected to go through `axisd`.

## Useful Commands

```bash
just doctor     # verify macOS toolchain and Xcode setup
just check      # cargo check --workspace
just test       # cargo test -q
just clippy     # cargo clippy --workspace --all-targets
just run        # launch axis-app
just smoke-acp  # run the ACP/demo smoke script
just dmg        # build a macOS DMG in dist/
```

## Environment Variables

These are the main knobs worth knowing first:

| Variable | Purpose |
| --- | --- |
| `AXIS_SOCKET_PATH` | Override the daemon socket path used by `axisd`, `axis-app`, and `axis`. |
| `AXIS_DAEMON_DATA_DIR` | Override the daemon data directory. |
| `AXIS_APP_DATA_DIR` | Override the GUI app data/session directory. |
| `AXIS_CODEX_BIN` | Override the executable used for the `codex` provider profile. |
| `AXIS_CLAUDE_CODE_BIN` | Override the executable used for the `claude-code` provider profile. |
| `AXIS_APP_BIN` | Override the GUI binary that `axisd` launches for `ensure-gui`. |
| `AXIS_DAEMON_SOCKET_TIMEOUT_MS` | Override daemon client timeout in tests/debugging. |
| `AXIS_GUI_HEARTBEAT_TTL_MS` | Override the daemon-side GUI heartbeat TTL. |
| `AXIS_MACOS_BUNDLE_ID` | Override the app bundle identifier used by `just dmg` (defaults to `tech.artelproject.axis`). |
| `AXIS_MACOS_TEAM_ID` | Override the Apple team ID used for notarization-oriented packaging metadata (defaults to `6DR98YW3PY`). |
| `AXIS_MACOS_SIGN_IDENTITY` | Override the signing identity used by `just dmg`; if unset, the recipe auto-selects a single `Developer ID Application` identity when available. |
| `AXIS_MACOS_NOTARY_PROFILE` | Optional `xcrun notarytool` keychain profile name to notarize and staple the generated DMG. |

## Troubleshooting

If `GPUI` or macOS linking fails:

1. Make sure `/Applications/Xcode.app` exists.
2. Run `sudo xcodebuild -license accept`.
3. If needed, run:

```bash
sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer
```

4. Re-run:

```bash
just doctor
```

## Docs

- [macOS bootstrap plan](docs/macos-bootstrap-plan.md)
- [v0 prototype scope](docs/v0-prototype-scope.md)
- [workdesk layout modes](docs/workdesk-layout-modes.md)
- [ACP-first agent runtime design](docs/superpowers/specs/2026-03-26-acp-first-agent-runtime-design.md)

## Packaging

`just dmg` builds `dist/Axis-<version>.dmg` and writes `Axis.app` with the
default bundle identifier `tech.artelproject.axis`.

If exactly one `Developer ID Application` certificate is available in the active
keychain, the recipe signs the nested binaries, app bundle, and DMG
automatically. If multiple matching identities exist, set
`AXIS_MACOS_SIGN_IDENTITY` explicitly. If no Developer ID identity is
available, the recipe falls back to a single `Apple Development` identity for
local installs, and then to ad-hoc signing if no signing identity is available
at all.

To notarize the DMG, first create a `notarytool` keychain profile and then run
`just dmg` with `AXIS_MACOS_NOTARY_PROFILE` set. The recipe will submit the DMG,
wait for completion, and staple the notarized ticket to the DMG. If the profile
is missing or the active identity is not `Developer ID Application`, the recipe
prints a warning and skips notarization instead of failing after the build.
