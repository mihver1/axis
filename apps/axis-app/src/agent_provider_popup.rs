//! State and pure helpers for the agent provider chooser popup (UI in a later task).

use crate::agent_sessions::ProviderProfileOption;
use axis_core::{PaneId, Point};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AgentLaunchTarget {
    NewPane { world_center: Point },
    StackIntoPane(PaneId),
}

pub struct AgentProviderPopupState {
    #[allow(dead_code)]
    pub desk_index: usize,
    #[allow(dead_code)]
    pub target: AgentLaunchTarget,
    pub options: Vec<ProviderProfileOption>,
}

impl AgentProviderPopupState {
    pub fn new(
        desk_index: usize,
        target: AgentLaunchTarget,
        options: Vec<ProviderProfileOption>,
    ) -> Self {
        Self {
            desk_index,
            target,
            options,
        }
    }

    pub fn allows_selection(&self, profile_id: &str) -> bool {
        self.options
            .iter()
            .find(|o| o.profile_id == profile_id)
            .is_some_and(|o| o.available)
    }

    pub fn has_available_options(&self) -> bool {
        self.options.iter().any(|option| option.available)
    }

    pub fn empty_state_message(&self) -> Option<&'static str> {
        if self.has_available_options() {
            return None;
        }

        if self.all_unavailability_looks_install_related() {
            Some("No installed agent backends found")
        } else {
            Some("No available agent backends found")
        }
    }

    fn all_unavailability_looks_install_related(&self) -> bool {
        self.options.is_empty()
            || self.options.iter().all(|option| {
                option
                    .unavailable_reason
                    .as_deref()
                    .map(reason_looks_install_related)
                    .unwrap_or(true)
            })
    }
}

fn reason_looks_install_related(reason: &str) -> bool {
    let normalized = reason.to_ascii_lowercase();
    normalized.contains("not found")
        || normalized.contains("missing")
        || normalized.contains("not installed")
        || normalized.contains("no such file")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_options_mixed_availability() -> Vec<ProviderProfileOption> {
        vec![
            ProviderProfileOption {
                profile_id: "codex".to_string(),
                capability_note: None,
                available: true,
                unavailable_reason: None,
            },
            ProviderProfileOption {
                profile_id: "claude-code".to_string(),
                capability_note: Some("basic lifecycle only".to_string()),
                available: false,
                unavailable_reason: Some("binary not found".to_string()),
            },
        ]
    }

    #[test]
    fn provider_popup_state_rejects_unavailable_profile_selection() {
        let state = AgentProviderPopupState::new(
            0,
            AgentLaunchTarget::NewPane {
                world_center: Point::new(0.0, 0.0),
            },
            sample_options_mixed_availability(),
        );
        assert!(state.allows_selection("codex"));
        assert!(!state.allows_selection("claude-code"));
        assert!(!state.allows_selection("missing"));
    }

    #[test]
    fn provider_popup_state_has_no_empty_state_when_any_row_is_available() {
        let state = AgentProviderPopupState::new(
            0,
            AgentLaunchTarget::NewPane {
                world_center: Point::new(0.0, 0.0),
            },
            sample_options_mixed_availability(),
        );
        assert!(state.has_available_options());
        assert_eq!(state.empty_state_message(), None);
    }

    #[test]
    fn provider_popup_state_reports_empty_state_when_all_rows_disabled() {
        let options = vec![
            ProviderProfileOption {
                profile_id: "codex".to_string(),
                capability_note: None,
                available: false,
                unavailable_reason: Some("missing".to_string()),
            },
            ProviderProfileOption {
                profile_id: "claude-code".to_string(),
                capability_note: None,
                available: false,
                unavailable_reason: Some("missing".to_string()),
            },
        ];
        let state = AgentProviderPopupState::new(
            0,
            AgentLaunchTarget::StackIntoPane(PaneId::new(1)),
            options,
        );
        assert!(!state.has_available_options());
        assert_eq!(
            state.empty_state_message(),
            Some("No installed agent backends found")
        );
    }

    #[test]
    fn provider_popup_state_reports_generic_empty_state_for_non_install_failures() {
        let options = vec![
            ProviderProfileOption {
                profile_id: "codex".to_string(),
                capability_note: None,
                available: false,
                unavailable_reason: Some("Configured path is not executable".to_string()),
            },
            ProviderProfileOption {
                profile_id: "claude-code".to_string(),
                capability_note: None,
                available: false,
                unavailable_reason: Some("Configured path is not executable".to_string()),
            },
        ];
        let state = AgentProviderPopupState::new(
            0,
            AgentLaunchTarget::StackIntoPane(PaneId::new(1)),
            options,
        );
        assert_eq!(
            state.empty_state_message(),
            Some("No available agent backends found")
        );
    }
}
