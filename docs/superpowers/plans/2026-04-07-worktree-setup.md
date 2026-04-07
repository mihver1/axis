# Worktree Setup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the broken ghostty submodule and create an idempotent setup script so that fresh git worktrees (especially those created by Conductor) are immediately buildable.

**Architecture:** Two changes — (1) add `.gitmodules` to properly declare the `vendor/ghostty` submodule, (2) create `scripts/setup-worktree.sh` that initializes the submodule (with `--reference` acceleration from the parent repo when available) and optionally warms `cargo fetch`. The justfile gets a `setup` recipe as the entry point.

**Tech Stack:** Bash, Git submodules, Just

---

## File Map

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `.gitmodules` | Declare vendor/ghostty submodule with remote URL |
| Create | `scripts/setup-worktree.sh` | Idempotent worktree bootstrap: submodule init, cargo fetch, doctor |
| Modify | `justfile` | Add `setup` recipe |
| Modify | `README.md` | Document the setup flow for worktrees |

---

### Task 1: Add `.gitmodules`

**Files:**
- Create: `.gitmodules`

The submodule is already in the git index at `vendor/ghostty` (mode 160000, commit `7114721bd4323c776b4c93b0afd313c4785b98b3`), but git has no `.gitmodules` to know the remote URL. Adding this file fixes `git submodule init` / `git submodule update`.

- [ ] **Step 1: Create `.gitmodules`**

```ini
[submodule "vendor/ghostty"]
	path = vendor/ghostty
	url = https://github.com/ghostty-org/ghostty.git
```

- [ ] **Step 2: Register the submodule in git config**

Run:
```bash
git submodule init
```

Expected: output like `Submodule 'vendor/ghostty' (https://github.com/ghostty-org/ghostty.git) registered for path 'vendor/ghostty'`

- [ ] **Step 3: Verify submodule status**

Run:
```bash
git submodule status
```

Expected: output showing `-7114721bd4323c776b4c93b0afd313c4785b98b3 vendor/ghostty` (the `-` prefix means not yet checked out, which is fine — we'll populate it later via the setup script).

- [ ] **Step 4: Stage `.gitmodules`**

Run:
```bash
git add .gitmodules
```

- [ ] **Step 5: Commit**

```bash
git commit -m "Add .gitmodules for vendor/ghostty submodule

The submodule gitlink was in the index but .gitmodules was missing,
making it impossible to initialize ghostty in worktrees."
```

---

### Task 2: Create `scripts/setup-worktree.sh`

**Files:**
- Create: `scripts/setup-worktree.sh`

Idempotent script that ensures the worktree is ready to build. Designed for Conductor's workspace setup hook — fast, quiet on success, informative on failure.

- [ ] **Step 1: Create the script**

```bash
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
    # Navigate from .git/worktrees/<name> -> .git -> parent repo
    main_repo_dir="$(cd "$main_git_dir/../.." && pwd)"
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
```

- [ ] **Step 2: Make it executable**

Run:
```bash
chmod +x scripts/setup-worktree.sh
```

- [ ] **Step 3: Test — dry run on the current worktree (ghostty missing)**

Run from the bismarck worktree root:
```bash
bash scripts/setup-worktree.sh --quick
```

Expected:
- Should detect that `vendor/ghostty/build.zig` is missing
- Should find the reference repo at `/Users/mihver/Projects/axis/vendor/ghostty`
- Should run `git submodule update --init --reference ...`
- Should print `[ok] vendor/ghostty initialized`

- [ ] **Step 4: Test — idempotency (run again)**

Run:
```bash
bash scripts/setup-worktree.sh --quick
```

Expected: `[ok] vendor/ghostty already populated` — no work done.

- [ ] **Step 5: Verify build works**

Run:
```bash
just check
```

Expected: `cargo check --workspace` passes without the `vendor/ghostty is missing` panic.

- [ ] **Step 6: Stage and commit**

```bash
git add scripts/setup-worktree.sh
git commit -m "Add worktree setup script

Idempotent script that initializes the ghostty submodule (with
--reference acceleration from the parent repo) and optionally
warms the cargo dependency cache. Designed for Conductor workspace
setup."
```

---

### Task 3: Add `setup` recipe to justfile

**Files:**
- Modify: `justfile`

- [ ] **Step 1: Add the `setup` recipe after the `default` recipe**

Add after line 2 of the justfile (`@just --list`):

```just
setup *FLAGS:
    bash scripts/setup-worktree.sh {{FLAGS}}
```

- [ ] **Step 2: Test the recipe**

Run:
```bash
just setup --quick
```

Expected: `[ok] vendor/ghostty already populated` (idempotent).

- [ ] **Step 3: Stage and commit**

```bash
git add justfile
git commit -m "Add 'just setup' recipe for worktree initialization"
```

---

### Task 4: Update README with worktree setup docs

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add a "Worktree Setup" section after "Quick Start"**

Insert after the Quick Start section (after line 65, before "## Full Local Loop"):

```markdown
## Worktree Setup

If you are working in a git worktree (e.g. created by Conductor or `git worktree add`),
run the setup script first to initialize vendored dependencies:

```bash
just setup
```

This initializes the `vendor/ghostty` submodule (using the parent repo as a local
cache for speed) and fetches Cargo dependencies. The script is idempotent — safe to
re-run at any time.

For a faster setup that skips `cargo fetch`:

```bash
just setup --quick
```
```

- [ ] **Step 2: Add `just setup` to the Useful Commands table**

In the "Useful Commands" code block (around line 107), add as the first entry:

```
just setup      # initialize worktree (submodules + cargo fetch)
```

- [ ] **Step 3: Stage and commit**

```bash
git add README.md
git commit -m "Document worktree setup in README"
```

---

### Task 5: End-to-end verification

- [ ] **Step 1: Run `just doctor`**

Run:
```bash
just doctor
```

Expected: all checks pass.

- [ ] **Step 2: Run `just check`**

Run:
```bash
just check
```

Expected: `cargo check --workspace` passes.

- [ ] **Step 3: Run `just test`**

Run:
```bash
just test
```

Expected: all tests pass.

- [ ] **Step 4: Verify git status is clean**

Run:
```bash
git status
git log --oneline -5
```

Expected: clean working tree, 4 new commits (`.gitmodules`, setup script, justfile, README).
