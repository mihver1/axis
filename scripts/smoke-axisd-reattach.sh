#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: bash scripts/smoke-axisd-reattach.sh [--keep-processes]

Manual smoke test for daemon-backed terminal reattach:
1. builds `axisd`, `axis-app`, and `axis-cli`;
2. starts `axisd` on a temp user-level socket/data dir;
3. starts `axis-app` with a temp app-data dir;
4. discovers the persisted first shell surface from `session.json`;
5. writes to the daemon-owned terminal, kills `axis-app`, keeps writing while the GUI is down;
6. relaunches `axis-app` and verifies the same daemon terminal session id is reused.
EOF
}

log() {
  printf '[smoke-axisd-reattach] %s\n' "$*"
}

fail() {
  printf '[smoke-axisd-reattach] ERROR: %s\n' "$*" >&2
  exit 1
}

KEEP_PROCESSES=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --keep-processes)
      KEEP_PROCESSES=1
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

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TMP_DIR="$(mktemp -d -t axisd-reattach-smoke)"
DAEMON_DATA_DIR="$TMP_DIR/daemon-data"
APP_DATA_DIR="$TMP_DIR/app-data"
SOCKET_PATH="$TMP_DIR/axisd.sock"
DAEMON_LOG="$TMP_DIR/axisd.log"
APP_LOG="$TMP_DIR/axis-app.log"
SESSION_PATH="$APP_DATA_DIR/session.json"
DAEMON_PID=""
APP_PID=""

cleanup() {
  local exit_code=$?
  trap - EXIT
  if [[ $KEEP_PROCESSES -eq 0 ]]; then
    if [[ -n "${APP_PID:-}" ]] && kill -0 "$APP_PID" 2>/dev/null; then
      kill "$APP_PID" 2>/dev/null || true
      wait "$APP_PID" 2>/dev/null || true
    fi
    if [[ -n "${DAEMON_PID:-}" ]] && kill -0 "$DAEMON_PID" 2>/dev/null; then
      kill "$DAEMON_PID" 2>/dev/null || true
      wait "$DAEMON_PID" 2>/dev/null || true
    fi
    rm -rf "$TMP_DIR"
  else
    log "keeping processes and temp dir at $TMP_DIR"
  fi
  exit "$exit_code"
}
trap cleanup EXIT

cargo_target_dir() {
  if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
    printf '%s\n' "$CARGO_TARGET_DIR"
    return 0
  fi

  cargo metadata --format-version 1 --no-deps \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])'
}

json_field() {
  local path="$1"
  python3 -c '
import json
import sys

value = json.load(sys.stdin)
try:
    for part in [p for p in sys.argv[1].split(".") if p]:
        value = value[int(part)] if part.isdigit() else value[part]
except (IndexError, KeyError, TypeError, ValueError):
    print("")
    raise SystemExit(0)
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

string_to_byte_array() {
  python3 -c 'import json,sys; print(json.dumps(list(sys.argv[1].encode())))' "$1"
}

axis_cli() {
  "$CLI_BIN" --socket "$SOCKET_PATH" "$@"
}

wait_for_daemon() {
  local attempt
  for attempt in $(seq 1 80); do
    if axis_cli raw daemon.health '{}' >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done
  [[ -f "$DAEMON_LOG" ]] && sed -n '1,200p' "$DAEMON_LOG" >&2 || true
  fail "axisd did not become ready on $SOCKET_PATH"
}

wait_for_session_file() {
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
  [[ -f "$APP_LOG" ]] && sed -n '1,200p' "$APP_LOG" >&2 || true
  fail "axis-app did not persist $SESSION_PATH"
}

read_shell_binding() {
  python3 - "$SESSION_PATH" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as handle:
    session = json.load(handle)

for desk in session.get("workdesks", []):
    workdesk_id = desk.get("workdesk_id", "").strip()
    if not workdesk_id:
        continue
    for pane in desk.get("panes", []):
        for surface in pane.get("surfaces", []):
            if surface.get("kind") == "shell":
                print(workdesk_id)
                print(surface["id"])
                sys.exit(0)

raise SystemExit("no persisted shell surface found in session.json")
PY
}

ensure_terminal() {
  local workdesk_id="$1"
  local surface_id="$2"
  local ensure_json
  ensure_json="$(axis_cli raw terminal.ensure "$(python3 -c 'import json,sys; print(json.dumps({
    "workdesk_id": sys.argv[1],
    "surface_id": int(sys.argv[2]),
    "kind": "shell",
    "title": "Shell",
    "cwd": sys.argv[3],
    "cols": 100,
    "rows": 28,
}))' "$workdesk_id" "$surface_id" "$WORKSPACE_ROOT")")"
  printf '%s\n' "$ensure_json"
}

write_terminal_text() {
  local terminal_session_id="$1"
  local text="$2"
  local bytes_json
  bytes_json="$(string_to_byte_array "$text")"
  axis_cli raw terminal.write "$(python3 -c 'import json,sys; print(json.dumps({
    "terminal_session_id": sys.argv[1],
    "bytes": json.loads(sys.argv[2]),
}))' "$terminal_session_id" "$bytes_json")" >/dev/null
}

wait_for_terminal_text() {
  local terminal_session_id="$1"
  local offset="$2"
  local needle="$3"
  local attempt
  local response
  local next_offset
  for attempt in $(seq 1 60); do
    response="$(axis_cli raw terminal.read "$(python3 -c 'import json,sys; print(json.dumps({
      "terminal_session_id": sys.argv[1],
      "offset": int(sys.argv[2]),
    }))' "$terminal_session_id" "$offset")")"
    next_offset="$(printf '%s\n' "$response" | json_field chunk.offset)"
    local chunk_bytes
    chunk_bytes="$(printf '%s\n' "$response" | json_field chunk.bytes)"
    if [[ -n "$next_offset" ]] && [[ "$chunk_bytes" != "" ]]; then
      local text
      local byte_count
      text="$(python3 -c 'import json,sys; print(bytes(json.loads(sys.argv[1])).decode("utf-8", "ignore"))' "$chunk_bytes")"
      byte_count="$(python3 -c 'import json,sys; print(len(json.loads(sys.argv[1])))' "$chunk_bytes")"
      offset="$((next_offset + byte_count))"
      if [[ "$text" == *"$needle"* ]]; then
        printf '%s\n' "$offset"
        return 0
      fi
    fi
    sleep 0.1
  done
  fail "terminal transcript never included marker: $needle"
}

TARGET_DIR="$(cargo_target_dir)"
DAEMON_BIN="$TARGET_DIR/debug/axisd"
APP_BIN="$TARGET_DIR/debug/axis-app"
CLI_BIN="$TARGET_DIR/debug/axis"

log "building binaries"
cargo build -p axisd -p axis-app -p axis-cli >/dev/null

log "starting axisd on $SOCKET_PATH"
AXIS_SOCKET_PATH="$SOCKET_PATH" \
AXIS_DAEMON_DATA_DIR="$DAEMON_DATA_DIR" \
  "$DAEMON_BIN" >"$DAEMON_LOG" 2>&1 &
DAEMON_PID=$!
wait_for_daemon

log "starting axis-app with temp app data dir"
AXIS_SOCKET_PATH="$SOCKET_PATH" \
AXIS_DAEMON_DATA_DIR="$DAEMON_DATA_DIR" \
AXIS_APP_DATA_DIR="$APP_DATA_DIR" \
  "$APP_BIN" >"$APP_LOG" 2>&1 &
APP_PID=$!
wait_for_session_file

SHELL_BINDING="$(read_shell_binding)"
WORKDESK_ID="${SHELL_BINDING%%$'\n'*}"
SURFACE_ID="${SHELL_BINDING#*$'\n'}"
[[ -n "$WORKDESK_ID" && -n "$SURFACE_ID" && "$SURFACE_ID" != "$SHELL_BINDING" ]] \
  || fail "expected workdesk_id and surface_id from session.json"
log "using persisted shell surface workdesk_id=$WORKDESK_ID surface_id=$SURFACE_ID"

ENSURE_JSON="$(ensure_terminal "$WORKDESK_ID" "$SURFACE_ID")"
TERMINAL_SESSION_ID="$(printf '%s\n' "$ENSURE_JSON" | json_field terminal_session_id)"
[[ -n "$TERMINAL_SESSION_ID" ]] || fail "terminal.ensure did not return terminal_session_id"
log "attached daemon terminal session $TERMINAL_SESSION_ID"

OFFSET=0
write_terminal_text "$TERMINAL_SESSION_ID" $'printf "smoke-before-restart\\n"\n'
OFFSET="$(wait_for_terminal_text "$TERMINAL_SESSION_ID" "$OFFSET" "smoke-before-restart")"

log "killing axis-app pid=$APP_PID while keeping axisd alive"
kill "$APP_PID"
wait "$APP_PID" 2>/dev/null || true
APP_PID=""

write_terminal_text "$TERMINAL_SESSION_ID" $'printf "smoke-while-gui-down\\n"\n'
OFFSET="$(wait_for_terminal_text "$TERMINAL_SESSION_ID" "$OFFSET" "smoke-while-gui-down")"

log "relaunching axis-app"
AXIS_SOCKET_PATH="$SOCKET_PATH" \
AXIS_DAEMON_DATA_DIR="$DAEMON_DATA_DIR" \
AXIS_APP_DATA_DIR="$APP_DATA_DIR" \
  "$APP_BIN" >>"$APP_LOG" 2>&1 &
APP_PID=$!
sleep 2

ENSURE_AFTER_RESTART_JSON="$(ensure_terminal "$WORKDESK_ID" "$SURFACE_ID")"
TERMINAL_SESSION_ID_AFTER_RESTART="$(printf '%s\n' "$ENSURE_AFTER_RESTART_JSON" | json_field terminal_session_id)"
[[ "$TERMINAL_SESSION_ID_AFTER_RESTART" == "$TERMINAL_SESSION_ID" ]] \
  || fail "expected same terminal session id after relaunch, got $TERMINAL_SESSION_ID_AFTER_RESTART"

write_terminal_text "$TERMINAL_SESSION_ID" $'printf "smoke-after-restart\\n"\n'
OFFSET="$(wait_for_terminal_text "$TERMINAL_SESSION_ID" "$OFFSET" "smoke-after-restart")"

log "reattach smoke passed"
log "daemon socket: $SOCKET_PATH"
log "app log: $APP_LOG"
log "daemon log: $DAEMON_LOG"
