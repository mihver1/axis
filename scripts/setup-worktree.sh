#!/usr/bin/env bash
#
# setup-worktree.sh — make a fresh worktree build-ready.
#
# Idempotent. Safe to re-run. Designed for agentic IDE workspace setup
# (Conductor, Claude Code, etc.) but works fine when invoked manually.
#
# Usage:
#   bash scripts/setup-worktree.sh          # full setup
#   bash scripts/setup-worktree.sh --quick  # submodule only, skip cargo fetch

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
VENDOR_GHOSTTY="$REPO_DIR/vendor/ghostty"
QUICK=false

for arg in "$@"; do
  case "$arg" in
    --quick) QUICK=true ;;
    --help|-h)
      echo "Usage: bash scripts/setup-worktree.sh [--quick]"
      echo "  --quick  submodule init only, skip cargo fetch"
      exit 0
      ;;
    *)
      echo "Unknown argument: $arg" >&2
      exit 1
      ;;
  esac
done

# ---------- Step 1: Ghostty submodule ----------

if [[ -f "$VENDOR_GHOSTTY/build.zig" ]]; then
  echo "[ok] vendor/ghostty already populated"
else
  echo "[setup] Initializing vendor/ghostty submodule..."

  cd "$REPO_DIR"
  git submodule init

  # Try to find a reference repo for faster clone.
  # In a worktree, the main repo's vendor/ghostty may already have objects.
  reference_repo=""

  # Check the parent git dir for a sibling worktree or main checkout
  if [[ -f "$REPO_DIR/.git" ]]; then
    # We are a worktree — .git is a file with "gitdir: <path>"
    main_git_dir="$(sed 's/^gitdir: //' "$REPO_DIR/.git")"
    # Navigate from .git/worktrees/<name> -> repo root
    # main_git_dir is e.g. /repo/.git/worktrees/bismarck
    main_repo_dir="$(cd "$main_git_dir/../../.." && pwd)"
    candidate="$main_repo_dir/vendor/ghostty"
    if [[ -d "$candidate/.git" ]]; then
      reference_repo="$candidate"
    fi
  fi

  if [[ -n "$reference_repo" ]]; then
    echo "[setup] Using reference repo at $reference_repo for faster clone"
    git submodule update --init --reference "$reference_repo" vendor/ghostty
  else
    echo "[setup] Cloning vendor/ghostty from remote (no local reference found)"
    git submodule update --init vendor/ghostty
  fi

  if [[ -f "$VENDOR_GHOSTTY/build.zig" ]]; then
    echo "[ok] vendor/ghostty initialized"
  else
    echo "[FAIL] vendor/ghostty/build.zig not found after submodule update" >&2
    exit 1
  fi
fi

# ---------- Step 2: Cargo fetch (warm dependency cache) ----------

if [[ "$QUICK" == false ]]; then
  echo "[setup] Running cargo fetch..."
  cd "$REPO_DIR"
  cargo fetch --quiet
  echo "[ok] Cargo dependencies fetched"
fi

echo ""
echo "Worktree is ready. Run 'just doctor' to verify the full toolchain."
