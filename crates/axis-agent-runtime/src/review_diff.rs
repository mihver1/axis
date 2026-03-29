//! Parse `git diff --unified=3` output into `axis_core::review` payloads.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context};
use axis_core::review::{
    DeskReviewPayload, ReviewFileChangeKind, ReviewFileDiff, ReviewHunk, ReviewLine,
    ReviewLineKind,
};
use axis_core::worktree::{ReviewSummary, WorktreeId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReviewPayloadLimits {
    pub max_files: usize,
    pub max_hunks_per_file: usize,
    pub max_lines_per_hunk: usize,
}

impl Default for ReviewPayloadLimits {
    fn default() -> Self {
        Self {
            max_files: 256,
            max_hunks_per_file: 64,
            max_lines_per_hunk: 2048,
        }
    }
}

pub(crate) fn build_desk_review_payload(
    root: &Path,
    base_branch: Option<&str>,
    working_tree_dirty: bool,
    limits: ReviewPayloadLimits,
) -> anyhow::Result<DeskReviewPayload> {
    let worktree_id = WorktreeId::new(root.display().to_string());
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64);

    let porcelain = git_stdout(root, &["status", "--porcelain", "-z"])?;
    let porcelain_entries = parse_porcelain(&porcelain);
    let porcelain_paths: BTreeSet<String> =
        porcelain_entries.iter().map(|e| e.path.clone()).collect();

    if !head_ok(root) {
        return Ok(build_unborn_payload(
            worktree_id,
            &porcelain_entries,
            limits,
            now_ms,
            porcelain_paths.len(),
            working_tree_dirty,
        ));
    }

    let review_base = resolve_review_base(root, base_branch)?;
    let diff_raw = git_stdout(root, &["diff", "--unified=3", &review_base])?;
    // Lower rename similarity so rename-with-edits still surfaces as `R` in name-status when possible.
    let name_status_raw =
        git_stdout(root, &["diff", "--name-status", "-M20", "-z", &review_base])?;
    let summary_raw = git_stdout(root, &["diff", "--summary", &review_base])?;

    let name_status_entries = parse_name_status(&name_status_raw);
    let name_status: BTreeMap<String, NameStatusEntry> = name_status_entries
        .iter()
        .cloned()
        .map(|entry| (entry.path.clone(), entry))
        .collect();
    let summary_renames = parse_diff_summary_renames(&summary_raw);
    let mut parsed_by_path = parse_unified_diff(&diff_raw, limits.max_lines_per_hunk);

    let mut all_paths: BTreeSet<String> = BTreeSet::new();
    for entry in &name_status_entries {
        all_paths.insert(entry.path.clone());
    }
    for p in parsed_by_path.keys() {
        all_paths.insert(p.clone());
    }
    for e in &porcelain_entries {
        all_paths.insert(e.path.clone());
    }

    let mut payload_truncated = all_paths.len() > limits.max_files;
    let paths_vec: Vec<String> = all_paths.into_iter().take(limits.max_files).collect();

    let mut files: Vec<ReviewFileDiff> = Vec::new();
    for path in paths_vec {
        let ns = name_status.get(&path);
        let pf = parsed_by_path.remove(&path);

        let mut change_kind = ns
            .map(|n| n.change_kind)
            .unwrap_or(ReviewFileChangeKind::Modified);
        let mut old_path = ns.and_then(|n| n.old_path.clone());

        let is_binary = pf.as_ref().map(|p| p.is_binary).unwrap_or(false);

        if let Some(pf) = pf.as_ref() {
            if let Some(prev) = &pf.rename_from {
                change_kind = ReviewFileChangeKind::Renamed;
                old_path = Some(prev.clone());
            }
        }
        if change_kind != ReviewFileChangeKind::Renamed {
            if let Some(prev) = summary_renames.get(&path) {
                change_kind = ReviewFileChangeKind::Renamed;
                old_path = Some(prev.clone());
            }
        }

        let mut hunks = pf.as_ref().map(|p| p.hunks.clone()).unwrap_or_default();
        if is_binary {
            hunks.clear();
        }

        if let Some(pe) = porcelain_entries.iter().find(|e| e.path == path) {
            if pe.is_untracked && ns.is_none() {
                change_kind = ReviewFileChangeKind::Added;
            }
            if pe.is_unmerged {
                change_kind = ReviewFileChangeKind::Modified;
            }
        }

        let mut file_trunc = pf.as_ref().map(|p| p.truncated).unwrap_or(false);
        if hunks.len() > limits.max_hunks_per_file {
            hunks.truncate(limits.max_hunks_per_file);
            file_trunc = true;
            payload_truncated = true;
        }

        let (added_lines, removed_lines) = count_delta_lines(&hunks);

        if file_trunc {
            payload_truncated = true;
        }

        files.push(ReviewFileDiff {
            path,
            old_path,
            change_kind,
            added_lines,
            removed_lines,
            truncated: file_trunc,
            hunks,
        });
    }

    let payload_has_reviewable_entries = !files.is_empty();
    let visible_uncommitted = files
        .iter()
        .filter(|f| porcelain_paths.contains(&f.path))
        .count() as u32;
    let uncommitted_files = if payload_truncated {
        visible_uncommitted
    } else {
        porcelain_paths.len() as u32
    };
    let summary = ReviewSummary {
        files_changed: files.len() as u32,
        uncommitted_files,
        ready_for_review: !working_tree_dirty && payload_has_reviewable_entries,
        last_inspected_at_ms: now_ms,
    };

    Ok(DeskReviewPayload {
        worktree_id,
        summary,
        files,
        truncated: payload_truncated,
    })
}

fn build_unborn_payload(
    worktree_id: WorktreeId,
    entries: &[PorcelainEntry],
    limits: ReviewPayloadLimits,
    now_ms: Option<u64>,
    porcelain_unique_paths: usize,
    working_tree_dirty: bool,
) -> DeskReviewPayload {
    let paths: BTreeSet<String> = entries.iter().map(|e| e.path.clone()).collect();
    let payload_truncated = paths.len() > limits.max_files;
    let paths_vec: Vec<String> = paths.into_iter().take(limits.max_files).collect();

    let files: Vec<ReviewFileDiff> = paths_vec
        .into_iter()
        .map(|path| ReviewFileDiff {
            path,
            old_path: None,
            change_kind: ReviewFileChangeKind::Added,
            added_lines: 0,
            removed_lines: 0,
            truncated: false,
            hunks: vec![],
        })
        .collect();

    let payload_has_reviewable_entries = !files.is_empty();
    let uncommitted_files = if payload_truncated {
        files.len() as u32
    } else {
        porcelain_unique_paths as u32
    };
    let summary = ReviewSummary {
        files_changed: files.len() as u32,
        uncommitted_files,
        ready_for_review: !working_tree_dirty && payload_has_reviewable_entries,
        last_inspected_at_ms: now_ms,
    };

    DeskReviewPayload {
        worktree_id,
        summary,
        files,
        truncated: payload_truncated,
    }
}

struct PorcelainEntry {
    path: String,
    is_untracked: bool,
    is_unmerged: bool,
}

pub(crate) fn parse_porcelain_paths(raw: &str) -> Vec<String> {
    parse_porcelain(raw)
        .into_iter()
        .map(|entry| entry.path)
        .collect()
}

fn parse_porcelain(raw: &str) -> Vec<PorcelainEntry> {
    let mut out = Vec::new();
    let tokens = raw.split('\0').filter(|token| !token.is_empty()).collect::<Vec<_>>();
    let mut index = 0;
    while index < tokens.len() {
        let record = tokens[index];
        if record.len() < 3 {
            index += 1;
            continue;
        }
        let status = &record[..2];
        let path = normalize_plain_path(&record[3..]);
        let is_untracked = status == "??";
        let is_unmerged = !is_untracked && porcelain_unmerged(status);

        out.push(PorcelainEntry {
            path,
            is_untracked,
            is_unmerged,
        });

        index += 1;
        if porcelain_has_secondary_path(status) && index < tokens.len() {
            index += 1;
        }
    }
    out
}

fn porcelain_unmerged(status: &str) -> bool {
    let b = status.as_bytes();
    if b.len() < 2 {
        return false;
    }
    b[0] == b'U' || b[1] == b'U'
}

fn porcelain_has_secondary_path(status: &str) -> bool {
    status.as_bytes().iter().any(|byte| matches!(byte, b'R' | b'C'))
}

#[derive(Clone)]
struct NameStatusEntry {
    path: String,
    change_kind: ReviewFileChangeKind,
    old_path: Option<String>,
}

fn parse_name_status(raw: &str) -> Vec<NameStatusEntry> {
    let tokens = raw.split('\0').filter(|token| !token.is_empty()).collect::<Vec<_>>();
    let mut entries = Vec::new();
    let mut index = 0;

    while index < tokens.len() {
        let tag = tokens[index];
        if tag.is_empty() {
            index += 1;
            continue;
        }

        if tag.starts_with('R') || tag.starts_with('C') {
            if index + 2 >= tokens.len() {
                break;
            }
            let old_path = normalize_plain_path(tokens[index + 1]);
            let new_path = normalize_plain_path(tokens[index + 2]);
            entries.push(NameStatusEntry {
                path: new_path,
                change_kind: ReviewFileChangeKind::Renamed,
                old_path: Some(old_path),
            });
            index += 3;
            continue;
        }

        if index + 1 >= tokens.len() {
            break;
        }

        let path = normalize_plain_path(tokens[index + 1]);
        let change_kind = match tag.as_bytes().first() {
            Some(b'A') => ReviewFileChangeKind::Added,
            Some(b'D') => ReviewFileChangeKind::Deleted,
            _ => ReviewFileChangeKind::Modified,
        };
        entries.push(NameStatusEntry {
            path,
            change_kind,
            old_path: None,
        });
        index += 2;
    }

    entries
}

struct ParsedDiffFile {
    hunks: Vec<ReviewHunk>,
    is_binary: bool,
    truncated: bool,
    /// Present when the diff chunk includes `rename from` / `rename to` (even if `--name-status` used D/A).
    rename_from: Option<String>,
}

fn parse_unified_diff(raw: &str, max_lines_per_hunk: usize) -> BTreeMap<String, ParsedDiffFile> {
    let mut map = BTreeMap::new();
    for chunk in diff_file_chunks(raw) {
        if let Some((path, parsed)) = parse_diff_file_chunk(chunk, max_lines_per_hunk) {
            map.insert(path, parsed);
        }
    }
    map
}

fn parse_diff_summary_renames(raw: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for line in raw.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("rename ") else {
            continue;
        };
        let summary = rest
            .rsplit_once(" (")
            .map(|(summary, _)| summary)
            .unwrap_or(rest)
            .trim();
        let Some((from, to)) = parse_summary_rename_pair(summary) else {
            continue;
        };
        m.insert(normalize_plain_path(&to), normalize_plain_path(&from));
    }
    m
}

fn parse_summary_rename_pair(summary: &str) -> Option<(String, String)> {
    if let Some(open_brace) = summary.find('{') {
        let close_brace = summary.rfind('}')?;
        let prefix = &summary[..open_brace];
        let suffix = &summary[close_brace + 1..];
        let inner = &summary[open_brace + 1..close_brace];
        let (from_inner, to_inner) = inner.split_once(" => ")?;
        return Some((
            format!("{prefix}{from_inner}{suffix}"),
            format!("{prefix}{to_inner}{suffix}"),
        ));
    }

    let (from, to) = summary.split_once(" => ")?;
    Some((from.to_string(), to.to_string()))
}

fn extract_rename_from_path(chunk: &str) -> Option<String> {
    for line in chunk.lines() {
        if line.starts_with("rename from ") {
            return Some(normalize_plain_path(&line["rename from ".len()..]));
        }
    }
    None
}

fn extract_rename_to_path(chunk: &str) -> Option<String> {
    for line in chunk.lines() {
        if line.starts_with("rename to ") {
            return Some(normalize_plain_path(&line["rename to ".len()..]));
        }
    }
    None
}

fn extract_patch_old_path(chunk: &str) -> Option<String> {
    for line in chunk.lines() {
        if let Some(rest) = line.strip_prefix("--- ") {
            return parse_patch_marker_path(rest);
        }
    }
    None
}

fn extract_patch_new_path(chunk: &str) -> Option<String> {
    for line in chunk.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            return parse_patch_marker_path(rest);
        }
    }
    None
}

fn parse_patch_marker_path(rest: &str) -> Option<String> {
    let token = rest
        .split('\t')
        .next()
        .unwrap_or(rest)
        .trim_end_matches('\r');
    if token == "/dev/null" {
        return None;
    }
    Some(normalize_git_path(token))
}

fn diff_file_chunks(raw: &str) -> Vec<&str> {
    let mut starts: Vec<usize> = raw
        .match_indices("\ndiff --git ")
        .map(|(i, _)| i + 1)
        .collect();
    if raw.starts_with("diff --git ") {
        starts.insert(0, 0);
    }
    if starts.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 0..starts.len() {
        let s = starts[i];
        let e = starts.get(i + 1).copied().unwrap_or(raw.len());
        if let Some(slice) = raw.get(s..e) {
            out.push(slice.trim_end_matches('\n'));
        }
    }
    out
}

fn parse_diff_file_chunk(
    chunk: &str,
    max_lines_per_hunk: usize,
) -> Option<(String, ParsedDiffFile)> {
    let mut lines = chunk.lines();
    let first = lines.next()?;
    let header_paths = parse_diff_git_paths(first);
    let path_key = extract_rename_to_path(chunk)
        .or_else(|| extract_patch_new_path(chunk))
        .or_else(|| header_paths.as_ref().map(|(_, path_b)| path_b.clone()))?;
    let path_a = extract_rename_from_path(chunk)
        .or_else(|| extract_patch_old_path(chunk))
        .or_else(|| header_paths.as_ref().map(|(path_a, _)| path_a.clone()))
        .unwrap_or_else(|| path_key.clone());

    if chunk.contains("Binary files ") && chunk.contains(" differ") {
        let rename_from = if path_a != path_key {
            Some(path_a.clone())
        } else {
            None
        };
        return Some((
            path_key,
            ParsedDiffFile {
                hunks: vec![],
                is_binary: true,
                truncated: false,
                rename_from,
            },
        ));
    }

    let rename_from = if path_a != path_key {
        Some(path_a)
    } else {
        None
    };

    let mut file_truncated = false;
    let mut hunks_out: Vec<ReviewHunk> = Vec::new();
    let mut in_hunk = false;
    let mut hunk_meta: Option<(u32, u32, u32, u32, String)> = None;
    let mut hunk_lines: Vec<ReviewLine> = Vec::new();
    let mut hunk_truncated = false;
    let mut old_cur: u32 = 0;
    let mut new_cur: u32 = 0;

    for line in lines {
        if line.starts_with("@@") {
            if in_hunk {
                if let Some((os, ol, ns, nl, hdr)) = hunk_meta.take() {
                    let body = std::mem::take(&mut hunk_lines);
                    let trunc = std::mem::replace(&mut hunk_truncated, false);
                    hunks_out.push(finish_hunk(os, ol, ns, nl, hdr, body, trunc));
                }
            }
            if let Some(parsed) = parse_hunk_header(line) {
                let (os, ol, ns, nl) = parsed;
                old_cur = os;
                new_cur = ns;
                hunk_meta = Some((os, ol, ns, nl, line.to_string()));
                in_hunk = true;
            }
            continue;
        }

        if !in_hunk {
            continue;
        }

        if line.starts_with('\\') {
            hunk_lines.push(ReviewLine::metadata(line));
            continue;
        }

        let Some(prefix) = line.as_bytes().first().copied() else {
            continue;
        };

        if prefix != b' ' && prefix != b'+' && prefix != b'-' {
            continue;
        }

        let text = line.get(1..).unwrap_or("").to_string();

        if hunk_truncated {
            continue;
        }

        if hunk_lines.len() >= max_lines_per_hunk {
            hunk_truncated = true;
            file_truncated = true;
            continue;
        }

        match prefix {
            b' ' => {
                hunk_lines.push(ReviewLine::context(
                    Some(old_cur),
                    Some(new_cur),
                    true,
                    text,
                ));
                old_cur = old_cur.saturating_add(1);
                new_cur = new_cur.saturating_add(1);
            }
            b'-' => {
                hunk_lines.push(ReviewLine::removed(Some(old_cur), None, true, text));
                old_cur = old_cur.saturating_add(1);
            }
            b'+' => {
                hunk_lines.push(ReviewLine::added(None, Some(new_cur), true, text));
                new_cur = new_cur.saturating_add(1);
            }
            _ => {}
        }
    }

    if in_hunk {
        if let Some((os, ol, ns, nl, hdr)) = hunk_meta.take() {
            let body = hunk_lines;
            hunks_out.push(finish_hunk(os, ol, ns, nl, hdr, body, hunk_truncated));
        }
    }

    Some((
        path_key,
        ParsedDiffFile {
            hunks: hunks_out,
            is_binary: false,
            truncated: file_truncated,
            rename_from,
        },
    ))
}

fn finish_hunk(
    old_start: u32,
    old_lines: u32,
    new_start: u32,
    new_lines: u32,
    header: String,
    lines: Vec<ReviewLine>,
    truncated: bool,
) -> ReviewHunk {
    let anchor = compute_anchor_new_line(new_start, &lines);
    ReviewHunk {
        header,
        old_start,
        old_lines,
        new_start,
        new_lines,
        anchor_new_line: anchor,
        truncated,
        lines,
    }
}

fn compute_anchor_new_line(new_start: u32, lines: &[ReviewLine]) -> Option<u32> {
    for row in lines {
        if matches!(row.kind, ReviewLineKind::Context | ReviewLineKind::Addition) {
            if let Some(n) = row.new_line {
                return Some(n);
            }
        }
    }
    Some(new_start)
}

fn normalize_plain_path(path: &str) -> String {
    decode_git_path(path)
}

fn normalize_git_path(p: &str) -> String {
    let decoded = normalize_plain_path(p);
    decoded
        .strip_prefix("a/")
        .or_else(|| decoded.strip_prefix("b/"))
        .unwrap_or(&decoded)
        .to_string()
}

fn parse_diff_git_paths(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("diff --git ")?;
    let (a_part, b_part) = rest.rsplit_once(" b/")?;
    Some((
        normalize_git_path(a_part),
        normalize_git_path(&format!("b/{b_part}")),
    ))
}

fn decode_git_path(raw: &str) -> String {
    if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        return unescape_git_quoted_path(&raw[1..raw.len() - 1]);
    }
    raw.to_string()
}

fn unescape_git_quoted_path(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        let byte = bytes[index];
        if byte != b'\\' {
            out.push(byte);
            index += 1;
            continue;
        }

        index += 1;
        if index >= bytes.len() {
            out.push(b'\\');
            break;
        }

        match bytes[index] {
            b'\\' => {
                out.push(b'\\');
                index += 1;
            }
            b'"' => {
                out.push(b'"');
                index += 1;
            }
            b'n' => {
                out.push(b'\n');
                index += 1;
            }
            b'r' => {
                out.push(b'\r');
                index += 1;
            }
            b't' => {
                out.push(b'\t');
                index += 1;
            }
            b'0'..=b'7' => {
                let mut value = 0u8;
                let mut consumed = 0usize;
                while index + consumed < bytes.len() && consumed < 3 {
                    let next = bytes[index + consumed];
                    if !(b'0'..=b'7').contains(&next) {
                        break;
                    }
                    value = (value << 3) + (next - b'0');
                    consumed += 1;
                }
                if consumed == 0 {
                    out.push(bytes[index]);
                    index += 1;
                } else {
                    out.push(value);
                    index += consumed;
                }
            }
            other => {
                out.push(other);
                index += 1;
            }
        }
    }

    String::from_utf8_lossy(&out).into_owned()
}

fn parse_hunk_header(line: &str) -> Option<(u32, u32, u32, u32)> {
    let trimmed = line.trim();
    let inner = trimmed.strip_prefix("@@")?.trim();
    let inner = inner.split("@@").next()?.trim();
    let (old_s, new_s) = inner.split_once(" +")?;
    let old_s = old_s.trim().strip_prefix('-')?;
    let (old_start, old_lines) = parse_range(old_s);
    let new_s = new_s.trim_start_matches('+');
    let (new_start, new_lines) = parse_range(new_s);
    Some((old_start, old_lines, new_start, new_lines))
}

fn parse_range(s: &str) -> (u32, u32) {
    let s = s.trim();
    if let Some((a, b)) = s.split_once(',') {
        let start = a.parse().unwrap_or(1);
        let len = b.parse().unwrap_or(0);
        (start, len)
    } else {
        let start = s.parse().unwrap_or(1);
        (start, 1)
    }
}

fn count_delta_lines(hunks: &[ReviewHunk]) -> (u32, u32) {
    let mut added = 0u32;
    let mut removed = 0u32;
    for h in hunks {
        for line in &h.lines {
            match line.kind {
                ReviewLineKind::Addition => added = added.saturating_add(1),
                ReviewLineKind::Removal => removed = removed.saturating_add(1),
                _ => {}
            }
        }
    }
    (added, removed)
}

fn head_ok(root: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--verify", "HEAD"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn ref_resolves(root: &Path, reference: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--verify", reference])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn resolve_review_base(root: &Path, base_branch: Option<&str>) -> anyhow::Result<String> {
    if let Some(b) = base_branch {
        if !ref_resolves(root, b) {
            return git_stdout(root, &["rev-parse", "HEAD"]).map(|s| s.trim().to_string());
        }

        let out = git_stdout(root, &["merge-base", b, "HEAD"])
            .with_context(|| format!("git merge-base {b} HEAD failed"))?;
        let t = out.trim();
        if t.is_empty() {
            return Err(anyhow!("git merge-base {b} HEAD returned empty output"));
        }
        return Ok(t.to_string());
    }
    git_stdout(root, &["rev-parse", "HEAD"]).map(|s| s.trim().to_string())
}

fn git_stdout(root: &Path, args: &[&str]) -> anyhow::Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .context("spawn git")?;
    if !out.status.success() {
        return Err(anyhow!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
