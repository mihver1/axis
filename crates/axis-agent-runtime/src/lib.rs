//! Agent session orchestration: provider contract, session manager, worktree helpers.

pub mod adapters;
pub mod events;
pub mod provider;
pub mod session;
pub mod worktree;

pub use events::RuntimeEvent;
pub use provider::{
    AgentProvider, ProviderProfileMetadata, ProviderRegistry, StartAgentRequest, StartedSession,
};
pub use session::SessionManager;
pub use worktree::WorktreeService;
