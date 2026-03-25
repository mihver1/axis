use std::{
    fs,
    ops::Range,
    path::{Path, PathBuf},
    time::SystemTime,
};

use unicode_segmentation::UnicodeSegmentation;

const TAB_TEXT: &str = "    ";

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
struct EditorSnapshot {
    text: String,
    selection: Selection,
    marked_range: Option<Range<usize>>,
    dirty: bool,
    scroll_top_line: usize,
}

#[derive(Clone, Debug)]
pub struct EditorBuffer {
    path: PathBuf,
    text: String,
    saved_text: String,
    dirty: bool,
    selection: Selection,
    marked_range: Option<Range<usize>>,
    preferred_column: Option<usize>,
    scroll_top_line: usize,
    search: SearchState,
    line_starts: Vec<usize>,
    language: LanguageKind,
    undo_stack: Vec<EditorSnapshot>,
    redo_stack: Vec<EditorSnapshot>,
    last_synced_modified_at: Option<SystemTime>,
    external_modified: bool,
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
        let saved_text = if dirty {
            fs::read_to_string(&path).unwrap_or_default()
        } else {
            text.clone()
        };
        let language = detect_language(&path);
        let mut this = Self {
            path,
            text,
            saved_text,
            dirty,
            selection: Selection::default(),
            marked_range: None,
            preferred_column: None,
            scroll_top_line: 0,
            search: SearchState::default(),
            line_starts: vec![0],
            language,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_synced_modified_at: None,
            external_modified: false,
        };
        this.rebuild_line_starts();
        this.selection.range = 0..0;
        this.last_synced_modified_at = file_modified_at(&this.path);
        this
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

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn persisted_buffer_text(&self) -> Option<&str> {
        self.dirty.then_some(self.text.as_str())
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
        self.line_starts.len().max(1)
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

    pub fn line_text(&self, line_index: usize) -> &str {
        let range = self.line_range(line_index);
        &self.text[range]
    }

    pub fn line_range(&self, line_index: usize) -> Range<usize> {
        let line_index = line_index.min(self.line_count().saturating_sub(1));
        let start = self.line_starts[line_index];
        let end = self
            .line_starts
            .get(line_index + 1)
            .copied()
            .unwrap_or(self.text.len());
        let end = if end > start && self.text.as_bytes().get(end - 1) == Some(&b'\n') {
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

    pub fn selected_text(&self) -> Option<&str> {
        (!self.selection.range.is_empty()).then_some(&self.text[self.selection.range.clone()])
    }

    pub fn move_left(&mut self, selecting: bool) {
        if !selecting && !self.selection.range.is_empty() {
            self.move_to(self.selection.range.start, false);
            return;
        }
        let target = previous_boundary(&self.text, self.cursor_offset());
        self.move_to(target, selecting);
    }

    pub fn move_right(&mut self, selecting: bool) {
        if !selecting && !self.selection.range.is_empty() {
            self.move_to(self.selection.range.end, false);
            return;
        }
        let target = next_boundary(&self.text, self.cursor_offset());
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
            range: 0..self.text.len(),
            reversed: false,
        };
        self.preferred_column = None;
    }

    pub fn backspace(&mut self) -> bool {
        if self.selection.range.is_empty() {
            let previous = previous_boundary(&self.text, self.cursor_offset());
            self.selection.range = previous..self.cursor_offset();
            self.selection.reversed = false;
        }
        self.replace_selection("")
    }

    pub fn delete_forward(&mut self) -> bool {
        if self.selection.range.is_empty() {
            let next = next_boundary(&self.text, self.cursor_offset());
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
        self.record_undo_state();
        let changed = self.replace_internal(None, replacement, None);
        if changed {
            self.redo_stack.clear();
        } else {
            self.undo_stack.pop();
        }
        changed
    }

    pub fn replace_text_in_range_utf16(
        &mut self,
        range_utf16: Option<Range<usize>>,
        replacement: &str,
    ) -> bool {
        self.record_undo_state();
        let changed = self.replace_internal(range_utf16, replacement, None);
        if changed {
            self.redo_stack.clear();
        } else {
            self.undo_stack.pop();
        }
        changed
    }

    pub fn replace_and_mark_text_in_range_utf16(
        &mut self,
        range_utf16: Option<Range<usize>>,
        replacement: &str,
        selected_range_utf16: Option<Range<usize>>,
    ) -> bool {
        self.record_undo_state();
        let changed = self.replace_internal(range_utf16, replacement, selected_range_utf16);
        if changed {
            self.redo_stack.clear();
        } else {
            self.undo_stack.pop();
        }
        changed
    }

    pub fn undo(&mut self) -> bool {
        let Some(snapshot) = self.undo_stack.pop() else {
            return false;
        };
        self.redo_stack.push(self.snapshot());
        self.restore_snapshot(snapshot);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(snapshot) = self.redo_stack.pop() else {
            return false;
        };
        self.undo_stack.push(self.snapshot());
        self.restore_snapshot(snapshot);
        true
    }

    pub fn save(&mut self) -> Result<(), String> {
        fs::write(&self.path, &self.text)
            .map_err(|error| format!("write {}: {error}", self.path.display()))?;
        self.saved_text = self.text.clone();
        self.dirty = false;
        self.external_modified = false;
        self.last_synced_modified_at = file_modified_at(&self.path);
        Ok(())
    }

    pub fn reload(&mut self) -> Result<(), String> {
        let reloaded = fs::read_to_string(&self.path)
            .map_err(|error| format!("read {}: {error}", self.path.display()))?;
        self.text = reloaded.clone();
        self.saved_text = reloaded;
        self.dirty = false;
        self.selection.range = 0..0;
        self.selection.reversed = false;
        self.marked_range = None;
        self.preferred_column = None;
        self.external_modified = false;
        self.last_synced_modified_at = file_modified_at(&self.path);
        self.rebuild_line_starts();
        self.recompute_search_matches(false);
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
        self.search.query.clear();
        self.search.active_match = None;
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
        let matches = self.search_matches();
        if matches.is_empty() {
            return false;
        }
        let next = self
            .search
            .active_match
            .map(|index| (index + 1) % matches.len())
            .unwrap_or(0);
        self.activate_search_match(next);
        true
    }

    pub fn previous_search_match(&mut self) -> bool {
        let matches = self.search_matches();
        if matches.is_empty() {
            return false;
        }
        let previous = self
            .search
            .active_match
            .map(|index| {
                if index == 0 {
                    matches.len() - 1
                } else {
                    index - 1
                }
            })
            .unwrap_or(matches.len() - 1);
        self.activate_search_match(previous);
        true
    }

    pub fn search_matches(&self) -> Vec<Range<usize>> {
        if self.search.query.is_empty() {
            return Vec::new();
        }
        let query = self.search.query.to_ascii_lowercase();
        let haystack = self.text.to_ascii_lowercase();
        let mut matches = Vec::new();
        let mut start = 0usize;
        while let Some(found) = haystack[start..].find(&query) {
            let begin = start + found;
            let end = begin + query.len();
            matches.push(begin..end);
            start = end.max(begin + 1);
        }
        matches
    }

    pub fn current_search_match(&self) -> Option<Range<usize>> {
        let matches = self.search_matches();
        self.search
            .active_match
            .and_then(|index| matches.get(index).cloned())
    }

    pub fn highlight_line(&self, line_index: usize) -> Vec<HighlightSpan> {
        let text = self.line_text(line_index);
        lexical_highlight_line(self.language, text)
    }

    pub fn move_to_offset(&mut self, offset: usize, selecting: bool) {
        self.move_to(offset, selecting);
    }

    pub fn offset_to_utf16(&self, offset: usize) -> usize {
        offset_to_utf16(&self.text, offset)
    }

    pub fn offset_from_utf16(&self, offset: usize) -> usize {
        offset_from_utf16(&self.text, offset)
    }

    pub fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    pub fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    pub fn line_col_for_offset(&self, offset: usize) -> (usize, usize) {
        let offset = offset.min(self.text.len());
        let line = self
            .line_starts
            .partition_point(|start| *start <= offset)
            .saturating_sub(1);
        let line_start = self.line_starts[line];
        (line, offset.saturating_sub(line_start))
    }

    pub fn offset_for_line_col(&self, line: usize, column: usize) -> usize {
        let range = self.line_range(line);
        (range.start + column).min(range.end)
    }

    fn move_to(&mut self, offset: usize, selecting: bool) {
        let offset = offset.min(self.text.len());
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
        let matches = self.search_matches();
        let Some(range) = matches.get(index).cloned() else {
            self.search.active_match = None;
            return;
        };
        self.selection.range = range;
        self.selection.reversed = false;
        self.search.active_match = Some(index);
    }

    fn recompute_search_matches(&mut self, snap_to_first: bool) {
        let matches = self.search_matches();
        if matches.is_empty() {
            self.search.active_match = None;
            return;
        }
        if snap_to_first {
            let cursor = self.cursor_offset();
            let index = matches
                .iter()
                .position(|range| range.start >= cursor)
                .unwrap_or(0);
            self.activate_search_match(index);
        } else if let Some(index) = self.search.active_match {
            if index >= matches.len() {
                self.search.active_match = Some(matches.len() - 1);
            }
        }
    }

    fn replace_internal(
        &mut self,
        range_utf16: Option<Range<usize>>,
        replacement: &str,
        selected_range_utf16: Option<Range<usize>>,
    ) -> bool {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selection.range.clone());

        if range.start > self.text.len() || range.end > self.text.len() || range.start > range.end {
            return false;
        }

        let mut next_text = String::with_capacity(
            self.text.len().saturating_sub(range.end - range.start) + replacement.len(),
        );
        next_text.push_str(&self.text[..range.start]);
        next_text.push_str(replacement);
        next_text.push_str(&self.text[range.end..]);

        let changed = next_text != self.text;
        self.text = next_text;
        self.dirty = self.text != self.saved_text;
        self.external_modified = false;
        self.marked_range = if replacement.is_empty() {
            None
        } else {
            Some(range.start..range.start + replacement.len())
        };
        self.selection.range = selected_range_utf16
            .as_ref()
            .map(|selection| self.range_from_utf16(selection))
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
        self.rebuild_line_starts();
        self.recompute_search_matches(false);
        changed
    }

    fn record_undo_state(&mut self) {
        self.undo_stack.push(self.snapshot());
        if self.undo_stack.len() > 200 {
            self.undo_stack.remove(0);
        }
    }

    fn snapshot(&self) -> EditorSnapshot {
        EditorSnapshot {
            text: self.text.clone(),
            selection: self.selection.clone(),
            marked_range: self.marked_range.clone(),
            dirty: self.dirty,
            scroll_top_line: self.scroll_top_line,
        }
    }

    fn restore_snapshot(&mut self, snapshot: EditorSnapshot) {
        self.text = snapshot.text;
        self.selection = snapshot.selection;
        self.marked_range = snapshot.marked_range;
        self.dirty = snapshot.dirty;
        self.scroll_top_line = snapshot.scroll_top_line;
        self.preferred_column = None;
        self.rebuild_line_starts();
        self.recompute_search_matches(false);
    }

    fn rebuild_line_starts(&mut self) {
        self.line_starts.clear();
        self.line_starts.push(0);
        for (index, ch) in self.text.char_indices() {
            if ch == '\n' && index + 1 <= self.text.len() {
                self.line_starts.push(index + 1);
            }
        }
        if self.line_starts.is_empty() {
            self.line_starts.push(0);
        }
        self.scroll_top_line = self
            .scroll_top_line
            .min(self.line_count().saturating_sub(1));
    }
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
    }
}
