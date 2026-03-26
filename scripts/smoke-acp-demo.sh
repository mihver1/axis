#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: bash scripts/smoke-acp-demo.sh [--repo-root PATH] [--keep-app|--no-keep-app]

Runs an isolated ACP smoke demo against a managed `axis-app` instance.
The script:
1. builds `axis-app` and `axis-cli`;
2. starts `axis-app` on a temp socket and temp app-data dir;
3. wires deterministic demo wrappers for `codex` and `claude-code`;
4. creates/attaches two worktree-backed Implementation desks;
5. starts the canonical `codex` provider and the `claude-code` baseline;
6. triggers review-ready attention and validates the review/attention loop.

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
SOCKET_PATH="$TMP_DIR/axis.sock"
APP_LOG="$TMP_DIR/axis-app.log"
CODEX_WRAPPER="$TMP_DIR/codex-demo"
CLAUDE_WRAPPER="$TMP_DIR/claude-code-demo"
APP_PID=""

cleanup() {
  local exit_code=$?
  trap - EXIT
  if [[ $exit_code -ne 0 || $KEEP_APP -eq 0 ]]; then
    if [[ -n "${APP_PID:-}" ]] && kill -0 "$APP_PID" 2>/dev/null; then
      kill "$APP_PID" 2>/dev/null || true
      wait "$APP_PID" 2>/dev/null || true
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
    | python3 - <<'PY'
import json
import sys

print(json.load(sys.stdin)["target_directory"])
PY
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

wait_for_managed_app() {
  local attempt
  for attempt in $(seq 1 120); do
    if [[ -S "$SOCKET_PATH" ]] && axis_cli state >/dev/null 2>&1; then
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
  fail "managed axis-app did not become ready on $SOCKET_PATH"
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

log "Building axis-app and axis-cli"
cargo build -q -p axis-app -p axis-cli

TARGET_DIR="$(cargo_target_dir)"
CLI_BIN="$TARGET_DIR/debug/axis"

[[ -x "$CLI_BIN" ]] || fail "missing cli binary at $CLI_BIN"

write_demo_wrappers

log "Starting isolated axis-app"
mkdir -p "$APP_DATA_DIR"
(
  cd "$WORKSPACE_ROOT"
  AXIS_APP_DATA_DIR="$APP_DATA_DIR" \
  AXIS_SOCKET_PATH="$SOCKET_PATH" \
  AXIS_CODEX_BIN="$CODEX_WRAPPER" \
  AXIS_CLAUDE_CODE_BIN="$CLAUDE_WRAPPER" \
  cargo run -q -p axis-app
) >"$APP_LOG" 2>&1 &
APP_PID="$!"
wait_for_managed_app

log "Creating or attaching codex worktree desk"
codex_worktree_json="$(axis_cli worktree "repo_root=$REPO_ROOT" "branch=$CODEX_BRANCH")"
codex_worktree_id="$(printf '%s\n' "$codex_worktree_json" | json_string_path "worktree_id")"
[[ -n "$codex_worktree_id" ]] || fail "codex worktree id was empty"

cat >"$codex_worktree_id/$REVIEW_FILE_NAME" <<EOF
# ACP Smoke Demo

This file is generated by scripts/smoke-acp-demo.sh to force a visible review summary.
EOF

log "Creating or attaching claude-code worktree desk"
claude_worktree_json="$(axis_cli worktree "repo_root=$REPO_ROOT" "branch=$CLAUDE_BRANCH")"
claude_worktree_id="$(printf '%s\n' "$claude_worktree_json" | json_string_path "worktree_id")"
[[ -n "$claude_worktree_id" ]] || fail "claude-code worktree id was empty"

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

log "Fetching review summary and next attention target"
review_json="$(axis_cli review "worktree_id=$codex_worktree_id")"
state_json="$(axis_cli state)"
next_attention_json="$(axis_cli next-attention)"

STATE_JSON="$state_json" \
REVIEW_JSON="$review_json" \
NEXT_ATTENTION_JSON="$next_attention_json" \
CODEX_AGENTS_JSON="$codex_agents_json" \
CLAUDE_AGENTS_JSON="$claude_agents_json" \
CODEX_WORKTREE_ID="$codex_worktree_id" \
CLAUDE_WORKTREE_ID="$claude_worktree_id" \
REVIEW_FILE_NAME="$REVIEW_FILE_NAME" \
SOCKET_PATH="$SOCKET_PATH" \
APP_DATA_DIR="$APP_DATA_DIR" \
APP_LOG="$APP_LOG" \
APP_PID="$APP_PID" \
KEEP_APP="$KEEP_APP" \
TMP_DIR="$TMP_DIR" \
python3 - <<'PY'
import json
import os
import sys

state = json.loads(os.environ["STATE_JSON"])
review = json.loads(os.environ["REVIEW_JSON"])
next_attention = json.loads(os.environ["NEXT_ATTENTION_JSON"])
codex_agents = json.loads(os.environ["CODEX_AGENTS_JSON"])
claude_agents = json.loads(os.environ["CLAUDE_AGENTS_JSON"])
codex_worktree_id = os.environ["CODEX_WORKTREE_ID"]
claude_worktree_id = os.environ["CLAUDE_WORKTREE_ID"]
review_file_name = os.environ["REVIEW_FILE_NAME"]

def fail(message: str) -> None:
    print(f"[smoke-acp] ERROR: {message}", file=sys.stderr)
    sys.exit(1)

def workdesk_for(worktree_id: str) -> dict:
    for desk in state["workdesks"]:
        if desk.get("worktree_id") == worktree_id:
            return desk
    fail(f"missing workdesk for {worktree_id}")

def session_for(agents: list, profile_id: str) -> dict:
    for agent in agents:
        if agent.get("provider_profile_id") == profile_id:
            return agent
    fail(f"missing {profile_id} agent session")

def pane_count(desk: dict, kind: str) -> int:
    return sum(1 for pane in desk["panes"] if pane["kind"] == kind)

codex_desk = workdesk_for(codex_worktree_id)
claude_desk = workdesk_for(claude_worktree_id)
codex_session = session_for(codex_agents, "codex")
claude_session = session_for(claude_agents, "claude-code")

if pane_count(codex_desk, "shell") < 1:
    fail("codex desk is missing a shell pane")
if pane_count(codex_desk, "agent") < 1:
    fail("codex desk is missing an agent pane")
if codex_session["lifecycle"] != "waiting" or codex_session["attention"] != "needs_review":
    fail("codex session did not reach waiting/needs_review")
if claude_session["lifecycle"] != "running" or claude_session["attention"] != "quiet":
    fail("claude-code baseline did not remain running/quiet")
if not review["summary"]["ready_for_review"]:
    fail("review summary did not become ready_for_review")

changed = set(review.get("changed_files", [])) | set(review.get("uncommitted_files", []))
if review_file_name not in changed:
    fail(f"review summary did not include {review_file_name}")

if next_attention.get("pane", {}).get("kind") != "agent":
    fail("next-attention did not land on an agent pane")
if next_attention.get("workdesk", {}).get("cwd") != codex_worktree_id:
    fail("next-attention did not target the codex worktree desk")

summary = [
    "[smoke-acp] ACP smoke demo passed.",
    f"[smoke-acp] Managed app pid: {os.environ['APP_PID']}",
    f"[smoke-acp] Socket: {os.environ['SOCKET_PATH']}",
    f"[smoke-acp] App data dir: {os.environ['APP_DATA_DIR']}",
    f"[smoke-acp] App log: {os.environ['APP_LOG']}",
    f"[smoke-acp] Codex worktree: {codex_worktree_id}",
    f"[smoke-acp] Claude worktree: {claude_worktree_id}",
    f"[smoke-acp] Codex attention: {codex_session['attention']} ({codex_session['lifecycle']})",
    f"[smoke-acp] Claude baseline: {claude_session['attention']} ({claude_session['lifecycle']})",
    f"[smoke-acp] Codex desk panes: shell={pane_count(codex_desk, 'shell')} agent={pane_count(codex_desk, 'agent')}",
    f"[smoke-acp] Desk rail snapshot: live_count={codex_desk['workdesk']['live_count']} highest_attention={codex_desk['workdesk']['attention']['highest']}",
    f"[smoke-acp] Review ready: {json.dumps(review['summary']['ready_for_review'])} changed={len(changed)}",
    f"[smoke-acp] Next attention pane: {next_attention['pane']['title']}",
]

if os.environ["KEEP_APP"] == "1":
    summary.append(
        f"[smoke-acp] Managed demo app is still running. Stop it with: kill {os.environ['APP_PID']} && rm -rf {os.environ['TMP_DIR']}"
    )
else:
    summary.append("[smoke-acp] Managed demo app will be cleaned up on exit.")

print("\n".join(summary))
PY
