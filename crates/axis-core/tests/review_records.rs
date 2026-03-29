//! Serialization round-trips for desk review payloads exposed over automation.

use axis_core::review::{
    DeskReviewPayload, ReviewFileChangeKind, ReviewFileDiff, ReviewHunk, ReviewLine,
};
use axis_core::worktree::{ReviewSummary, WorktreeId};

#[test]
fn desk_review_payload_round_trips_structured_hunks() {
    let payload = DeskReviewPayload {
        worktree_id: WorktreeId::new("wt-demo"),
        summary: ReviewSummary {
            files_changed: 2,
            uncommitted_files: 1,
            ready_for_review: true,
            last_inspected_at_ms: Some(123),
        },
        files: vec![ReviewFileDiff {
            path: "src/lib.rs".to_string(),
            old_path: None,
            change_kind: ReviewFileChangeKind::Modified,
            added_lines: 1,
            removed_lines: 1,
            truncated: false,
            hunks: vec![ReviewHunk {
                header: "@@ -4,2 +4,2 @@".to_string(),
                old_start: 4,
                old_lines: 2,
                new_start: 4,
                new_lines: 2,
                anchor_new_line: Some(4),
                truncated: false,
                lines: vec![
                    ReviewLine::context(Some(4), Some(4), true, "fn demo() {"),
                    ReviewLine::removed(Some(5), None, true, "old_call();"),
                    ReviewLine::added(None, Some(5), true, "new_call();"),
                ],
            }],
        }],
        truncated: false,
    };

    let json = serde_json::to_value(&payload).unwrap();
    assert_eq!(json["files"][0]["change_kind"], "modified");
    assert_eq!(json["files"][0]["hunks"][0]["lines"][2]["kind"], "addition");

    let back: DeskReviewPayload = serde_json::from_value(json).unwrap();
    assert_eq!(back, payload);
}
