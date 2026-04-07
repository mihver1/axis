use std::cell::RefCell;
use std::{
    fs,
    ops::Range,
    path::{Path, PathBuf},
    time::SystemTime,
};

use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;

const TAB_TEXT: &str = "    ";

/// A text change delta for LSP incremental sync.
#[derive(Clone, Debug)]
pub struct TextDelta {
    pub range: Range<usize>,
    pub text: String,
}

#[derive(Clone, Debug)]
struct UndoEntry {
    delta: TextDelta,
    selection: Selection,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub range: Range<usize>,
    pub reversed: bool,
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HighlightKind {
    Plain,
    Comment,
    String,
    Keyword,
    Number,
    Type,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightSpan {
    pub range: Range<usize>,
    pub kind: HighlightKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LanguageKind {
    Plaintext,
    Rust,
    JavaScript,
    TypeScript,
    Tsx,
    Jsx,
    Json,
    Toml,
    Yaml,
    Markdown,
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

impl EditorBuffer {
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        Ok(Self::restore(path, text, false))
    }

    pub fn restore(path: impl Into<PathBuf>, text: impl Into<String>, dirty: bool) -> Self {
        let path = path.into();
        let text = text.into();
        let saved_rope = if dirty {
            Rope::from_str(&fs::read_to_string(&path).unwrap_or_default())
        } else {
            Rope::from_str(&text)
        };
        let language = detect_language(&path);
        let rope = Rope::from_str(&text);
        let line_count = rope_line_count(&rope);
        let mut this = Self {
            path,
            rope,
            saved_rope,
            dirty,
            selection: Selection::default(),
            marked_range: None,
            preferred_column: None,
            scroll_top_line: 0,
            search: SearchState::default(),
            line_highlight_cache: RefCell::new(vec![None; line_count.max(1)]),
            language,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_synced_modified_at: None,
            external_modified: false,
            document_version: 0,
            pending_deltas: Vec::new(),
        };
        this.selection.range = 0..0;
        this.last_synced_modified_at = file_modified_at(&this.path);
        this
    }

    pub fn from_text(path: PathBuf, text: String) -> Self {
        Self::restore(path, text, false)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn path_string(&self) -> String {
        self.path.display().to_string()
    }

    pub fn title(&self) -> String {
        self.path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Editor")
            .to_string()
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn persisted_buffer_text(&self) -> Option<String> {
        self.dirty.then(|| self.rope.to_string())
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn search_state(&self) -> &SearchState {
        &self.search
    }

    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    pub fn marked_range(&self) -> Option<&Range<usize>> {
        self.marked_range.as_ref()
    }

    pub fn language(&self) -> LanguageKind {
        self.language
    }

    pub fn line_count(&self) -> usize {
        rope_line_count(&self.rope)
    }

    pub fn scroll_top_line(&self) -> usize {
        self.scroll_top_line
            .min(self.line_count().saturating_sub(1))
    }

    pub fn set_scroll_top_line(&mut self, line: usize) {
        self.scroll_top_line = line.min(self.line_count().saturating_sub(1));
    }

    pub fn scroll_by_lines(&mut self, delta: isize, viewport_lines: usize) {
        let max_top = self
            .line_count()
            .saturating_sub(viewport_lines.max(1))
            .saturating_add(1)
            .saturating_sub(1);
        let next = if delta.is_negative() {
            self.scroll_top_line
                .saturating_sub(delta.unsigned_abs().min(self.scroll_top_line))
        } else {
            self.scroll_top_line
                .saturating_add(delta as usize)
                .min(max_top)
        };
        self.scroll_top_line = next;
    }

    pub fn visible_line_range(&self, viewport_lines: usize) -> Range<usize> {
        let start = self.scroll_top_line();
        let end = (start + viewport_lines.max(1)).min(self.line_count());
        start..end
    }

    pub fn line_text(&self, line_index: usize) -> String {
        let line_index = line_index.min(self.line_count().saturating_sub(1));
        let line_slice = self.rope.line(line_index);
        let s = line_slice.to_string();
        // Strip trailing newline
        if s.ends_with('\n') {
            s[..s.len() - 1].to_string()
        } else {
            s
        }
    }

    pub fn line_range(&self, line_index: usize) -> Range<usize> {
        let line_index = line_index.min(self.line_count().saturating_sub(1));
        let start = self.rope.line_to_byte(line_index);
        let end = if line_index + 1 < self.rope.len_lines() {
            self.rope.line_to_byte(line_index + 1)
        } else {
            self.rope.len_bytes()
        };
        // Strip trailing newline from range
        let end = if end > start && self.rope.byte(end - 1) == b'\n' {
            end - 1
        } else {
            end
        };
        start..end
    }

    pub fn line_number_width(&self) -> usize {
        self.line_count().to_string().len().max(2)
    }

    pub fn cursor_offset(&self) -> usize {
        if self.selection.reversed {
            self.selection.range.start
        } else {
            self.selection.range.end
        }
    }

    pub fn selected_text(&self) -> Option<String> {
        if self.selection.range.is_empty() {
            return None;
        }
        let start = self.selection.range.start;
        let end = self.selection.range.end;
        if end > self.rope.len_bytes() {
            return None;
        }
        let start_char = self.rope.byte_to_char(start);
        let end_char = self.rope.byte_to_char(end);
        Some(self.rope.slice(start_char..end_char).to_string())
    }

    pub fn move_left(&mut self, selecting: bool) {
        let text = self.rope.to_string();
        if !selecting && !self.selection.range.is_empty() {
            self.move_to(self.selection.range.start, false);
            return;
        }
        let target = previous_boundary(&text, self.cursor_offset());
        self.move_to(target, selecting);
    }

    pub fn move_right(&mut self, selecting: bool) {
        let text = self.rope.to_string();
        if !selecting && !self.selection.range.is_empty() {
            self.move_to(self.selection.range.end, false);
            return;
        }
        let target = next_boundary(&text, self.cursor_offset());
        self.move_to(target, selecting);
    }

    pub fn move_up(&mut self, selecting: bool) {
        let (line, column) = self.line_col_for_offset(self.cursor_offset());
        let preferred = self.preferred_column.unwrap_or(column);
        let target_line = line.saturating_sub(1);
        let target = self.offset_for_line_col(target_line, preferred);
        self.preferred_column = Some(preferred);
        self.move_to(target, selecting);
    }

    pub fn move_down(&mut self, selecting: bool) {
        let (line, column) = self.line_col_for_offset(self.cursor_offset());
        let preferred = self.preferred_column.unwrap_or(column);
        let target_line = (line + 1).min(self.line_count().saturating_sub(1));
        let target = self.offset_for_line_col(target_line, preferred);
        self.preferred_column = Some(preferred);
        self.move_to(target, selecting);
    }

    pub fn move_home(&mut self, selecting: bool) {
        let (line, _) = self.line_col_for_offset(self.cursor_offset());
        let target = self.line_range(line).start;
        self.preferred_column = Some(0);
        self.move_to(target, selecting);
    }

    pub fn move_end(&mut self, selecting: bool) {
        let (line, _) = self.line_col_for_offset(self.cursor_offset());
        let target = self.line_range(line).end;
        let (_, column) = self.line_col_for_offset(target);
        self.preferred_column = Some(column);
        self.move_to(target, selecting);
    }

    pub fn page_up(&mut self, selecting: bool, viewport_lines: usize) {
        let (line, column) = self.line_col_for_offset(self.cursor_offset());
        let target_line = line.saturating_sub(viewport_lines.max(1));
        let target = self.offset_for_line_col(target_line, column);
        self.scroll_by_lines(-(viewport_lines as isize), viewport_lines);
        self.preferred_column = Some(column);
        self.move_to(target, selecting);
    }

    pub fn page_down(&mut self, selecting: bool, viewport_lines: usize) {
        let (line, column) = self.line_col_for_offset(self.cursor_offset());
        let target_line = (line + viewport_lines.max(1)).min(self.line_count().saturating_sub(1));
        let target = self.offset_for_line_col(target_line, column);
        self.scroll_by_lines(viewport_lines as isize, viewport_lines);
        self.preferred_column = Some(column);
        self.move_to(target, selecting);
    }

    pub fn select_all(&mut self) {
        self.selection = Selection {
            range: 0..self.rope.len_bytes(),
            reversed: false,
        };
        self.preferred_column = None;
    }

    pub fn backspace(&mut self) -> bool {
        if self.selection.range.is_empty() {
            let text = self.rope.to_string();
            let previous = previous_boundary(&text, self.cursor_offset());
            self.selection.range = previous..self.cursor_offset();
            self.selection.reversed = false;
        }
        self.replace_selection("")
    }

    pub fn delete_forward(&mut self) -> bool {
        if self.selection.range.is_empty() {
            let text = self.rope.to_string();
            let next = next_boundary(&text, self.cursor_offset());
            self.selection.range = self.cursor_offset()..next;
            self.selection.reversed = false;
        }
        self.replace_selection("")
    }

    pub fn insert_newline(&mut self) -> bool {
        self.replace_selection("\n")
    }

    pub fn insert_tab(&mut self) -> bool {
        self.replace_selection(TAB_TEXT)
    }

    pub fn replace_selection(&mut self, replacement: &str) -> bool {
        let range = self.selection.range.clone();
        let old_text = self.get_byte_range_text(range.clone());
        let changed = self.replace_internal(None, replacement, None);
        if changed {
            // Record undo entry
            let undo_entry = UndoEntry {
                delta: TextDelta {
                    range: range.start..range.start + replacement.len(),
                    text: old_text,
                },
                selection: self.selection.clone(),
            };
            self.undo_stack.push(undo_entry);
            self.redo_stack.clear();
        }
        changed
    }

    pub fn replace_text_in_range_utf16(
        &mut self,
        range_utf16: Option<Range<usize>>,
        replacement: &str,
    ) -> bool {
        let text = self.rope.to_string();
        let range = range_utf16
            .as_ref()
            .map(|r| range_from_utf16_str(&text, r))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selection.range.clone());
        let old_text = self.get_byte_range_text(range.clone());
        let changed = self.replace_internal(range_utf16, replacement, None);
        if changed {
            let undo_entry = UndoEntry {
                delta: TextDelta {
                    range: range.start..range.start + replacement.len(),
                    text: old_text,
                },
                selection: self.selection.clone(),
            };
            self.undo_stack.push(undo_entry);
            self.redo_stack.clear();
        }
        changed
    }

    pub fn replace_and_mark_text_in_range_utf16(
        &mut self,
        range_utf16: Option<Range<usize>>,
        replacement: &str,
        selected_range_utf16: Option<Range<usize>>,
    ) -> bool {
        let text = self.rope.to_string();
        let range = range_utf16
            .as_ref()
            .map(|r| range_from_utf16_str(&text, r))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selection.range.clone());
        let old_text = self.get_byte_range_text(range.clone());
        let changed = self.replace_internal(range_utf16, replacement, selected_range_utf16);
        if changed {
            let undo_entry = UndoEntry {
                delta: TextDelta {
                    range: range.start..range.start + replacement.len(),
                    text: old_text,
                },
                selection: self.selection.clone(),
            };
            self.undo_stack.push(undo_entry);
            self.redo_stack.clear();
        }
        changed
    }

    pub fn undo(&mut self) -> bool {
        let Some(entry) = self.undo_stack.pop() else {
            return false;
        };
        // The entry.delta.range is the range of new text that was inserted.
        // entry.delta.text is the old text that was replaced.
        // To undo: remove the new text at entry.delta.range, insert old text at entry.delta.range.start.
        let new_range = entry.delta.range.clone();
        let new_text = self.get_byte_range_text(new_range.clone());
        let old_text = entry.delta.text.clone();
        let current_selection = self.selection.clone();

        // Apply the inverse
        let start_char = self.rope.byte_to_char(new_range.start);
        let end_char = self.rope.byte_to_char(new_range.end);
        self.rope.remove(start_char..end_char);
        let insert_char = self.rope.byte_to_char(new_range.start);
        self.rope.insert(insert_char, &old_text);

        // Push redo entry (inverse of what we just did)
        let redo_entry = UndoEntry {
            delta: TextDelta {
                range: new_range.start..new_range.start + old_text.len(),
                text: new_text,
            },
            selection: current_selection,
        };
        self.redo_stack.push(redo_entry);

        // Restore selection from entry
        self.selection = entry.selection;
        self.dirty = self.rope.to_string() != self.saved_rope.to_string();
        self.preferred_column = None;
        self.invalidate_line_cache();
        self.recompute_search_matches(false);
        self.document_version += 1;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(entry) = self.redo_stack.pop() else {
            return false;
        };
        let new_range = entry.delta.range.clone();
        let new_text = self.get_byte_range_text(new_range.clone());
        let old_text = entry.delta.text.clone();
        let current_selection = self.selection.clone();

        let start_char = self.rope.byte_to_char(new_range.start);
        let end_char = self.rope.byte_to_char(new_range.end);
        self.rope.remove(start_char..end_char);
        let insert_char = self.rope.byte_to_char(new_range.start);
        self.rope.insert(insert_char, &old_text);

        let undo_entry = UndoEntry {
            delta: TextDelta {
                range: new_range.start..new_range.start + old_text.len(),
                text: new_text,
            },
            selection: current_selection,
        };
        self.undo_stack.push(undo_entry);

        self.selection = entry.selection;
        self.dirty = self.rope.to_string() != self.saved_rope.to_string();
        self.preferred_column = None;
        self.invalidate_line_cache();
        self.recompute_search_matches(false);
        self.document_version += 1;
        true
    }

    pub fn save(&mut self) -> Result<(), String> {
        let text = self.rope.to_string();
        fs::write(&self.path, &text)
            .map_err(|error| format!("write {}: {error}", self.path.display()))?;
        self.saved_rope = self.rope.clone();
        self.dirty = false;
        self.external_modified = false;
        self.last_synced_modified_at = file_modified_at(&self.path);
        Ok(())
    }

    pub fn reload(&mut self) -> Result<(), String> {
        let reloaded = fs::read_to_string(&self.path)
            .map_err(|error| format!("read {}: {error}", self.path.display()))?;
        self.rope = Rope::from_str(&reloaded);
        self.saved_rope = Rope::from_str(&reloaded);
        self.dirty = false;
        self.selection.range = 0..0;
        self.selection.reversed = false;
        self.marked_range = None;
        self.preferred_column = None;
        self.external_modified = false;
        self.last_synced_modified_at = file_modified_at(&self.path);
        self.invalidate_line_cache();
        self.recompute_search_matches(false);
        self.document_version += 1;
        Ok(())
    }

    pub fn check_external_change(&mut self) {
        if self.dirty {
            return;
        }
        match (self.last_synced_modified_at, file_modified_at(&self.path)) {
            (Some(previous), Some(current)) if current > previous => {
                self.external_modified = true;
            }
            _ => {}
        }
    }

    pub fn external_modified(&self) -> bool {
        self.external_modified
    }

    pub fn open_search(&mut self) {
        self.search.open = true;
        self.recompute_search_matches(true);
    }

    pub fn close_search(&mut self) {
        self.search.open = false;
        self.search.replace_open = false;
        self.search.query.clear();
        self.search.replace_text.clear();
        self.search.active_match = None;
        self.search.matches.clear();
    }

    pub fn open_replace(&mut self) {
        self.search.open = true;
        self.search.replace_open = true;
        self.recompute_search_matches(true);
    }

    pub fn toggle_case_sensitivity(&mut self) {
        self.search.case_sensitive = !self.search.case_sensitive;
        self.recompute_search_matches(true);
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

    pub fn replace_current_match(&mut self) -> bool {
        let Some(index) = self.search.active_match else {
            return false;
        };
        let Some(range) = self.search.matches.get(index).cloned() else {
            return false;
        };
        let replace_text = self.search.replace_text.clone();
        let replacement_start = range.start;
        self.selection.range = range;
        self.selection.reversed = false;
        self.replace_selection(&replace_text);
        self.recompute_search_matches(false);
        // Snap to next match at or after the replacement point
        if !self.search.matches.is_empty() {
            let next_index = self
                .search
                .matches
                .iter()
                .position(|r| r.start >= replacement_start)
                .unwrap_or(0);
            self.activate_search_match(next_index);
        }
        true
    }

    pub fn replace_all_matches(&mut self) -> usize {
        let mut count = 0usize;
        loop {
            let Some(range) = self.search.matches.last().cloned() else {
                break;
            };
            let replace_text = self.search.replace_text.clone();
            self.selection.range = range;
            self.selection.reversed = false;
            self.replace_selection(&replace_text);
            self.recompute_search_matches(false);
            count += 1;
            // Safety: if matches are not decreasing we should stop
            if self.search.matches.is_empty() {
                break;
            }
        }
        count
    }

    pub fn append_search_text(&mut self, text: &str) {
        self.search.query.push_str(text);
        self.recompute_search_matches(true);
    }

    pub fn pop_search_text(&mut self) {
        self.search.query.pop();
        self.recompute_search_matches(true);
    }

    pub fn next_search_match(&mut self) -> bool {
        if self.search.matches.is_empty() {
            return false;
        }
        let next = self
            .search
            .active_match
            .map(|index| (index + 1) % self.search.matches.len())
            .unwrap_or(0);
        self.activate_search_match(next);
        true
    }

    pub fn previous_search_match(&mut self) -> bool {
        if self.search.matches.is_empty() {
            return false;
        }
        let previous = self
            .search
            .active_match
            .map(|index| {
                if index == 0 {
                    self.search.matches.len() - 1
                } else {
                    index - 1
                }
            })
            .unwrap_or(self.search.matches.len() - 1);
        self.activate_search_match(previous);
        true
    }

    pub fn search_matches(&self) -> Vec<Range<usize>> {
        self.search.matches.clone()
    }

    pub fn search_match_count(&self) -> usize {
        self.search.matches.len()
    }

    pub fn current_search_match(&self) -> Option<Range<usize>> {
        self.search
            .active_match
            .and_then(|index| self.search.matches.get(index).cloned())
    }

    pub fn highlight_line(&self, line_index: usize) -> Vec<HighlightSpan> {
        let line_index = line_index.min(self.line_count().saturating_sub(1));
        {
            let cache = self.line_highlight_cache.borrow();
            if let Some(Some(spans)) = cache.get(line_index) {
                return spans.clone();
            }
        }

        let text = self.line_text(line_index);
        let spans = lexical_highlight_line(self.language, &text);
        let mut cache = self.line_highlight_cache.borrow_mut();
        if cache.len() < self.line_count() {
            cache.resize(self.line_count(), None);
        }
        cache[line_index] = Some(spans.clone());
        spans
    }

    pub fn move_to_offset(&mut self, offset: usize, selecting: bool) {
        self.move_to(offset, selecting);
    }

    pub fn offset_to_utf16(&self, offset: usize) -> usize {
        let text = self.rope.to_string();
        offset_to_utf16(&text, offset)
    }

    pub fn offset_from_utf16(&self, offset: usize) -> usize {
        let text = self.rope.to_string();
        offset_from_utf16(&text, offset)
    }

    pub fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    pub fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    pub fn line_col_for_offset(&self, offset: usize) -> (usize, usize) {
        let offset = offset.min(self.rope.len_bytes());
        let line = self.rope.byte_to_line(offset);
        let line_start = self.rope.line_to_byte(line);
        (line, offset.saturating_sub(line_start))
    }

    pub fn offset_for_line_col(&self, line: usize, column: usize) -> usize {
        let range = self.line_range(line);
        (range.start + column).min(range.end)
    }

    pub fn document_version(&self) -> u64 {
        self.document_version
    }

    pub fn take_pending_deltas(&mut self) -> Vec<TextDelta> {
        std::mem::take(&mut self.pending_deltas)
    }

    fn move_to(&mut self, offset: usize, selecting: bool) {
        let offset = offset.min(self.rope.len_bytes());
        if selecting {
            if self.selection.reversed {
                self.selection.range.start = offset;
            } else {
                self.selection.range.end = offset;
            }
            if self.selection.range.end < self.selection.range.start {
                self.selection.reversed = !self.selection.reversed;
                self.selection.range = self.selection.range.end..self.selection.range.start;
            }
        } else {
            self.selection.range = offset..offset;
            self.selection.reversed = false;
        }
        self.marked_range = None;
    }

    fn activate_search_match(&mut self, index: usize) {
        let Some(range) = self.search.matches.get(index).cloned() else {
            self.search.active_match = None;
            return;
        };
        self.selection.range = range;
        self.selection.reversed = false;
        self.search.active_match = Some(index);
    }

    fn recompute_search_matches(&mut self, snap_to_first: bool) {
        let text = self.rope.to_string();
        self.search.matches =
            compute_search_matches(&text, &self.search.query, self.search.case_sensitive);
        if self.search.matches.is_empty() {
            self.search.active_match = None;
            return;
        }
        if snap_to_first {
            let cursor = self.cursor_offset();
            let index = self
                .search
                .matches
                .iter()
                .position(|range| range.start >= cursor)
                .unwrap_or(0);
            self.activate_search_match(index);
        } else if let Some(index) = self.search.active_match {
            if index >= self.search.matches.len() {
                self.search.active_match = Some(self.search.matches.len() - 1);
            }
        }
    }

    fn get_byte_range_text(&self, range: Range<usize>) -> String {
        if range.is_empty() || range.start >= self.rope.len_bytes() {
            return String::new();
        }
        let end = range.end.min(self.rope.len_bytes());
        let start_char = self.rope.byte_to_char(range.start);
        let end_char = self.rope.byte_to_char(end);
        self.rope.slice(start_char..end_char).to_string()
    }

    fn replace_internal(
        &mut self,
        range_utf16: Option<Range<usize>>,
        replacement: &str,
        selected_range_utf16: Option<Range<usize>>,
    ) -> bool {
        let text = self.rope.to_string();
        let range = range_utf16
            .as_ref()
            .map(|r| range_from_utf16_str(&text, r))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selection.range.clone());

        let len = self.rope.len_bytes();
        if range.start > len || range.end > len || range.start > range.end {
            return false;
        }

        // Check if this would actually change anything
        let old_text = self.get_byte_range_text(range.clone());
        let changed = old_text != replacement;

        // Apply the edit via rope operations
        if !range.is_empty() {
            let start_char = self.rope.byte_to_char(range.start);
            let end_char = self.rope.byte_to_char(range.end);
            self.rope.remove(start_char..end_char);
        }
        if !replacement.is_empty() {
            let insert_char = self.rope.byte_to_char(range.start);
            self.rope.insert(insert_char, replacement);
        }

        self.dirty = self.rope.to_string() != self.saved_rope.to_string();
        self.external_modified = false;
        self.marked_range = if replacement.is_empty() {
            None
        } else {
            Some(range.start..range.start + replacement.len())
        };

        // Compute new selection, handling utf16 selected_range if provided
        let new_text = self.rope.to_string();
        self.selection.range = selected_range_utf16
            .as_ref()
            .map(|selection| range_from_utf16_str(&new_text, selection))
            .map(|selection| {
                range.start + selection.start..range.start + selection.end.min(replacement.len())
            })
            .unwrap_or_else(|| {
                let offset = range.start + replacement.len();
                offset..offset
            });
        self.selection.reversed = false;
        if selected_range_utf16.is_none() {
            self.marked_range = None;
        }
        self.preferred_column = None;
        self.invalidate_line_cache();
        self.recompute_search_matches(false);

        if changed {
            // Record delta for LSP
            self.document_version += 1;
            self.pending_deltas.push(TextDelta {
                range: range.clone(),
                text: replacement.to_string(),
            });
        }

        changed
    }

    fn invalidate_line_cache(&mut self) {
        let line_count = self.line_count();
        self.line_highlight_cache
            .replace(vec![None; line_count.max(1)]);
        self.scroll_top_line = self
            .scroll_top_line
            .min(self.line_count().saturating_sub(1));
    }
}

/// Compute the "logical" line count, collapsing the phantom trailing line
/// that ropey reports when the text ends with `\n`.
fn rope_line_count(rope: &Rope) -> usize {
    let raw = rope.len_lines();
    if raw > 1 && rope.len_bytes() > 0 && rope.byte(rope.len_bytes() - 1) == b'\n' {
        // ropey counts the empty trailing line after a final newline;
        // the old line_starts model did not.
        raw - 1
    } else {
        raw.max(1)
    }
}

fn range_from_utf16_str(text: &str, range: &Range<usize>) -> Range<usize> {
    offset_from_utf16(text, range.start)..offset_from_utf16(text, range.end)
}

fn compute_search_matches(text: &str, query: &str, case_sensitive: bool) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut matches = Vec::new();
    let mut start = 0usize;
    if case_sensitive {
        while let Some(found) = text[start..].find(query) {
            let begin = start + found;
            let end = begin + query.len();
            matches.push(begin..end);
            start = end.max(begin + 1);
        }
    } else {
        let query_lower = query.to_ascii_lowercase();
        let haystack = text.to_ascii_lowercase();
        while let Some(found) = haystack[start..].find(&query_lower) {
            let begin = start + found;
            let end = begin + query_lower.len();
            matches.push(begin..end);
            start = end.max(begin + 1);
        }
    }
    matches
}

fn detect_language(path: &Path) -> LanguageKind {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return LanguageKind::Plaintext;
    };
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".rs") {
        return LanguageKind::Rust;
    }
    if lower.ends_with(".tsx") {
        return LanguageKind::Tsx;
    }
    if lower.ends_with(".ts") {
        return LanguageKind::TypeScript;
    }
    if lower.ends_with(".jsx") {
        return LanguageKind::Jsx;
    }
    if lower.ends_with(".js") || lower.ends_with(".mjs") || lower.ends_with(".cjs") {
        return LanguageKind::JavaScript;
    }
    if lower.ends_with(".json") {
        return LanguageKind::Json;
    }
    if lower.ends_with(".toml") {
        return LanguageKind::Toml;
    }
    if lower.ends_with(".yaml") || lower.ends_with(".yml") {
        return LanguageKind::Yaml;
    }
    if lower.ends_with(".md") || lower.ends_with(".markdown") {
        return LanguageKind::Markdown;
    }
    LanguageKind::Plaintext
}

fn lexical_highlight_line(language: LanguageKind, line: &str) -> Vec<HighlightSpan> {
    if line.is_empty() {
        return Vec::new();
    }

    if matches!(language, LanguageKind::Markdown) {
        if line.starts_with('#') {
            return vec![HighlightSpan {
                range: 0..line.len(),
                kind: HighlightKind::Keyword,
            }];
        }
        if line.starts_with("```") {
            return vec![HighlightSpan {
                range: 0..line.len(),
                kind: HighlightKind::Type,
            }];
        }
    }

    let comment_prefix = match language {
        LanguageKind::Rust
        | LanguageKind::JavaScript
        | LanguageKind::TypeScript
        | LanguageKind::Tsx
        | LanguageKind::Jsx => Some("//"),
        LanguageKind::Toml | LanguageKind::Yaml => Some("#"),
        _ => None,
    };

    let mut spans = Vec::new();
    if let Some(prefix) = comment_prefix {
        if let Some(index) = line.find(prefix) {
            spans.push(HighlightSpan {
                range: index..line.len(),
                kind: HighlightKind::Comment,
            });
        }
    }

    let keywords = match language {
        LanguageKind::Rust => &[
            "fn", "let", "mut", "pub", "impl", "struct", "enum", "match", "if", "else", "use",
            "mod", "for", "while", "loop", "return", "crate", "self", "super", "trait", "where",
        ][..],
        LanguageKind::JavaScript
        | LanguageKind::TypeScript
        | LanguageKind::Tsx
        | LanguageKind::Jsx => &[
            "function",
            "const",
            "let",
            "var",
            "return",
            "if",
            "else",
            "import",
            "from",
            "export",
            "class",
            "extends",
            "new",
            "switch",
            "case",
            "default",
            "for",
            "while",
            "async",
            "await",
            "type",
            "interface",
        ][..],
        LanguageKind::Json => &["true", "false", "null"][..],
        LanguageKind::Toml => &["true", "false"][..],
        LanguageKind::Yaml => &["true", "false", "null"][..],
        _ => &[][..],
    };

    spans.extend(find_string_spans(line));
    spans.extend(find_number_spans(line));
    spans.extend(find_keyword_spans(line, keywords));
    spans.sort_by_key(|span| span.range.start);
    coalesce_spans(spans)
}

fn find_string_spans(line: &str) -> Vec<HighlightSpan> {
    let mut spans = Vec::new();
    let bytes = line.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let quote = bytes[cursor];
        if quote != b'"' && quote != b'\'' {
            cursor += 1;
            continue;
        }
        let start = cursor;
        cursor += 1;
        while cursor < bytes.len() {
            if bytes[cursor] == b'\\' {
                cursor += 2;
                continue;
            }
            if bytes[cursor] == quote {
                cursor += 1;
                break;
            }
            cursor += 1;
        }
        spans.push(HighlightSpan {
            range: start..cursor.min(bytes.len()),
            kind: HighlightKind::String,
        });
    }
    spans
}

fn find_number_spans(line: &str) -> Vec<HighlightSpan> {
    let mut spans = Vec::new();
    let bytes = line.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if !bytes[cursor].is_ascii_digit() {
            cursor += 1;
            continue;
        }
        let start = cursor;
        cursor += 1;
        while cursor < bytes.len() && (bytes[cursor].is_ascii_digit() || bytes[cursor] == b'_') {
            cursor += 1;
        }
        spans.push(HighlightSpan {
            range: start..cursor,
            kind: HighlightKind::Number,
        });
    }
    spans
}

fn find_keyword_spans(line: &str, keywords: &[&str]) -> Vec<HighlightSpan> {
    let mut spans = Vec::new();
    for keyword in keywords {
        let mut start = 0usize;
        while let Some(found) = line[start..].find(keyword) {
            let begin = start + found;
            let end = begin + keyword.len();
            let left_ok = begin == 0 || !is_identifier_char(line.as_bytes()[begin - 1]);
            let right_ok = end == line.len() || !is_identifier_char(line.as_bytes()[end]);
            if left_ok && right_ok {
                spans.push(HighlightSpan {
                    range: begin..end,
                    kind: HighlightKind::Keyword,
                });
            }
            start = end;
        }
    }
    spans
}

fn coalesce_spans(mut spans: Vec<HighlightSpan>) -> Vec<HighlightSpan> {
    spans.sort_by_key(|span| span.range.start);
    let mut merged: Vec<HighlightSpan> = Vec::new();
    for span in spans {
        if let Some(last) = merged.last_mut() {
            if span.range.start <= last.range.end {
                if span.range.end > last.range.end {
                    last.range.end = span.range.end;
                }
                continue;
            }
        }
        merged.push(span);
    }
    merged
}

fn is_identifier_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn previous_boundary(text: &str, offset: usize) -> usize {
    text.grapheme_indices(true)
        .rev()
        .find_map(|(index, _)| (index < offset).then_some(index))
        .unwrap_or(0)
}

fn next_boundary(text: &str, offset: usize) -> usize {
    text.grapheme_indices(true)
        .find_map(|(index, _)| (index > offset).then_some(index))
        .unwrap_or(text.len())
}

fn offset_to_utf16(text: &str, offset: usize) -> usize {
    let mut utf16_offset = 0usize;
    let mut utf8_count = 0usize;
    for ch in text.chars() {
        if utf8_count >= offset {
            break;
        }
        utf8_count += ch.len_utf8();
        utf16_offset += ch.len_utf16();
    }
    utf16_offset
}

fn offset_from_utf16(text: &str, offset: usize) -> usize {
    let mut utf8_offset = 0usize;
    let mut utf16_count = 0usize;
    for ch in text.chars() {
        if utf16_count >= offset {
            break;
        }
        utf16_count += ch.len_utf16();
        utf8_offset += ch.len_utf8();
    }
    utf8_offset
}

fn file_modified_at(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_round_trips_undo_and_redo() {
        let mut editor = EditorBuffer::restore("/tmp/test.rs", "fn main() {}", false);
        editor.move_to_offset(editor.text().len(), false);
        assert!(editor.insert_newline());
        assert!(editor.replace_selection("let x = 1;"));
        assert!(editor.dirty());

        assert!(editor.undo());
        assert!(editor.undo());
        assert_eq!(editor.text(), "fn main() {}");

        assert!(editor.redo());
        assert!(editor.redo());
        assert!(editor.text().contains("let x = 1;"));
    }

    #[test]
    fn search_tracks_matches() {
        let mut editor = EditorBuffer::restore("/tmp/test.rs", "alpha beta alpha", false);
        editor.open_search();
        editor.append_search_text("alpha");
        let matches = editor.search_matches();
        assert_eq!(matches.len(), 2);
        assert_eq!(editor.current_search_match(), Some(0..5));
        assert!(editor.next_search_match());
        assert_eq!(editor.current_search_match(), Some(11..16));
        editor.close_search();
        assert_eq!(editor.search_match_count(), 0);
        assert_eq!(editor.current_search_match(), None);
    }

    #[test]
    fn from_text_creates_buffer() {
        let editor = EditorBuffer::from_text("/tmp/test.rs".into(), "hello world".to_string());
        assert_eq!(editor.text(), "hello world");
        assert_eq!(editor.line_count(), 1);
        assert!(!editor.dirty());
    }

    #[test]
    fn document_version_increments_on_edit() {
        let mut editor = EditorBuffer::restore("/tmp/test.rs", "hello", false);
        assert_eq!(editor.document_version(), 0);
        editor.move_to_offset(5, false);
        editor.replace_selection(" world");
        assert_eq!(editor.document_version(), 1);
        editor.replace_selection("!");
        assert_eq!(editor.document_version(), 2);
    }

    #[test]
    fn pending_deltas_are_recorded() {
        let mut editor = EditorBuffer::restore("/tmp/test.rs", "hello", false);
        editor.move_to_offset(5, false);
        editor.replace_selection(" world");
        let deltas = editor.take_pending_deltas();
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].range, 5..5);
        assert_eq!(deltas[0].text, " world");
        // After taking, should be empty
        assert!(editor.take_pending_deltas().is_empty());
    }

    #[test]
    fn line_operations_work() {
        let editor = EditorBuffer::restore("/tmp/test.rs", "line1\nline2\nline3", false);
        assert_eq!(editor.line_count(), 3);
        assert_eq!(editor.line_text(0), "line1");
        assert_eq!(editor.line_text(1), "line2");
        assert_eq!(editor.line_text(2), "line3");
        assert_eq!(editor.line_range(0), 0..5);
        assert_eq!(editor.line_range(1), 6..11);
        assert_eq!(editor.line_range(2), 12..17);
    }

    #[test]
    fn line_count_with_trailing_newline() {
        let editor = EditorBuffer::restore("/tmp/test.rs", "line1\nline2\n", false);
        // Should be 2 lines (the old model didn't count the empty trailing line)
        assert_eq!(editor.line_count(), 2);
    }

    #[test]
    fn line_col_conversions() {
        let editor = EditorBuffer::restore("/tmp/test.rs", "abc\ndef\nghi", false);
        assert_eq!(editor.line_col_for_offset(0), (0, 0));
        assert_eq!(editor.line_col_for_offset(3), (0, 3));
        assert_eq!(editor.line_col_for_offset(4), (1, 0));
        assert_eq!(editor.line_col_for_offset(7), (1, 3));
        assert_eq!(editor.offset_for_line_col(0, 2), 2);
        assert_eq!(editor.offset_for_line_col(1, 1), 5);
        assert_eq!(editor.offset_for_line_col(2, 0), 8);
    }
}
