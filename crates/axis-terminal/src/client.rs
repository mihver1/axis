use super::{
    GhosttyEngine, TerminalGridSize, TerminalRow, TerminalScrollbar, TerminalSnapshot, TerminalTheme,
};
use anyhow::Result;
use std::sync::Mutex;

/// Ghostty-backed client-side terminal emulator used for transcript replay.
pub struct TerminalReplayClient {
    fallback_title: String,
    engine: Mutex<GhosttyEngine>,
}

impl TerminalReplayClient {
    pub fn new(fallback_title: impl Into<String>, grid: TerminalGridSize) -> Result<Self> {
        Ok(Self {
            fallback_title: fallback_title.into(),
            engine: Mutex::new(GhosttyEngine::new(grid)?),
        })
    }

    pub fn apply_bytes(&self, bytes: &[u8]) -> Result<()> {
        self.engine
            .lock()
            .expect("ghostty replay client mutex poisoned")
            .write(bytes)
    }

    pub fn resize(&self, grid: TerminalGridSize) -> Result<()> {
        self.engine
            .lock()
            .expect("ghostty replay client mutex poisoned")
            .resize(grid)
    }

    pub fn scroll_viewport_delta(&self, delta: isize) -> Result<()> {
        if delta == 0 {
            return Ok(());
        }

        self.engine
            .lock()
            .expect("ghostty replay client mutex poisoned")
            .scroll_viewport_delta(delta)
    }

    pub fn scroll_viewport_bottom(&self) -> Result<()> {
        self.engine
            .lock()
            .expect("ghostty replay client mutex poisoned")
            .scroll_viewport_bottom()
    }

    pub fn snapshot(&self) -> TerminalSnapshot {
        match self.engine.lock() {
            Ok(mut engine) => engine.snapshot(&self.fallback_title).unwrap_or_else(|error| {
                replay_error_snapshot(self.fallback_title.clone(), format!(
                    "ghostty-vt snapshot failed: {error}"
                ))
            }),
            Err(_) => replay_error_snapshot(
                self.fallback_title.clone(),
                "ghostty-vt state mutex poisoned".to_string(),
            ),
        }
    }
}

fn replay_error_snapshot(title: String, message: String) -> TerminalSnapshot {
    TerminalSnapshot {
        title,
        rows: vec![TerminalRow::plain(message, &TerminalTheme::default())],
        theme: TerminalTheme::default(),
        cursor: (0, 0),
        cursor_blinking: false,
        cols: 0,
        rows_count: 0,
        scrollbar: TerminalScrollbar::default(),
        alternate_screen: false,
        application_cursor: false,
        closed: false,
        status: None,
    }
}
