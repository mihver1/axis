//! Agent session identifiers, lifecycle, attention, and shared record shape.

use crate::SurfaceId;
use serde::{Deserialize, Serialize};

/// Stable identifier for an agent session (opaque to callers).
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentSessionId(pub String);

impl AgentSessionId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

/// Normalized lifecycle across CLI-wrapped and native ACP providers.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentLifecycle {
    Planned,
    Starting,
    Running,
    Waiting,
    Completed,
    Failed,
    Cancelled,
}

/// User-facing attention routing signal (orthogonal to lifecycle).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentAttention {
    Quiet,
    Working,
    NeedsInput,
    NeedsReview,
    Error,
}

/// How the runtime talks to the provider for this session.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTransportKind {
    CliWrapped,
    NativeAcp,
}

/// Portable agent session snapshot for UI, automation, and persistence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AgentSessionRecord {
    pub id: AgentSessionId,
    pub provider_profile_id: String,
    pub transport: AgentTransportKind,
    pub workdesk_id: Option<String>,
    /// Pane surface that renders this session’s terminal/UI attachment, when any.
    pub surface_id: Option<SurfaceId>,
    pub cwd: String,
    pub lifecycle: AgentLifecycle,
    pub attention: AgentAttention,
    pub status_message: String,
}
