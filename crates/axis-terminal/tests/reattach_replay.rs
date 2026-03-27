use axis_terminal::{TerminalGridSize, TerminalReplayClient};

fn snapshot_text(snapshot: &axis_terminal::TerminalSnapshot) -> String {
    snapshot
        .rows
        .iter()
        .map(|row| {
            row.runs
                .iter()
                .map(|run| run.text.replace('\u{00A0}', " "))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn replay_client_reconstructs_snapshot_from_transcript_chunks() {
    let client = TerminalReplayClient::new("Reattach", TerminalGridSize::new(32, 6))
        .expect("client should construct");

    client
        .apply_bytes(b"printf ready\r\n")
        .expect("first chunk should replay");
    client
        .apply_bytes(b"line two\r\n")
        .expect("second chunk should replay");

    let text = snapshot_text(&client.snapshot());
    assert!(text.contains("printf ready"));
    assert!(text.contains("line two"));
}

#[test]
fn replay_client_late_attach_can_consume_full_history_at_once() {
    let client = TerminalReplayClient::new("Late Attach", TerminalGridSize::new(32, 6))
        .expect("client should construct");

    client
        .apply_bytes(b"line one\r\nline two\r\nline three\r\n")
        .expect("full transcript should replay");

    let text = snapshot_text(&client.snapshot());
    assert!(text.contains("line one"));
    assert!(text.contains("line two"));
    assert!(text.contains("line three"));
}
