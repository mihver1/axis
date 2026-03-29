//! Agent session orchestration: provider contract, session manager, worktree helpers.

pub mod adapters;
mod bin_resolver;
pub mod cli_protocol;
pub mod events;
pub mod provider;
pub mod session;
mod review_diff;
pub mod worktree;

pub use review_diff::ReviewPayloadLimits;

pub use bin_resolver::{
    provider_base_argv_from_env_or_default, resolve_provider_command_from_env_or_default,
    resolve_provider_command_from_env_or_default_for_cwd,
    ProviderCommandResolution,
};
pub use events::RuntimeEvent;
pub use provider::{
    AgentProvider, ProviderProfileMetadata, ProviderRegistry, RespondApprovalRequest,
    ResumeRequest, SendTurnRequest, StartAgentRequest, StartedSession,
};
pub use session::SessionManager;
pub use worktree::WorktreeService;
