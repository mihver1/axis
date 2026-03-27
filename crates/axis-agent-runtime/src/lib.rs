//! Agent session orchestration: provider contract, session manager, worktree helpers.

pub mod adapters;
mod bin_resolver;
pub mod events;
pub mod provider;
pub mod session;
pub mod worktree;

pub use bin_resolver::provider_base_argv_from_env_or_default;
pub use events::RuntimeEvent;
pub use provider::{
    AgentProvider, ProviderProfileMetadata, ProviderRegistry, StartAgentRequest, StartedSession,
};
pub use session::SessionManager;
pub use worktree::WorktreeService;
