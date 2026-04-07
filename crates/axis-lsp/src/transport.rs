use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};

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

pub fn write_message(writer: &mut impl Write, msg: &LspMessage) -> Result<()> {
    let body = serde_json::to_string(msg)?;
    write!(writer, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
    writer.flush()?;
    Ok(())
}

pub fn read_message(reader: &mut impl BufRead) -> Result<LspMessage> {
    // Read headers until blank line, extract Content-Length
    let mut content_length: Option<usize> = None;

    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .context("failed to read header line")?;

        // Strip CRLF or LF
        let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');

        if trimmed.is_empty() {
            // Blank line signals end of headers
            break;
        }

        if let Some(value) = trimmed.strip_prefix("Content-Length: ") {
            content_length = Some(
                value
                    .parse::<usize>()
                    .context("invalid Content-Length value")?,
            );
        }
        // Other headers (e.g. Content-Type) are silently ignored
    }

    let length = content_length.context("missing Content-Length header")?;

    let mut body = vec![0u8; length];
    reader
        .read_exact(&mut body)
        .context("failed to read message body")?;

    serde_json::from_slice(&body).context("failed to deserialize LSP message")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn write_and_read_message_round_trip() {
        let original = LspMessage::request(
            1,
            "textDocument/completion",
            serde_json::json!({"position": {"line": 0, "character": 5}}),
        );

        let mut buf: Vec<u8> = Vec::new();
        write_message(&mut buf, &original).expect("write_message failed");

        let mut cursor = Cursor::new(buf);
        let decoded = read_message(&mut cursor).expect("read_message failed");

        assert_eq!(decoded.jsonrpc, "2.0");
        assert_eq!(decoded.method.as_deref(), Some("textDocument/completion"));
        assert_eq!(
            decoded.id,
            Some(serde_json::Value::Number(1u64.into()))
        );
    }

    #[test]
    fn notification_has_no_id() {
        let msg = LspMessage::notification(
            "textDocument/didOpen",
            serde_json::json!({"uri": "file:///foo.rs"}),
        );
        assert!(msg.is_notification());
        assert!(!msg.is_response());
    }

    #[test]
    fn response_detection() {
        let msg = LspMessage {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(42)),
            method: None,
            params: None,
            result: Some(serde_json::json!({})),
            error: None,
        };
        assert!(msg.is_response());
        assert!(!msg.is_notification());
    }
}
