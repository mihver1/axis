use std::fmt;

/// Classified agent runtime errors for context-appropriate UI recovery.
#[derive(Debug)]
pub enum AgentError {
    /// Session not found — likely already cleaned up or invalid ID
    SessionNotFound(String),
    /// Provider profile not registered
    ProviderNotFound(String),
    /// Operation not supported by this provider's capabilities
    UnsupportedOperation { provider: String, operation: String },
    /// Invalid lifecycle state transition
    InvalidTransition { from: String, to: String },
    /// Transient network or daemon communication error
    DaemonUnavailable(String),
    /// Provider process error (crash, timeout, etc.)
    ProviderError(String),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SessionNotFound(id) => write!(f, "session not found: {id}"),
            Self::ProviderNotFound(id) => write!(f, "provider not found: {id}"),
            Self::UnsupportedOperation { provider, operation } => {
                write!(f, "provider '{provider}' does not support {operation}")
            }
            Self::InvalidTransition { from, to } => {
                write!(f, "invalid lifecycle transition: {from} → {to}")
            }
            Self::DaemonUnavailable(msg) => write!(f, "daemon unavailable: {msg}"),
            Self::ProviderError(msg) => write!(f, "provider error: {msg}"),
        }
    }
}

impl std::error::Error for AgentError {}
