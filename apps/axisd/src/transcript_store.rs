use anyhow::Context;
use axis_core::terminal::{TerminalSessionId, TerminalTranscriptChunk};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Append-only transcript storage for daemon-owned PTY sessions.
#[derive(Clone, Debug)]
pub struct TranscriptStore {
    root: PathBuf,
}

#[allow(dead_code)]
impl TranscriptStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn append(&self, session_id: &TerminalSessionId, bytes: &[u8]) -> anyhow::Result<u64> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("create {}", self.root.display()))?;
        let path = self.path_for(session_id);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("append {}", path.display()))?;
        file.flush()
            .with_context(|| format!("flush {}", path.display()))?;

        Ok(fs::metadata(&path)
            .with_context(|| format!("stat {}", path.display()))?
            .len())
    }

    pub fn read_from(
        &self,
        session_id: &TerminalSessionId,
        offset: u64,
    ) -> anyhow::Result<Option<TerminalTranscriptChunk>> {
        let path = self.path_for(session_id);
        let Ok(mut file) = OpenOptions::new().read(true).open(&path) else {
            return Ok(None);
        };
        let length = file
            .metadata()
            .with_context(|| format!("stat {}", path.display()))?
            .len();
        if offset >= length {
            return Ok(None);
        }

        file.seek(SeekFrom::Start(offset))
            .with_context(|| format!("seek {}", path.display()))?;
        let mut bytes = Vec::with_capacity((length - offset) as usize);
        file.read_to_end(&mut bytes)
            .with_context(|| format!("read {}", path.display()))?;

        Ok(Some(TerminalTranscriptChunk {
            terminal_session_id: session_id.clone(),
            offset,
            bytes,
        }))
    }

    pub fn len(&self, session_id: &TerminalSessionId) -> anyhow::Result<u64> {
        let path = self.path_for(session_id);
        match fs::metadata(&path) {
            Ok(metadata) => Ok(metadata.len()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(error) => Err(error).with_context(|| format!("stat {}", path.display())),
        }
    }

    fn path_for(&self, session_id: &TerminalSessionId) -> PathBuf {
        self.root.join(transcript_file_name(session_id))
    }
}

fn transcript_file_name(session_id: &TerminalSessionId) -> String {
    format!("{}.log", sanitize_session_id(&session_id.0))
}

fn sanitize_session_id(session_id: &str) -> String {
    session_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

#[allow(dead_code)]
fn ensure_parent_dir(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    Ok(())
}
