//! Shared terminal session ids and transcript payloads for daemon reattach.

use crate::workdesk::WorkdeskId;
use crate::SurfaceId;
use serde::{Deserialize, Serialize};

/// Stable daemon-owned terminal session identifier.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TerminalSessionId(pub String);

impl TerminalSessionId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

/// Terminal-capable surface kinds supported by the daemon runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalSurfaceKind {
    Shell,
    Agent,
}

/// Metadata for a daemon-owned terminal session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TerminalSessionRecord {
    pub terminal_session_id: TerminalSessionId,
    pub workdesk_id: WorkdeskId,
    pub surface_id: SurfaceId,
    pub kind: TerminalSurfaceKind,
    pub title: String,
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
    pub transcript_len: u64,
    pub closed: bool,
}

/// Append-only transcript chunk returned to reconnecting clients.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TerminalTranscriptChunk {
    pub terminal_session_id: TerminalSessionId,
    pub offset: u64,
    pub bytes: Vec<u8>,
}
