#!/usr/bin/env bash

set -euo pipefail

XCODE_APP_PATH="/Applications/Xcode.app/Contents/Developer"

find_tool_path() {
  local tool="$1"

  if command -v "$tool" >/dev/null 2>&1; then
    command -v "$tool"
    return 0
  fi

  local candidates=()

  case "$tool" in
    brew)
      candidates=(/opt/homebrew/bin/brew /usr/local/bin/brew)
      ;;
    cargo|rustc)
      candidates=("$HOME/.cargo/bin/$tool" /opt/homebrew/bin/"$tool" /usr/local/bin/"$tool")
      ;;
    *)
      candidates=(/opt/homebrew/bin/"$tool" /usr/local/bin/"$tool")
      ;;
  esac

  for candidate in "${candidates[@]}"; do
    if [[ -x "$candidate" ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done

  return 1
}

doctor() {
  local missing=0
  local path=""
  local active_dev_dir=""
  local xcrun_error=""

  for tool in xcode-select git brew cargo rustc zig just; do
    path=""

    if path="$(find_tool_path "$tool")"; then
      if command -v "$tool" >/dev/null 2>&1; then
        printf '[ok] %s\n' "$tool"
      else
        printf '[not-on-path] %s -> %s\n' "$tool" "$path"
        missing=1
      fi
    else
      printf '[missing] %s\n' "$tool"
      missing=1
    fi
  done

  if command -v xcode-select >/dev/null 2>&1; then
    if active_dev_dir="$(xcode-select -p 2>/dev/null)"; then
      printf '[ok] active developer directory -> %s\n' "$active_dev_dir"
    else
      printf '[missing] active developer directory\n'
      missing=1
    fi
  fi

  if [[ -d "$XCODE_APP_PATH" ]]; then
    printf '[ok] Xcode.app\n'
  else
    printf '[missing] Xcode.app\n'
    missing=1
  fi

  if [[ -d "$XCODE_APP_PATH" ]]; then
    if xcrun_error="$(DEVELOPER_DIR="$XCODE_APP_PATH" xcrun -f metal 2>&1 >/dev/null)"; then
      printf '[ok] metal compiler available\n'
    else
      if [[ "$xcrun_error" == *"license agreements"* ]]; then
        printf '[missing] Xcode license accepted\n'
      else
        printf '[missing] metal compiler available\n'
      fi
      missing=1
    fi
  fi

  if [[ "$missing" -ne 0 ]]; then
    cat <<'EOF'

Install the missing tools, then re-run:
  just doctor

Recommended base packages:
  brew install git cmake ninja pkg-config just rustup-init

If Homebrew is installed but not on PATH, add:
  eval "$(/opt/homebrew/bin/brew shellenv)"

If GPUI builds fail on macOS:
  1. Ensure /Applications/Xcode.app is installed.
  2. Run sudo xcodebuild -license accept
  3. Optionally run sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer
EOF
    return 1
  fi
}

main() {
  case "${1:-}" in
    --doctor)
      doctor
      ;;
    *)
      cat <<'EOF'
Canvas bootstrap helper

Usage:
  bash scripts/bootstrap-macos.sh --doctor
EOF
      ;;
  esac
}

main "${1:-}"
