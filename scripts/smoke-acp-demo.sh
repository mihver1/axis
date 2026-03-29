#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: bash scripts/smoke-acp-demo.sh [--repo-root PATH] [--keep-app|--no-keep-app]

Runs an isolated ACP smoke demo against a managed `axisd` + `axis-app` pair.
The script:
1. builds `axisd`, `axis-app`, and `axis-cli`;
2. starts `axisd` on a temp socket/data dir, then starts `axis-app`;
3. wires deterministic demo wrappers for `codex` and `claude-code` into the daemon;
4. creates/attaches two worktrees and records daemon-side workdesk metadata;
5. starts the canonical `codex` provider and the `claude-code` baseline;
6. validates GUI heartbeat, daemon workdesk registry, and structured review payload state.

Environment overrides:
  AXIS_SMOKE_CODEX_BRANCH       default: axis-smoke-codex
  AXIS_SMOKE_CLAUDE_BRANCH      default: axis-smoke-claude
  AXIS_SMOKE_REVIEW_FILE        default: SMOKE_DEMO_CHANGE.md
  AXIS_SMOKE_CODEX_HOLD_SECS    default: 300
  AXIS_SMOKE_CLAUDE_HOLD_SECS   default: 300
EOF
}

log() {
  printf '[smoke-acp] %s\n' "$*"
}

fail() {
  printf '[smoke-acp] ERROR: %s\n' "$*" >&2
  exit 1
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$WORKSPACE_ROOT"
KEEP_APP=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo-root)
      shift
      [[ $# -gt 0 ]] || fail "--repo-root requires a path"
      REPO_ROOT="$1"
      ;;
    --keep-app)
      KEEP_APP=1
      ;;
    --no-keep-app)
      KEEP_APP=0
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      fail "unknown argument: $1"
      ;;
  esac
  shift
done

REPO_ROOT="$(cd "$REPO_ROOT" && pwd)"
CODEX_BRANCH="${AXIS_SMOKE_CODEX_BRANCH:-axis-smoke-codex}"
CLAUDE_BRANCH="${AXIS_SMOKE_CLAUDE_BRANCH:-axis-smoke-claude}"
REVIEW_FILE_NAME="${AXIS_SMOKE_REVIEW_FILE:-SMOKE_DEMO_CHANGE.md}"

TMP_DIR="$(mktemp -d -t axis-smoke-demo)"
APP_DATA_DIR="$TMP_DIR/data"
DAEMON_DATA_DIR="$TMP_DIR/daemon-data"
SOCKET_PATH="$TMP_DIR/axis.sock"
DAEMON_LOG="$TMP_DIR/axisd.log"
APP_LOG="$TMP_DIR/axis-app.log"
SESSION_PATH="$APP_DATA_DIR/session.json"
CODEX_WRAPPER="$TMP_DIR/codex-demo"
CLAUDE_WRAPPER="$TMP_DIR/claude-code-demo"
DAEMON_PID=""
APP_PID=""

cleanup() {
  local exit_code=$?
  trap - EXIT
  if [[ $exit_code -ne 0 || $KEEP_APP -eq 0 ]]; then
    if [[ -n "${APP_PID:-}" ]] && kill -0 "$APP_PID" 2>/dev/null; then
      kill "$APP_PID" 2>/dev/null || true
      wait "$APP_PID" 2>/dev/null || true
    fi
    if [[ -n "${DAEMON_PID:-}" ]] && kill -0 "$DAEMON_PID" 2>/dev/null; then
      kill "$DAEMON_PID" 2>/dev/null || true
      wait "$DAEMON_PID" 2>/dev/null || true
    fi
    rm -rf "$TMP_DIR"
  fi
  exit "$exit_code"
}
trap cleanup EXIT

json_string_path() {
  local path="$1"
  python3 -c '
import json
import sys

value = json.load(sys.stdin)
for part in [p for p in sys.argv[1].split(".") if p]:
    value = value[int(part)] if part.isdigit() else value[part]
if value is None:
    print("")
elif isinstance(value, bool):
    print("true" if value else "false")
elif isinstance(value, (int, float)):
    print(value)
elif isinstance(value, (dict, list)):
    print(json.dumps(value))
else:
    print(value)
' "$path"
}

axis_cli() {
  "$CLI_BIN" --socket "$SOCKET_PATH" "$@"
}

first_array_field() {
  local field="$1"
  python3 -c '
import json
import sys

items = json.load(sys.stdin)
if not items:
    print("")
else:
    value = items[0].get(sys.argv[1], "")
    if value is None:
        print("")
    else:
        print(value)
' "$field"
}

cargo_target_dir() {
  if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
    printf '%s\n' "$CARGO_TARGET_DIR"
    return 0
  fi

  cargo metadata --format-version 1 --no-deps \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])'
}

write_demo_wrappers() {
  cat >"$CODEX_WRAPPER" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
echo "AXIS_STATUS codex demo booting"
sleep "${AXIS_SMOKE_CODEX_DELAY_SECS:-1.5}"
echo "AXIS_ATTENTION needs_review"
echo "AXIS_STATUS review requested"
sleep "${AXIS_SMOKE_CODEX_HOLD_SECS:-300}"
EOF
  chmod +x "$CODEX_WRAPPER"

  cat >"$CLAUDE_WRAPPER" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
echo "claude-code baseline booting"
echo "basic lifecycle only"
sleep "${AXIS_SMOKE_CLAUDE_HOLD_SECS:-300}"
EOF
  chmod +x "$CLAUDE_WRAPPER"
}

wait_for_daemon() {
  local attempt
  for attempt in $(seq 1 120); do
    if axis_cli raw daemon.health '{}' >/dev/null 2>&1; then
      return 0
    fi
    if [[ -n "${DAEMON_PID:-}" ]] && ! kill -0 "$DAEMON_PID" 2>/dev/null; then
      break
    fi
    sleep 0.1
  done

  if [[ -f "$DAEMON_LOG" ]]; then
    sed -n '1,160p' "$DAEMON_LOG" >&2 || true
  fi
  fail "managed axisd did not become ready on $SOCKET_PATH"
}

wait_for_managed_app() {
  local attempt
  for attempt in $(seq 1 120); do
    if [[ -f "$SESSION_PATH" ]]; then
      return 0
    fi
    if [[ -n "${APP_PID:-}" ]] && ! kill -0 "$APP_PID" 2>/dev/null; then
      break
    fi
    sleep 0.25
  done

  if [[ -f "$APP_LOG" ]]; then
    sed -n '1,160p' "$APP_LOG" >&2 || true
  fi
  fail "managed axis-app did not persist $SESSION_PATH"
}

wait_for_gui_heartbeat() {
  local json=""
  local attempt

  for attempt in $(seq 1 80); do
    json="$(axis_cli ensure-gui)"
    if [[ "$(printf '%s\n' "$json" | json_string_path "launched")" == "false" ]]; then
      printf '%s\n' "$json"
      return 0
    fi
    sleep 0.25
  done

  printf '%s\n' "$json" >&2
  fail "daemon never observed a fresh GUI heartbeat for workspace $WORKSPACE_ROOT"
}

ensure_workdesk_record() {
  local workdesk_id="$1"
  local name="$2"
  local summary="$3"
  local binding_json="$4"

  axis_cli raw workdesk.ensure "$(python3 -c '
import json
import sys

record = {
    "workdesk_id": sys.argv[1],
    "workspace_root": sys.argv[2],
    "name": sys.argv[3],
    "summary": sys.argv[4],
    "template": "implementation",
    "worktree_binding": json.loads(sys.argv[5]),
}
print(json.dumps({"record": record}))
' "$workdesk_id" "$REPO_ROOT" "$name" "$summary" "$binding_json")"
}

wait_for_agent_state() {
  local worktree_id="$1"
  local profile_id="$2"
  local expected_lifecycle="$3"
  local expected_attention="$4"
  local json=""
  local attempt

  for attempt in $(seq 1 80); do
    json="$(axis_cli list-agents "worktree_id=$worktree_id")"
    if AGENTS_JSON="$json" \
      PROFILE_ID="$profile_id" \
      EXPECTED_LIFECYCLE="$expected_lifecycle" \
      EXPECTED_ATTENTION="$expected_attention" \
      python3 - <<'PY'
import json
import os
import sys

agents = json.loads(os.environ["AGENTS_JSON"])
for agent in agents:
    if (
        agent.get("provider_profile_id") == os.environ["PROFILE_ID"]
        and agent.get("lifecycle") == os.environ["EXPECTED_LIFECYCLE"]
        and agent.get("attention") == os.environ["EXPECTED_ATTENTION"]
    ):
        sys.exit(0)
sys.exit(1)
PY
    then
      printf '%s\n' "$json"
      return 0
    fi
    sleep 0.25
  done

  printf '%s\n' "$json" >&2
  fail "timed out waiting for $profile_id to reach lifecycle=$expected_lifecycle attention=$expected_attention"
}

log "Building axisd, axis-app, and axis-cli"
cargo build -q -p axisd -p axis-app -p axis-cli

TARGET_DIR="$(cargo_target_dir)"
DAEMON_BIN="$TARGET_DIR/debug/axisd"
APP_BIN="$TARGET_DIR/debug/axis-app"
CLI_BIN="$TARGET_DIR/debug/axis"

[[ -x "$DAEMON_BIN" ]] || fail "missing daemon binary at $DAEMON_BIN"
[[ -x "$APP_BIN" ]] || fail "missing app binary at $APP_BIN"
[[ -x "$CLI_BIN" ]] || fail "missing cli binary at $CLI_BIN"

write_demo_wrappers

log "Starting isolated axisd"
mkdir -p "$DAEMON_DATA_DIR"
(
  cd "$WORKSPACE_ROOT"
  AXIS_SOCKET_PATH="$SOCKET_PATH" \
  AXIS_DAEMON_DATA_DIR="$DAEMON_DATA_DIR" \
  AXIS_CODEX_BIN="$CODEX_WRAPPER" \
  AXIS_CLAUDE_CODE_BIN="$CLAUDE_WRAPPER" \
  "$DAEMON_BIN"
) >"$DAEMON_LOG" 2>&1 &
DAEMON_PID="$!"
wait_for_daemon

log "Starting isolated axis-app"
mkdir -p "$APP_DATA_DIR"
(
  cd "$WORKSPACE_ROOT"
  AXIS_APP_DATA_DIR="$APP_DATA_DIR" \
  AXIS_SOCKET_PATH="$SOCKET_PATH" \
  AXIS_DAEMON_DATA_DIR="$DAEMON_DATA_DIR" \
  "$APP_BIN"
) >"$APP_LOG" 2>&1 &
APP_PID="$!"
wait_for_managed_app
gui_json="$(wait_for_gui_heartbeat)"

log "Creating or attaching codex worktree desk"
codex_worktree_json="$(axis_cli worktree "repo_root=$REPO_ROOT" "branch=$CODEX_BRANCH")"
codex_worktree_id="$(printf '%s\n' "$codex_worktree_json" | json_string_path "worktree_id")"
[[ -n "$codex_worktree_id" ]] || fail "codex worktree id was empty"
codex_binding_json="$(printf '%s\n' "$codex_worktree_json" | json_string_path "binding")"
[[ -n "$codex_binding_json" ]] || fail "codex worktree binding was empty"
ensure_workdesk_record "smoke-codex-desk" "Smoke Codex Desk" "Codex ACP smoke demo" "$codex_binding_json" >/dev/null

cat >"$codex_worktree_id/$REVIEW_FILE_NAME" <<EOF
# ACP Smoke Demo

This file is generated by scripts/smoke-acp-demo.sh to force a visible review summary.
EOF

log "Creating or attaching claude-code worktree desk"
claude_worktree_json="$(axis_cli worktree "repo_root=$REPO_ROOT" "branch=$CLAUDE_BRANCH")"
claude_worktree_id="$(printf '%s\n' "$claude_worktree_json" | json_string_path "worktree_id")"
[[ -n "$claude_worktree_id" ]] || fail "claude-code worktree id was empty"
claude_binding_json="$(printf '%s\n' "$claude_worktree_json" | json_string_path "binding")"
[[ -n "$claude_binding_json" ]] || fail "claude worktree binding was empty"
ensure_workdesk_record "smoke-claude-desk" "Smoke Claude Desk" "Claude ACP smoke demo" "$claude_binding_json" >/dev/null
workdesks_json="$(axis_cli raw workdesk.list "$(python3 -c 'import json,sys; print(json.dumps({"workspace_root": sys.argv[1]}))' "$REPO_ROOT")")"

existing_claude_agents_json="$(axis_cli list-agents "worktree_id=$claude_worktree_id")"
existing_claude_session_id="$(printf '%s\n' "$existing_claude_agents_json" | first_array_field "id")"
existing_claude_profile_id="$(printf '%s\n' "$existing_claude_agents_json" | first_array_field "provider_profile_id")"
if [[ -n "$existing_claude_session_id" && "$existing_claude_profile_id" != "claude-code" ]]; then
  log "Replacing default $existing_claude_profile_id session on claude desk"
  axis_cli stop-agent "agent_session_id=$existing_claude_session_id" >/dev/null
fi

log "Starting claude-code baseline provider"
if [[ "$existing_claude_profile_id" != "claude-code" ]]; then
  axis_cli start-agent "worktree_id=$claude_worktree_id" "provider_profile_id=claude-code" >/dev/null
fi
claude_agents_json="$(wait_for_agent_state "$claude_worktree_id" "claude-code" "running" "quiet")"

log "Confirming codex attention while its desk is unfocused"
axis_cli start-agent "worktree_id=$codex_worktree_id" "provider_profile_id=codex" >/dev/null
codex_agents_json="$(wait_for_agent_state "$codex_worktree_id" "codex" "waiting" "needs_review")"

log "Fetching review payload"
review_json="$(axis_cli review "worktree_id=$codex_worktree_id")"

GUI_JSON="$gui_json" \
WORKDESKS_JSON="$workdesks_json" \
REVIEW_JSON="$review_json" \
CODEX_AGENTS_JSON="$codex_agents_json" \
CLAUDE_AGENTS_JSON="$claude_agents_json" \
CODEX_WORKTREE_ID="$codex_worktree_id" \
CLAUDE_WORKTREE_ID="$claude_worktree_id" \
REVIEW_FILE_NAME="$REVIEW_FILE_NAME" \
SOCKET_PATH="$SOCKET_PATH" \
DAEMON_LOG="$DAEMON_LOG" \
APP_DATA_DIR="$APP_DATA_DIR" \
APP_LOG="$APP_LOG" \
APP_PID="$APP_PID" \
DAEMON_PID="$DAEMON_PID" \
KEEP_APP="$KEEP_APP" \
TMP_DIR="$TMP_DIR" \
python3 - <<'PY'
import json
import os
import sys

gui = json.loads(os.environ["GUI_JSON"])
workdesks = json.loads(os.environ["WORKDESKS_JSON"])
review = json.loads(os.environ["REVIEW_JSON"])
review_summary = review.get("summary") or {}
review_files = review.get("files") or []
review_ready = bool(review_summary.get("ready_for_review", False))
files_changed = int(review_summary.get("files_changed", 0))
uncommitted_files = int(review_summary.get("uncommitted_files", 0))
codex_agents = json.loads(os.environ["CODEX_AGENTS_JSON"])
claude_agents = json.loads(os.environ["CLAUDE_AGENTS_JSON"])
codex_worktree_id = os.environ["CODEX_WORKTREE_ID"]
claude_worktree_id = os.environ["CLAUDE_WORKTREE_ID"]
review_file_name = os.environ["REVIEW_FILE_NAME"]

def fail(message: str) -> None:
    print(f"[smoke-acp] ERROR: {message}", file=sys.stderr)
    sys.exit(1)

def workdesk_for(worktree_id: str) -> dict:
    for desk in workdesks:
        binding = desk.get("worktree_binding") or {}
        if binding.get("root_path") == worktree_id:
            return desk
    fail(f"missing workdesk for {worktree_id}")

def session_for(agents: list, profile_id: str) -> dict:
    for agent in agents:
        if agent.get("provider_profile_id") == profile_id:
            return agent
    fail(f"missing {profile_id} agent session")

codex_desk = workdesk_for(codex_worktree_id)
claude_desk = workdesk_for(claude_worktree_id)
codex_session = session_for(codex_agents, "codex")
claude_session = session_for(claude_agents, "claude-code")

if gui.get("launched") is not False:
    fail("daemon did not observe the already-running GUI heartbeat")
if codex_session["lifecycle"] != "waiting" or codex_session["attention"] != "needs_review":
    fail("codex session did not reach waiting/needs_review")
if claude_session["lifecycle"] != "running" or claude_session["attention"] != "quiet":
    fail("claude-code baseline did not remain running/quiet")
if files_changed < 1:
    fail("review payload did not report any changed files")
if uncommitted_files < 1:
    fail("review payload did not report the generated uncommitted change")

changed = {entry.get("path") for entry in review_files if entry.get("path")}
if review_file_name not in changed:
    fail(f"review payload did not include {review_file_name}")

summary = [
    "[smoke-acp] ACP smoke demo passed.",
    f"[smoke-acp] Managed daemon pid: {os.environ['DAEMON_PID']}",
    f"[smoke-acp] Managed app pid: {os.environ['APP_PID']}",
    f"[smoke-acp] Socket: {os.environ['SOCKET_PATH']}",
    f"[smoke-acp] Daemon log: {os.environ['DAEMON_LOG']}",
    f"[smoke-acp] App data dir: {os.environ['APP_DATA_DIR']}",
    f"[smoke-acp] App log: {os.environ['APP_LOG']}",
    f"[smoke-acp] Codex worktree: {codex_worktree_id}",
    f"[smoke-acp] Claude worktree: {claude_worktree_id}",
    f"[smoke-acp] Codex attention: {codex_session['attention']} ({codex_session['lifecycle']})",
    f"[smoke-acp] Claude baseline: {claude_session['attention']} ({claude_session['lifecycle']})",
    f"[smoke-acp] GUI heartbeat observed: launched={json.dumps(gui['launched'])}",
    f"[smoke-acp] Daemon workdesks: codex={codex_desk['workdesk_id']} claude={claude_desk['workdesk_id']}",
    "[smoke-acp] Review payload: "
    f"ready={json.dumps(review_ready)} "
    f"files_changed={json.dumps(files_changed)} "
    f"uncommitted={json.dumps(uncommitted_files)} "
    f"visible={len(changed)}",
]

if os.environ["KEEP_APP"] == "1":
    summary.append(
        f"[smoke-acp] Managed demo is still running. Stop it with: kill {os.environ['APP_PID']} {os.environ['DAEMON_PID']} && rm -rf {os.environ['TMP_DIR']}"
    )
else:
    summary.append("[smoke-acp] Managed daemon and app will be cleaned up on exit.")

print("\n".join(summary))
PY
