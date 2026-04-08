# Track B: Editor Evolution — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Evolve the axis editor from basic text editing to a capable code editor (level B: find-replace, tabs, file picker, multi-buffer) with architecture ready for LSP integration.

**Architecture:** Replace `EditorBuffer`'s line-based String model with rope-based buffer (`ropey`), add operation-based undo, tree-sitter highlighting, and essential editor UI. All changes in `crates/axis-editor` and editor-related code in `apps/axis-app/src/main.rs`.

**Tech Stack:** Rust, ropey, tree-sitter, GPUI 0.2.2, axis-editor crate

---

## Task 1: Add ropey dependency and create Rope-backed buffer

**Files:**
- Modify: `crates/axis-editor/Cargo.toml`
- Modify: `crates/axis-editor/src/lib.rs` (lines 73-91: EditorBuffer struct, 94-132: constructors)
- Create: `crates/axis-editor/tests/rope_buffer.rs`

- [ ] **Step 1: Add ropey dependency**

Add to `Cargo.toml` workspace root:
```toml
[workspace.dependencies]
ropey = "1"
```

Add to `crates/axis-editor/Cargo.toml`:
```toml
ropey.workspace = true
```

- [ ] **Step 2: Write failing tests for rope buffer**

Create `crates/axis-editor/tests/rope_buffer.rs`:

```rust
use axis_editor::EditorBuffer;
use std::path::PathBuf;

fn make_buffer(text: &str) -> EditorBuffer {
    EditorBuffer::from_text(PathBuf::from("/tmp/test.rs"), text.to_string())
}

#[test]
fn line_count_matches_text() {
    let buf = make_buffer("line1\nline2\nline3\n");
    assert_eq!(buf.line_count(), 3);
}

#[test]
fn line_text_returns_correct_content() {
    let buf = make_buffer("hello\nworld\n");
    assert_eq!(buf.line_text(0), "hello");
    assert_eq!(buf.line_text(1), "world");
}

#[test]
fn insert_text_updates_content() {
    let mut buf = make_buffer("hello world");
    buf.move_to_offset(5, false);
    buf.replace_selection(",");
    assert_eq!(buf.text(), "hello, world");
}

#[test]
fn document_version_increments_on_edit() {
    let mut buf = make_buffer("hello");
    let v0 = buf.document_version();
    buf.replace_selection("x");
    let v1 = buf.document_version();
    assert!(v1 > v0, "version should increment on edit");
}

#[test]
fn text_delta_produced_on_edit() {
    let mut buf = make_buffer("hello");
    buf.move_to_offset(5, false);
    buf.replace_selection(" world");
    let deltas = buf.take_pending_deltas();
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].range.start, 5);
    assert_eq!(deltas[0].range.end, 5);
    assert_eq!(deltas[0].text, " world");
}

#[test]
fn undo_restores_previous_state() {
    let mut buf = make_buffer("hello");
    buf.select_all();
    buf.replace_selection("goodbye");
    assert_eq!(buf.text(), "goodbye");
    buf.undo();
    assert_eq!(buf.text(), "hello");
}

#[test]
fn large_file_line_count() {
    let text: String = (0..10000).map(|i| format!("line {i}\n")).collect();
    let buf = make_buffer(&text);
    assert_eq!(buf.line_count(), 10000);
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p axis-editor --test rope_buffer 2>&1`
Expected: Compilation errors — `from_text`, `document_version`, `take_pending_deltas`, `move_to_offset` don't exist yet.

- [ ] **Step 4: Replace String with Rope in EditorBuffer**

In `crates/axis-editor/src/lib.rs`, replace the core data structure:

```rust
use ropey::Rope;

/// A text change delta for LSP incremental sync.
#[derive(Clone, Debug)]
pub struct TextDelta {
    /// Byte range in the old text that was replaced.
    pub range: std::ops::Range<usize>,
    /// The replacement text.
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct EditorBuffer {
    path: PathBuf,
    rope: Rope,
    saved_rope: Rope,
    dirty: bool,
    selection: Selection,
    marked_range: Option<Range<usize>>,
    preferred_column: Option<usize>,
    scroll_top_line: usize,
    search: SearchState,
    line_highlight_cache: RefCell<Vec<Option<Vec<HighlightSpan>>>>,
    language: LanguageKind,
    undo_stack: Vec<UndoEntry>,
    redo_stack: Vec<UndoEntry>,
    last_synced_modified_at: Option<SystemTime>,
    external_modified: bool,
    document_version: u64,
    pending_deltas: Vec<TextDelta>,
}

#[derive(Clone, Debug)]
struct UndoEntry {
    /// The inverse delta to apply for undo.
    delta: TextDelta,
    /// Selection state before the edit.
    selection: Selection,
}
```

- [ ] **Step 5: Update constructors**

Replace `load` and `restore` (lines 94-132):

```rust
impl EditorBuffer {
    pub fn load(path: PathBuf) -> Result<Self, String> {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        Ok(Self::from_text(path, content))
    }

    pub fn from_text(path: PathBuf, text: String) -> Self {
        let language = detect_language(&path);
        let rope = Rope::from_str(&text);
        let saved_rope = rope.clone();
        Self {
            path,
            rope,
            saved_rope,
            dirty: false,
            selection: Selection::default(),
            marked_range: None,
            preferred_column: None,
            scroll_top_line: 0,
            search: SearchState::default(),
            line_highlight_cache: RefCell::new(Vec::new()),
            language,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_synced_modified_at: None,
            external_modified: false,
            document_version: 0,
            pending_deltas: Vec::new(),
        }
    }
}
```

- [ ] **Step 6: Update core accessors to use Rope**

Replace line-based accessors:

```rust
pub fn text(&self) -> String {
    self.rope.to_string()
}

pub fn line_count(&self) -> usize {
    // ropey counts a trailing newline as an extra empty line; compensate
    let len = self.rope.len_lines();
    if len > 0 && self.rope.len_chars() > 0 && self.rope.char(self.rope.len_chars() - 1) == '\n' {
        len - 1
    } else {
        len.max(1)
    }
}

pub fn line_text(&self, line_index: usize) -> String {
    if line_index >= self.rope.len_lines() {
        return String::new();
    }
    let line = self.rope.line(line_index);
    let s = line.to_string();
    // Strip trailing newline for display
    s.trim_end_matches('\n').to_string()
}

pub fn line_range(&self, line_index: usize) -> Range<usize> {
    if line_index >= self.rope.len_lines() {
        let end = self.rope.len_bytes();
        return end..end;
    }
    let start = self.rope.line_to_byte(line_index);
    let end = if line_index + 1 < self.rope.len_lines() {
        self.rope.line_to_byte(line_index + 1)
    } else {
        self.rope.len_bytes()
    };
    start..end
}

pub fn offset_for_line_col(&self, line: usize, col: usize) -> usize {
    if line >= self.rope.len_lines() {
        return self.rope.len_bytes();
    }
    let line_start = self.rope.line_to_byte(line);
    let line_len = self.line_text(line).len();
    line_start + col.min(line_len)
}

pub fn line_col_for_offset(&self, offset: usize) -> (usize, usize) {
    let offset = offset.min(self.rope.len_bytes());
    let line = self.rope.byte_to_line(offset);
    let line_start = self.rope.line_to_byte(line);
    (line, offset - line_start)
}

pub fn document_version(&self) -> u64 {
    self.document_version
}

pub fn take_pending_deltas(&mut self) -> Vec<TextDelta> {
    std::mem::take(&mut self.pending_deltas)
}
```

- [ ] **Step 7: Update replace_selection to use Rope and produce deltas**

```rust
pub fn replace_selection(&mut self, new_text: &str) {
    let range = self.selection.range.clone();
    let old_text = self.rope.byte_slice(range.clone()).to_string();

    // Record undo entry (inverse delta)
    self.undo_stack.push(UndoEntry {
        delta: TextDelta {
            range: range.start..(range.start + new_text.len()),
            text: old_text,
        },
        selection: self.selection.clone(),
    });
    self.redo_stack.clear();

    // Record forward delta for LSP sync
    self.pending_deltas.push(TextDelta {
        range: range.clone(),
        text: new_text.to_string(),
    });

    // Apply edit to rope
    let start_char = self.rope.byte_to_char(range.start);
    let end_char = self.rope.byte_to_char(range.end);
    self.rope.remove(start_char..end_char);
    if !new_text.is_empty() {
        self.rope.insert(start_char, new_text);
    }

    // Update state
    let new_cursor = range.start + new_text.len();
    self.selection = Selection {
        range: new_cursor..new_cursor,
        reversed: false,
    };
    self.preferred_column = None;
    self.dirty = self.rope.to_string() != self.saved_rope.to_string();
    self.document_version += 1;
    self.invalidate_highlight_cache();
}
```

- [ ] **Step 8: Update undo/redo to use operation-based approach**

```rust
pub fn undo(&mut self) -> bool {
    let Some(entry) = self.undo_stack.pop() else {
        return false;
    };

    // Apply the inverse delta
    let range = entry.delta.range.clone();
    let current_text = self.rope.byte_slice(range.clone()).to_string();

    let start_char = self.rope.byte_to_char(range.start);
    let end_char = self.rope.byte_to_char(range.end);
    self.rope.remove(start_char..end_char);
    if !entry.delta.text.is_empty() {
        self.rope.insert(start_char, &entry.delta.text);
    }

    // Push redo entry
    self.redo_stack.push(UndoEntry {
        delta: TextDelta {
            range: range.start..(range.start + entry.delta.text.len()),
            text: current_text,
        },
        selection: self.selection.clone(),
    });

    self.selection = entry.selection;
    self.dirty = self.rope.to_string() != self.saved_rope.to_string();
    self.document_version += 1;
    self.pending_deltas.push(TextDelta {
        range,
        text: entry.delta.text.clone(),
    });
    self.invalidate_highlight_cache();
    true
}

pub fn redo(&mut self) -> bool {
    let Some(entry) = self.redo_stack.pop() else {
        return false;
    };

    let range = entry.delta.range.clone();
    let current_text = self.rope.byte_slice(range.clone()).to_string();

    let start_char = self.rope.byte_to_char(range.start);
    let end_char = self.rope.byte_to_char(range.end);
    self.rope.remove(start_char..end_char);
    if !entry.delta.text.is_empty() {
        self.rope.insert(start_char, &entry.delta.text);
    }

    self.undo_stack.push(UndoEntry {
        delta: TextDelta {
            range: range.start..(range.start + entry.delta.text.len()),
            text: current_text,
        },
        selection: self.selection.clone(),
    });

    self.selection = entry.selection;
    self.dirty = self.rope.to_string() != self.saved_rope.to_string();
    self.document_version += 1;
    self.invalidate_highlight_cache();
    true
}
```

- [ ] **Step 9: Update save/reload**

```rust
pub fn save(&mut self) -> Result<(), String> {
    let text = self.rope.to_string();
    std::fs::write(&self.path, &text).map_err(|e| e.to_string())?;
    self.saved_rope = self.rope.clone();
    self.dirty = false;
    self.external_modified = false;
    self.last_synced_modified_at = file_modified_at(&self.path);
    Ok(())
}

pub fn reload(&mut self) -> Result<(), String> {
    let text = std::fs::read_to_string(&self.path).map_err(|e| e.to_string())?;
    self.rope = Rope::from_str(&text);
    self.saved_rope = self.rope.clone();
    self.dirty = false;
    self.selection = Selection::default();
    self.search = SearchState::default();
    self.undo_stack.clear();
    self.redo_stack.clear();
    self.document_version += 1;
    self.invalidate_highlight_cache();
    self.last_synced_modified_at = file_modified_at(&self.path);
    self.external_modified = false;
    Ok(())
}
```

- [ ] **Step 10: Update remaining methods that use self.text or line_starts**

All movement methods (`move_left`, `move_right`, `move_up`, `move_down`, `move_home`, `move_end`, `page_up`, `page_down`), `backspace`, `delete_forward`, `insert_newline`, `insert_tab`, `selected_text`, `cursor_offset`, `select_all` — update them to use `self.rope` instead of `self.text` and `self.line_starts`.

Key conversions:
- `self.text.len()` → `self.rope.len_bytes()`
- `self.text[range].to_string()` → `self.rope.byte_slice(range).to_string()`
- `self.line_starts` → removed, use `self.rope.line_to_byte()`
- `rebuild_line_starts()` → removed entirely (rope maintains this)

Also update `compute_search_matches` to work with `self.rope.to_string()` (acceptable since search still does full text scan).

- [ ] **Step 11: Update highlight_line cache invalidation**

```rust
fn invalidate_highlight_cache(&self) {
    self.line_highlight_cache.borrow_mut().clear();
}
```

The cache should be resized lazily in `highlight_line`:
```rust
pub fn highlight_line(&self, line_index: usize) -> Vec<HighlightSpan> {
    let mut cache = self.line_highlight_cache.borrow_mut();
    if cache.len() <= line_index {
        cache.resize(line_index + 1, None);
    }
    if let Some(cached) = &cache[line_index] {
        return cached.clone();
    }
    let text = self.line_text(line_index);
    let spans = compute_line_highlights(&text, self.language);
    cache[line_index] = Some(spans.clone());
    spans
}
```

- [ ] **Step 12: Run tests**

Run: `cargo test -p axis-editor 2>&1`
Expected: All tests pass including the new rope_buffer tests.

Run: `cargo build -p axis-app 2>&1 | head -30`
Expected: Clean build (main.rs should compile without changes since we preserved the public API).

- [ ] **Step 13: Run full test suite**

Run: `cargo test --workspace 2>&1`
Expected: All tests pass.

- [ ] **Step 14: Commit**

```bash
git add -A && git commit -m "feat: replace line-based buffer with ropey Rope

EditorBuffer now uses ropey::Rope instead of String + line_starts.
Adds document_version, TextDelta for LSP incremental sync, and
operation-based undo/redo (no more full-text snapshots)."
```

---

## Task 2: Add find-replace to editor

**Files:**
- Modify: `crates/axis-editor/src/lib.rs` (SearchState, replace methods)
- Modify: `apps/axis-app/src/main.rs` (search bar UI ~lines 8089-8122, key handler ~lines 7121-7155)

- [ ] **Step 1: Extend SearchState with replace fields**

In `crates/axis-editor/src/lib.rs`, update SearchState:

```rust
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchState {
    pub open: bool,
    pub query: String,
    pub active_match: Option<usize>,
    pub replace_open: bool,
    pub replace_text: String,
    pub case_sensitive: bool,
    matches: Vec<Range<usize>>,
}
```

- [ ] **Step 2: Add replace methods to EditorBuffer**

```rust
pub fn open_replace(&mut self) {
    self.search.open = true;
    self.search.replace_open = true;
}

pub fn toggle_case_sensitivity(&mut self) {
    self.search.case_sensitive = !self.search.case_sensitive;
    self.recompute_search_matches();
}

pub fn set_replace_text(&mut self, text: String) {
    self.search.replace_text = text;
}

pub fn append_replace_text(&mut self, ch: &str) {
    self.search.replace_text.push_str(ch);
}

pub fn pop_replace_text(&mut self) {
    self.search.replace_text.pop();
}

/// Replace the current match with the replacement text and advance to next.
pub fn replace_current_match(&mut self) -> bool {
    let Some(match_index) = self.search.active_match else {
        return false;
    };
    let Some(match_range) = self.search.matches.get(match_index).cloned() else {
        return false;
    };

    // Select the match range
    self.selection = Selection {
        range: match_range,
        reversed: false,
    };
    // Replace it
    self.replace_selection(&self.search.replace_text.clone());
    // Recompute matches
    self.recompute_search_matches();
    // Snap to next match after replacement point
    if !self.search.matches.is_empty() {
        let cursor = self.cursor_offset();
        self.search.active_match = Some(
            self.search
                .matches
                .iter()
                .position(|m| m.start >= cursor)
                .unwrap_or(0),
        );
    }
    true
}

/// Replace all matches with the replacement text.
pub fn replace_all_matches(&mut self) -> usize {
    let replace = self.search.replace_text.clone();
    let mut count = 0;
    // Replace from end to start to preserve offsets
    while let Some(match_range) = self.search.matches.last().cloned() {
        self.selection = Selection {
            range: match_range,
            reversed: false,
        };
        self.replace_selection(&replace);
        count += 1;
        self.recompute_search_matches();
    }
    count
}
```

- [ ] **Step 3: Update recompute_search_matches for case sensitivity**

```rust
fn recompute_search_matches(&mut self) {
    if self.search.query.is_empty() {
        self.search.matches.clear();
        self.search.active_match = None;
        return;
    }

    let text = self.rope.to_string();
    self.search.matches = if self.search.case_sensitive {
        compute_search_matches_exact(&text, &self.search.query)
    } else {
        compute_search_matches_case_insensitive(&text, &self.search.query)
    };

    // Adjust active match
    if self.search.matches.is_empty() {
        self.search.active_match = None;
    } else if let Some(idx) = self.search.active_match {
        if idx >= self.search.matches.len() {
            self.search.active_match = Some(0);
        }
    }
}

fn compute_search_matches_exact(text: &str, query: &str) -> Vec<Range<usize>> {
    let mut matches = Vec::new();
    let mut start = 0;
    while let Some(pos) = text[start..].find(query) {
        let abs_start = start + pos;
        matches.push(abs_start..abs_start + query.len());
        start = abs_start + query.len();
    }
    matches
}

fn compute_search_matches_case_insensitive(text: &str, query: &str) -> Vec<Range<usize>> {
    let lower_text = text.to_ascii_lowercase();
    let lower_query = query.to_ascii_lowercase();
    let mut matches = Vec::new();
    let mut start = 0;
    while let Some(pos) = lower_text[start..].find(&lower_query) {
        let abs_start = start + pos;
        matches.push(abs_start..abs_start + query.len());
        start = abs_start + query.len();
    }
    matches
}
```

- [ ] **Step 4: Add Cmd+H keybinding in main.rs**

In `handle_editor_key_down` (main.rs ~line 7157), add:

```rust
// After Cmd+F handler:
if keystroke.key == "h" && keystroke.modifiers.platform {
    if let Some(editor) = self.active_editor_mut() {
        editor.open_replace();
    }
    cx.notify();
    return true;
}
```

- [ ] **Step 5: Add replace UI to search bar in main.rs**

In the search bar rendering (~line 8089-8122), extend with replace field:

```rust
.when(editor.search_state().replace_open, |column| {
    column.child(
        div()
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_2()
            .bg(rgb(0x131a20))
            .border_1()
            .border_color(rgb(0x24303a))
            .rounded_md()
            .child(div().text_xs().text_color(rgb(0xf0d35f)).child("Replace"))
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .text_color(rgb(0xdce2e8))
                    .child(editor.search_state().replace_text.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x7f8a94))
                    .cursor_pointer()
                    .child("Replace")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(editor) = this.active_editor_mut() {
                            editor.replace_current_match();
                        }
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x7f8a94))
                    .cursor_pointer()
                    .child("All")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(editor) = this.active_editor_mut() {
                            editor.replace_all_matches();
                        }
                        cx.notify();
                    })),
            ),
    )
})
```

Also add a case sensitivity toggle button in the search bar:

```rust
.child(
    div()
        .text_xs()
        .text_color(if editor.search_state().case_sensitive {
            rgb(0xf0d35f)
        } else {
            rgb(0x7f8a94)
        })
        .cursor_pointer()
        .child("Aa")
        .on_click(cx.listener(move |this, _, _, cx| {
            if let Some(editor) = this.active_editor_mut() {
                editor.toggle_case_sensitivity();
            }
            cx.notify();
        })),
)
```

- [ ] **Step 6: Handle keyboard input for replace field**

In the search mode key handler (~line 7121-7155), when `replace_open` is true and the replace field is focused, route keystrokes to replace text manipulation. Add a `search_focus: SearchFocus` enum (Find/Replace) to track which field has focus:

```rust
enum SearchFocus {
    Find,
    Replace,
}
```

Tab key switches between Find and Replace fields.

- [ ] **Step 7: Build and test**

Run: `cargo test -p axis-editor 2>&1`
Expected: All tests pass.

Run: `cargo build -p axis-app 2>&1 | head -20`
Expected: Clean build.

- [ ] **Step 8: Commit**

```bash
git add -A && git commit -m "feat: add find-replace to editor (Cmd+H)

Search bar now supports replace field, replace one, replace all,
and case sensitivity toggle."
```

---

## Task 3: Add visual tab bar for editor surfaces

**Files:**
- Modify: `apps/axis-app/src/main.rs` (stack rail rendering ~lines 8256-8307, pane rendering)

- [ ] **Step 1: Replace vertical stack rail with horizontal tab bar for editor panes**

The current stack rail (lines 8256-8307) is a vertical sidebar with icons. For editor panes, replace with a horizontal tab bar at the top showing filenames.

In the pane rendering code, when there are multiple surfaces and the active surface is an editor, render a tab bar:

```rust
// Tab bar for editor panes with multiple surfaces
let tab_bar = div()
    .flex()
    .items_center()
    .h(px(28.0))
    .px(px(4.0))
    .bg(rgb(0x11171d))
    .border_b_1()
    .border_color(rgb(0x22303a))
    .overflow_x_hidden()
    .children(
        pane.surfaces
            .iter()
            .filter(|s| s.kind == PaneKind::Editor)
            .map(|surface| {
                let surface_id = surface.id;
                let active = surface_id == active_surface_id;
                let title = surface.title.clone().unwrap_or_default();
                let filename = title.rsplit('/').next().unwrap_or(&title);
                let dirty = surface.dirty;

                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded_t_md()
                    .cursor_pointer()
                    .when(active, |d| d.bg(rgb(0x1a2330)).border_b_2().border_color(rgb(0x7cc7ff)))
                    .when(!active, |d| d.hover(|d| d.bg(rgb(0x161d26))))
                    .child(
                        div()
                            .text_xs()
                            .text_color(if active { rgb(0xdce2e8) } else { rgb(0x7f8a94) })
                            .child(filename.to_string()),
                    )
                    .when(dirty, |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(rgb(0xf0d35f))
                                .child("●"),
                        )
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.active_workdesk_mut().focus_surface(pane_id, surface_id);
                        this.request_persist(cx);
                        cx.notify();
                    }))
                    // Middle-click to close
                    .on_mouse_down(gpui::MouseButton::Middle, cx.listener(move |this, _, window, cx| {
                        this.active_workdesk_mut().remove_surface(pane_id, surface_id);
                        this.request_persist(cx);
                        cx.notify();
                    }))
            })
            .collect::<Vec<_>>(),
    );
```

Only show this tab bar when the pane has more than 1 surface.

- [ ] **Step 2: Build and verify**

Run: `cargo build -p axis-app 2>&1 | head -20`
Expected: Clean build.

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "feat: add horizontal tab bar for editor panes

Shows filename + dirty indicator (●) for each open editor surface.
Click to switch, middle-click to close. Replaces vertical stack rail
for editor panes."
```

---

## Task 4: Add go-to-line dialog (Cmd+G)

**Files:**
- Modify: `apps/axis-app/src/main.rs` (key handler, dialog state, rendering)

- [ ] **Step 1: Add dialog state**

In the `AxisShell` struct (or equivalent top-level state in main.rs), add:

```rust
goto_line_open: bool,
goto_line_input: String,
```

Initialize both in the constructor: `false` and `String::new()`.

- [ ] **Step 2: Add Cmd+G keybinding**

In `handle_editor_key_down`, add:

```rust
if keystroke.key == "g" && keystroke.modifiers.platform && !keystroke.modifiers.shift {
    self.goto_line_open = true;
    self.goto_line_input.clear();
    cx.notify();
    return true;
}
```

- [ ] **Step 3: Handle dialog input**

When `goto_line_open` is true, intercept keystrokes:

```rust
if self.goto_line_open {
    match keystroke.key.as_str() {
        "escape" => {
            self.goto_line_open = false;
            cx.notify();
            return true;
        }
        "enter" => {
            if let Ok(line_num) = self.goto_line_input.parse::<usize>() {
                if let Some((_, surface_id)) = self.active_editor_ids() {
                    self.move_active_editor_to_line(surface_id, line_num);
                }
            }
            self.goto_line_open = false;
            cx.notify();
            return true;
        }
        "backspace" => {
            self.goto_line_input.pop();
            cx.notify();
            return true;
        }
        _ => {
            if let Some(ch) = editable_keystroke_text(keystroke) {
                if ch.chars().all(|c| c.is_ascii_digit()) {
                    self.goto_line_input.push_str(&ch);
                    cx.notify();
                    return true;
                }
            }
        }
    }
    return false;
}
```

- [ ] **Step 4: Render dialog overlay**

In the editor rendering section, add a small overlay when `goto_line_open`:

```rust
.when(self.goto_line_open, |container| {
    container.child(
        div()
            .absolute()
            .top(px(40.0))
            .right(px(20.0))
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .bg(rgb(0x1a2330))
            .border_1()
            .border_color(rgb(0x3b4d5e))
            .rounded_md()
            .shadow_lg()
            .child(div().text_xs().text_color(rgb(0xf0d35f)).child("Go to line:"))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0xdce2e8))
                    .min_w(px(40.0))
                    .child(self.goto_line_input.clone()),
            ),
    )
})
```

- [ ] **Step 5: Build and test**

Run: `cargo build -p axis-app 2>&1 | head -20`
Expected: Clean build.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat: add go-to-line dialog (Cmd+G)

Type a line number and press Enter to jump. Escape to dismiss."
```

---

## Task 5: Add file picker with fuzzy search (Cmd+P)

**Files:**
- Modify: `apps/axis-app/src/main.rs` (palette state, rendering, file indexing)

- [ ] **Step 1: Add file picker state**

```rust
file_picker_open: bool,
file_picker_query: String,
file_picker_files: Vec<String>,       // all files in worktree
file_picker_filtered: Vec<String>,    // filtered by query
file_picker_selected: usize,          // selected index in filtered list
```

- [ ] **Step 2: Add file indexing**

When a workdesk is activated or a worktree is bound, index all files:

```rust
fn index_worktree_files(&mut self, root: &Path) {
    let mut files = Vec::new();
    fn walk(dir: &Path, root: &Path, files: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            // Skip hidden dirs and common ignores
            if name.starts_with('.') || name == "node_modules" || name == "target" || name == "vendor" {
                continue;
            }
            if path.is_dir() {
                walk(&path, root, files);
            } else {
                if let Ok(rel) = path.strip_prefix(root) {
                    files.push(rel.to_string_lossy().to_string());
                }
            }
        }
    }
    walk(root, root, &mut files);
    files.sort();
    self.file_picker_files = files;
}
```

- [ ] **Step 3: Add fuzzy matching**

Simple substring match (can be upgraded later):

```rust
fn filter_file_picker(&mut self) {
    let query = self.file_picker_query.to_ascii_lowercase();
    if query.is_empty() {
        self.file_picker_filtered = self.file_picker_files.clone();
    } else {
        self.file_picker_filtered = self.file_picker_files
            .iter()
            .filter(|f| {
                let lower = f.to_ascii_lowercase();
                // Simple fuzzy: all query chars appear in order
                let mut chars = query.chars();
                let mut current = chars.next();
                for c in lower.chars() {
                    if current == Some(c) {
                        current = chars.next();
                    }
                }
                current.is_none()
            })
            .cloned()
            .collect();
    }
    self.file_picker_selected = 0;
}
```

- [ ] **Step 4: Add Cmd+P keybinding**

```rust
if keystroke.key == "p" && keystroke.modifiers.platform {
    self.file_picker_open = true;
    self.file_picker_query.clear();
    self.file_picker_selected = 0;
    // Index files if not done
    if self.file_picker_files.is_empty() {
        if let Some(root) = self.active_worktree_root() {
            self.index_worktree_files(&root);
        }
    }
    self.filter_file_picker();
    cx.notify();
    return true;
}
```

- [ ] **Step 5: Handle file picker input**

```rust
if self.file_picker_open {
    match keystroke.key.as_str() {
        "escape" => {
            self.file_picker_open = false;
            cx.notify();
            return true;
        }
        "enter" => {
            if let Some(path) = self.file_picker_filtered.get(self.file_picker_selected).cloned() {
                let root = self.active_worktree_root().unwrap_or_default();
                let full_path = root.join(&path);
                self.open_file_in_editor(full_path, cx);
            }
            self.file_picker_open = false;
            cx.notify();
            return true;
        }
        "up" => {
            self.file_picker_selected = self.file_picker_selected.saturating_sub(1);
            cx.notify();
            return true;
        }
        "down" => {
            if self.file_picker_selected + 1 < self.file_picker_filtered.len() {
                self.file_picker_selected += 1;
            }
            cx.notify();
            return true;
        }
        "backspace" => {
            self.file_picker_query.pop();
            self.filter_file_picker();
            cx.notify();
            return true;
        }
        _ => {
            if let Some(ch) = editable_keystroke_text(keystroke) {
                self.file_picker_query.push_str(&ch);
                self.filter_file_picker();
                cx.notify();
                return true;
            }
        }
    }
}
```

- [ ] **Step 6: Render file picker overlay**

```rust
.when(self.file_picker_open, |container| {
    let max_visible = 12;
    container.child(
        div()
            .absolute()
            .top(px(60.0))
            .left_1_2()
            .w(px(500.0))
            .translate((-px(250.0), px(0.0)))  // center
            .flex()
            .flex_col()
            .bg(rgb(0x1a2330))
            .border_1()
            .border_color(rgb(0x3b4d5e))
            .rounded_lg()
            .shadow_lg()
            .overflow_hidden()
            // Search input
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(0x22303a))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xdce2e8))
                            .child(if self.file_picker_query.is_empty() {
                                "Type to search files...".to_string()
                            } else {
                                self.file_picker_query.clone()
                            }),
                    ),
            )
            // Results
            .child(
                div()
                    .flex()
                    .flex_col()
                    .max_h(px(300.0))
                    .overflow_y_scroll()
                    .children(
                        self.file_picker_filtered
                            .iter()
                            .take(max_visible)
                            .enumerate()
                            .map(|(i, path)| {
                                let selected = i == self.file_picker_selected;
                                div()
                                    .px_3()
                                    .py_1()
                                    .text_xs()
                                    .text_color(if selected { rgb(0xdce2e8) } else { rgb(0x7f8a94) })
                                    .when(selected, |d| d.bg(rgb(0x2d5b88)))
                                    .child(path.clone())
                            })
                            .collect::<Vec<_>>(),
                    ),
            ),
    )
})
```

- [ ] **Step 7: Build and test**

Run: `cargo build -p axis-app 2>&1 | head -20`
Expected: Clean build.

- [ ] **Step 8: Commit**

```bash
git add -A && git commit -m "feat: add file picker with fuzzy search (Cmd+P)

Indexes worktree files, supports fuzzy substring matching,
arrow key navigation, Enter to open, Escape to dismiss."
```

---

## Task 6: Add keyboard shortcuts (duplicate, comment, delete line, move line, indent)

**Files:**
- Modify: `crates/axis-editor/src/lib.rs` (add methods)
- Modify: `apps/axis-app/src/main.rs` (handle_editor_key_down, ~line 7157-7237)

- [ ] **Step 1: Add editor methods**

In `crates/axis-editor/src/lib.rs`:

```rust
/// Duplicate the current line (or selected lines).
pub fn duplicate_line(&mut self) {
    let (line, _) = self.line_col_for_offset(self.cursor_offset());
    let line_range = self.line_range(line);
    let line_text = self.rope.byte_slice(line_range.clone()).to_string();
    // Insert a copy after the current line
    let insert_at = line_range.end;
    let old_selection = self.selection.clone();
    self.selection = Selection { range: insert_at..insert_at, reversed: false };
    self.replace_selection(&line_text);
    // Restore cursor on the new line
    let new_offset = old_selection.range.start + line_text.len();
    self.selection = Selection {
        range: new_offset..new_offset,
        reversed: false,
    };
}

/// Toggle line comment (prefix-based: //, #, etc.).
pub fn toggle_line_comment(&mut self) {
    let prefix = match self.language {
        LanguageKind::Rust | LanguageKind::JavaScript | LanguageKind::TypeScript
        | LanguageKind::Tsx | LanguageKind::Jsx | LanguageKind::Json => "// ",
        LanguageKind::Toml | LanguageKind::Yaml => "# ",
        LanguageKind::Markdown | LanguageKind::Plaintext => "// ",
    };

    let (line, _) = self.line_col_for_offset(self.cursor_offset());
    let text = self.line_text(line);
    let line_start = self.line_range(line).start;

    if text.trim_start().starts_with(prefix.trim()) {
        // Remove comment
        if let Some(pos) = text.find(prefix.trim()) {
            let remove_len = if text[pos..].starts_with(prefix) { prefix.len() } else { prefix.trim().len() };
            self.selection = Selection {
                range: (line_start + pos)..(line_start + pos + remove_len),
                reversed: false,
            };
            self.replace_selection("");
        }
    } else {
        // Add comment at start of content (after whitespace)
        let indent_len = text.len() - text.trim_start().len();
        let insert_at = line_start + indent_len;
        self.selection = Selection { range: insert_at..insert_at, reversed: false };
        self.replace_selection(prefix);
    }
}

/// Delete the entire current line.
pub fn delete_line(&mut self) {
    let (line, _) = self.line_col_for_offset(self.cursor_offset());
    let range = self.line_range(line);
    self.selection = Selection { range, reversed: false };
    self.replace_selection("");
}

/// Move the current line up.
pub fn move_line_up(&mut self) {
    let (line, col) = self.line_col_for_offset(self.cursor_offset());
    if line == 0 {
        return;
    }
    let current = self.line_text(line);
    let current_range = self.line_range(line);
    let above_range = self.line_range(line - 1);

    // Swap: delete current line, insert before previous
    let current_with_newline = self.rope.byte_slice(current_range.clone()).to_string();
    self.selection = Selection { range: current_range, reversed: false };
    self.replace_selection("");

    let insert_at = above_range.start;
    self.selection = Selection { range: insert_at..insert_at, reversed: false };
    self.replace_selection(&current_with_newline);

    // Restore cursor on moved line
    let new_offset = self.offset_for_line_col(line - 1, col);
    self.selection = Selection { range: new_offset..new_offset, reversed: false };
}

/// Move the current line down.
pub fn move_line_down(&mut self) {
    let (line, col) = self.line_col_for_offset(self.cursor_offset());
    if line + 1 >= self.line_count() {
        return;
    }
    let below_range = self.line_range(line + 1);
    let current_range = self.line_range(line);

    let below_text = self.rope.byte_slice(below_range.clone()).to_string();
    // Delete below line first (higher offset)
    self.selection = Selection { range: below_range, reversed: false };
    self.replace_selection("");

    // Insert above current line
    let insert_at = current_range.start;
    self.selection = Selection { range: insert_at..insert_at, reversed: false };
    self.replace_selection(&below_text);

    // Cursor moves down one line
    let new_offset = self.offset_for_line_col(line + 1, col);
    self.selection = Selection { range: new_offset..new_offset, reversed: false };
}

/// Indent selected lines (add spaces at start).
pub fn indent(&mut self) {
    let (line, _) = self.line_col_for_offset(self.cursor_offset());
    let line_start = self.line_range(line).start;
    self.selection = Selection { range: line_start..line_start, reversed: false };
    self.replace_selection(TAB_TEXT);
}

/// Outdent selected lines (remove spaces from start).
pub fn outdent(&mut self) {
    let (line, _) = self.line_col_for_offset(self.cursor_offset());
    let text = self.line_text(line);
    let line_start = self.line_range(line).start;
    let spaces = text.len() - text.trim_start().len();
    let remove = spaces.min(TAB_TEXT.len());
    if remove > 0 {
        self.selection = Selection {
            range: line_start..(line_start + remove),
            reversed: false,
        };
        self.replace_selection("");
    }
}
```

- [ ] **Step 2: Add keybindings in handle_editor_key_down**

In `apps/axis-app/src/main.rs`, in the platform modifier section (~line 7157):

```rust
// Cmd+D: duplicate line
if keystroke.key == "d" && keystroke.modifiers.platform {
    if let Some(editor) = self.active_editor_mut() {
        editor.duplicate_line();
    }
    changed = true;
}

// Cmd+/: toggle comment
if keystroke.key == "/" && keystroke.modifiers.platform {
    if let Some(editor) = self.active_editor_mut() {
        editor.toggle_line_comment();
    }
    changed = true;
}

// Cmd+Shift+K: delete line
if keystroke.key == "k" && keystroke.modifiers.platform && keystroke.modifiers.shift {
    if let Some(editor) = self.active_editor_mut() {
        editor.delete_line();
    }
    changed = true;
}

// Option+Up/Down: move line
if keystroke.key == "up" && keystroke.modifiers.alt {
    if let Some(editor) = self.active_editor_mut() {
        editor.move_line_up();
    }
    changed = true;
}
if keystroke.key == "down" && keystroke.modifiers.alt {
    if let Some(editor) = self.active_editor_mut() {
        editor.move_line_down();
    }
    changed = true;
}

// Cmd+[/]: indent/outdent
if keystroke.key == "]" && keystroke.modifiers.platform {
    if let Some(editor) = self.active_editor_mut() {
        editor.indent();
    }
    changed = true;
}
if keystroke.key == "[" && keystroke.modifiers.platform {
    if let Some(editor) = self.active_editor_mut() {
        editor.outdent();
    }
    changed = true;
}
```

- [ ] **Step 3: Build and test**

Run: `cargo test -p axis-editor 2>&1`
Run: `cargo build -p axis-app 2>&1 | head -20`
Expected: All pass.

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "feat: add editor shortcuts (duplicate, comment, delete, move, indent)

Cmd+D duplicate line, Cmd+/ toggle comment, Cmd+Shift+K delete line,
Option+Up/Down move line, Cmd+[/] indent/outdent."
```

---

## Task 7: Integrate tree-sitter for syntax highlighting

**Files:**
- Modify: `crates/axis-editor/src/lib.rs` (highlighting, language detection)

- [ ] **Step 1: Write failing test**

Add to `crates/axis-editor/tests/rope_buffer.rs`:

```rust
#[test]
fn rust_function_keyword_highlighted() {
    let buf = make_buffer_with_ext("fn main() {}\n", "rs");
    let spans = buf.highlight_line(0);
    // "fn" should be highlighted as Keyword
    let fn_span = spans.iter().find(|s| s.kind == axis_editor::HighlightKind::Keyword);
    assert!(fn_span.is_some(), "fn should be highlighted as keyword");
}

fn make_buffer_with_ext(text: &str, ext: &str) -> EditorBuffer {
    EditorBuffer::from_text(
        PathBuf::from(format!("/tmp/test.{ext}")),
        text.to_string(),
    )
}
```

- [ ] **Step 2: Add tree-sitter parser initialization**

In `crates/axis-editor/src/lib.rs`, add tree-sitter integration:

```rust
use tree_sitter::{Language, Parser, Tree, Query, QueryCursor};

struct TreeSitterState {
    parser: Parser,
    tree: Option<Tree>,
    highlight_query: Option<Query>,
}

impl EditorBuffer {
    fn init_tree_sitter(language: LanguageKind) -> Option<TreeSitterState> {
        let (ts_language, query_source) = match language {
            LanguageKind::Rust => (
                tree_sitter_rust::LANGUAGE.into(),
                include_str!("queries/rust/highlights.scm"),
            ),
            LanguageKind::TypeScript => (
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                include_str!("queries/typescript/highlights.scm"),
            ),
            LanguageKind::Tsx => (
                tree_sitter_typescript::LANGUAGE_TSX.into(),
                include_str!("queries/typescript/highlights.scm"),
            ),
            LanguageKind::JavaScript | LanguageKind::Jsx => (
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                include_str!("queries/typescript/highlights.scm"),
            ),
            LanguageKind::Json => (
                tree_sitter_json::LANGUAGE.into(),
                include_str!("queries/json/highlights.scm"),
            ),
            LanguageKind::Toml => (
                tree_sitter_toml_ng::LANGUAGE.into(),
                include_str!("queries/toml/highlights.scm"),
            ),
            LanguageKind::Yaml => (
                tree_sitter_yaml::LANGUAGE.into(),
                include_str!("queries/yaml/highlights.scm"),
            ),
            LanguageKind::Markdown => (
                tree_sitter_markdown::LANGUAGE.into(),
                include_str!("queries/markdown/highlights.scm"),
            ),
            LanguageKind::Plaintext => return None,
        };

        let mut parser = Parser::new();
        parser.set_language(&ts_language).ok()?;
        let query = Query::new(&ts_language, query_source).ok()?;

        Some(TreeSitterState {
            parser,
            tree: None,
            highlight_query: Some(query),
        })
    }
}
```

Note: You'll need to create `queries/` subdirectories with highlight queries for each language. Use minimal versions initially — the nvim-treesitter project has extensive ones, but start with core highlights (keywords, comments, strings, types, numbers).

- [ ] **Step 3: Create highlight query files**

Create `crates/axis-editor/src/queries/rust/highlights.scm`:
```scheme
(line_comment) @comment
(block_comment) @comment
(string_literal) @string
(raw_string_literal) @string
(char_literal) @string
(integer_literal) @number
(float_literal) @number
(boolean_literal) @keyword
["fn" "let" "mut" "pub" "use" "mod" "struct" "enum" "impl" "trait" "type"
 "where" "if" "else" "match" "for" "while" "loop" "return" "break" "continue"
 "async" "await" "move" "ref" "self" "Self" "super" "crate" "const" "static"
 "unsafe" "extern" "as" "in" "dyn" "true" "false"] @keyword
(type_identifier) @type
(primitive_type) @type
```

Create similar minimal query files for other languages. Each maps tree-sitter node types to highlight kinds.

- [ ] **Step 4: Parse on load and incremental reparse on edit**

Add `tree_sitter_state: Option<TreeSitterState>` to `EditorBuffer`.

In constructor:
```rust
let tree_sitter_state = Self::init_tree_sitter(language);
```

In `replace_selection`, after rope edit, update tree:
```rust
if let Some(ref mut ts) = self.tree_sitter_state {
    // Incremental edit notification
    if let Some(ref old_tree) = ts.tree {
        // tree-sitter edit point calculation
        let edit = tree_sitter::InputEdit {
            start_byte: range.start,
            old_end_byte: range.end,
            new_end_byte: range.start + new_text.len(),
            start_position: self.byte_to_point(range.start),
            old_end_position: self.byte_to_point(range.end),
            new_end_position: self.byte_to_point(range.start + new_text.len()),
        };
        // Clone and edit the old tree for incremental parse
        let mut old = old_tree.clone();
        old.edit(&edit);
        ts.tree = ts.parser.parse(self.rope.to_string(), Some(&old));
    } else {
        ts.tree = ts.parser.parse(self.rope.to_string(), None);
    }
}
```

Helper:
```rust
fn byte_to_point(&self, byte_offset: usize) -> tree_sitter::Point {
    let (line, col) = self.line_col_for_offset(byte_offset);
    tree_sitter::Point::new(line, col)
}
```

- [ ] **Step 5: Replace highlight_line with tree-sitter query**

```rust
pub fn highlight_line(&self, line_index: usize) -> Vec<HighlightSpan> {
    let mut cache = self.line_highlight_cache.borrow_mut();
    if cache.len() <= line_index {
        cache.resize(line_index + 1, None);
    }
    if let Some(cached) = &cache[line_index] {
        return cached.clone();
    }

    let text = self.line_text(line_index);
    let spans = if let Some(ref ts) = self.tree_sitter_state {
        if let (Some(tree), Some(query)) = (&ts.tree, &ts.highlight_query) {
            self.tree_sitter_highlights_for_line(tree, query, line_index, &text)
        } else {
            compute_line_highlights(&text, self.language) // fallback
        }
    } else {
        compute_line_highlights(&text, self.language) // no tree-sitter for this language
    };

    cache[line_index] = Some(spans.clone());
    spans
}

fn tree_sitter_highlights_for_line(
    &self,
    tree: &Tree,
    query: &Query,
    line_index: usize,
    line_text: &str,
) -> Vec<HighlightSpan> {
    let line_range = self.line_range(line_index);
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(line_range.clone());

    let source = self.rope.to_string();
    let matches = cursor.matches(query, tree.root_node(), source.as_bytes());

    let mut spans = Vec::new();
    for m in matches {
        for capture in m.captures {
            let node = capture.node;
            let capture_name = &query.capture_names()[capture.index as usize];
            let kind = capture_name_to_highlight_kind(capture_name);

            let start = node.start_byte().max(line_range.start) - line_range.start;
            let end = node.end_byte().min(line_range.end) - line_range.start;
            if start < end && end <= line_text.len() {
                spans.push(HighlightSpan { start, end, kind });
            }
        }
    }

    // Sort and dedup
    spans.sort_by_key(|s| s.start);
    spans
}

fn capture_name_to_highlight_kind(name: &str) -> HighlightKind {
    match name {
        "comment" => HighlightKind::Comment,
        "string" => HighlightKind::String,
        "number" => HighlightKind::Number,
        "keyword" => HighlightKind::Keyword,
        "type" => HighlightKind::Type,
        _ => HighlightKind::Plain,
    }
}
```

- [ ] **Step 6: Remove old lexical highlighting code**

Remove the hand-coded keyword lists, comment/string detection functions that were in `compute_line_highlights`. Keep `compute_line_highlights` as a fallback for `Plaintext` language but simplify it.

- [ ] **Step 7: Run tests**

Run: `cargo test -p axis-editor 2>&1`
Expected: All tests pass including the tree-sitter highlighting test.

- [ ] **Step 8: Build full app**

Run: `cargo build -p axis-app 2>&1 | head -20`
Expected: Clean build.

- [ ] **Step 9: Commit**

```bash
git add -A && git commit -m "feat: replace lexical highlighting with tree-sitter

Incremental parsing on each edit. Supports Rust, TypeScript, JavaScript,
JSON, TOML, YAML, Markdown. Falls back to lexical for Plaintext."
```

---

## Task 8: Create axis-lsp crate skeleton for LSP foundation

**Files:**
- Create: `crates/axis-lsp/Cargo.toml`
- Create: `crates/axis-lsp/src/lib.rs`
- Create: `crates/axis-lsp/src/transport.rs`
- Create: `crates/axis-lsp/src/manager.rs`
- Modify: `Cargo.toml` (workspace members)

- [ ] **Step 1: Create crate structure**

Create `crates/axis-lsp/Cargo.toml`:
```toml
[package]
name = "axis-lsp"
edition.workspace = true
license.workspace = true
publish = false
rust-version.workspace = true
version.workspace = true

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
```

Add to workspace `Cargo.toml`:
```toml
members = [
    # ... existing members
    "crates/axis-lsp",
]
```

- [ ] **Step 2: Create transport layer**

Create `crates/axis-lsp/src/transport.rs`:

```rust
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout};

/// JSON-RPC message for LSP communication.
#[derive(Debug, Serialize, Deserialize)]
pub struct LspMessage {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<serde_json::Value>,
}

impl LspMessage {
    pub fn request(id: u64, method: &str, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::Value::Number(id.into())),
            method: Some(method.to_string()),
            params: Some(params),
            result: None,
            error: None,
        }
    }

    pub fn notification(method: &str, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: None,
            method: Some(method.to_string()),
            params: Some(params),
            result: None,
            error: None,
        }
    }

    pub fn is_response(&self) -> bool {
        self.id.is_some() && self.method.is_none()
    }

    pub fn is_notification(&self) -> bool {
        self.id.is_none() && self.method.is_some()
    }
}

/// Writes an LSP message (with Content-Length header) to a writer.
pub fn write_message(writer: &mut impl Write, msg: &LspMessage) -> Result<()> {
    let body = serde_json::to_string(msg)?;
    write!(writer, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
    writer.flush()?;
    Ok(())
}

/// Reads an LSP message (with Content-Length header) from a reader.
pub fn read_message(reader: &mut impl BufRead) -> Result<LspMessage> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header)?;
        let header = header.trim();
        if header.is_empty() {
            break;
        }
        if let Some(len_str) = header.strip_prefix("Content-Length: ") {
            content_length = Some(len_str.parse().context("invalid Content-Length")?);
        }
    }
    let length = content_length.context("missing Content-Length header")?;
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    let msg: LspMessage = serde_json::from_slice(&body)?;
    Ok(msg)
}
```

- [ ] **Step 3: Create language server manager**

Create `crates/axis-lsp/src/manager.rs`:

```rust
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

/// Configuration for a language server.
#[derive(Clone, Debug)]
pub struct LspServerConfig {
    /// Command to spawn the server.
    pub command: String,
    /// Arguments.
    pub args: Vec<String>,
    /// File extensions this server handles.
    pub extensions: Vec<String>,
}

/// Manages spawned LSP server processes.
pub struct LspManager {
    configs: HashMap<String, LspServerConfig>,
    running: HashMap<String, Child>,
}

impl LspManager {
    pub fn new() -> Self {
        Self {
            configs: HashMap::new(),
            running: HashMap::new(),
        }
    }

    /// Register a server config for a language.
    pub fn register(&mut self, language: &str, config: LspServerConfig) {
        self.configs.insert(language.to_string(), config);
    }

    /// Find which language handles a given file extension.
    pub fn language_for_extension(&self, ext: &str) -> Option<String> {
        self.configs
            .iter()
            .find(|(_, c)| c.extensions.iter().any(|e| e == ext))
            .map(|(lang, _)| lang.clone())
    }

    /// Start a server for the given language if not already running.
    pub fn ensure_server(&mut self, language: &str) -> anyhow::Result<()> {
        if self.running.contains_key(language) {
            return Ok(());
        }
        let config = self.configs.get(language)
            .ok_or_else(|| anyhow::anyhow!("no LSP config for {language}"))?
            .clone();

        let child = Command::new(&config.command)
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to start LSP server '{}': {e}", config.command))?;

        self.running.insert(language.to_string(), child);
        Ok(())
    }

    /// Stop a running server.
    pub fn stop_server(&mut self, language: &str) {
        if let Some(mut child) = self.running.remove(language) {
            let _ = child.kill();
        }
    }

    /// Stop all running servers.
    pub fn stop_all(&mut self) {
        let languages: Vec<String> = self.running.keys().cloned().collect();
        for lang in languages {
            self.stop_server(&lang);
        }
    }
}

impl Drop for LspManager {
    fn drop(&mut self) {
        self.stop_all();
    }
}
```

- [ ] **Step 4: Create lib.rs**

Create `crates/axis-lsp/src/lib.rs`:

```rust
pub mod manager;
pub mod transport;

pub use manager::{LspManager, LspServerConfig};
pub use transport::{LspMessage, read_message, write_message};
```

- [ ] **Step 5: Build**

Run: `cargo build -p axis-lsp 2>&1 | head -20`
Expected: Clean build.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat: create axis-lsp crate skeleton

JSON-RPC transport layer and language server manager with
spawn/stop lifecycle. Foundation for LSP integration."
```

---

## Summary

| Task | Component | Type |
|------|-----------|------|
| 1 | Rope buffer (ropey) + TextDelta + document versioning | Buffer model |
| 2 | Find-replace (Cmd+H) | Editor UI |
| 3 | Visual tab bar | Editor UI |
| 4 | Go-to-line (Cmd+G) | Editor UI |
| 5 | File picker (Cmd+P) | Editor UI |
| 6 | Keyboard shortcuts | Editor UI |
| 7 | Tree-sitter highlighting | Syntax |
| 8 | axis-lsp crate skeleton | LSP foundation |
