use super::{AttentionState, PaneAttention, WorkdeskAttentionSummary};
use axis_core::{agent::AgentAttention, PaneId};

pub(crate) fn agent_attention_state(attention: AgentAttention) -> AttentionState {
    match attention {
        AgentAttention::Quiet => AttentionState::Idle,
        AgentAttention::Working => AttentionState::Working,
        AgentAttention::NeedsInput => AttentionState::NeedsInput,
        AgentAttention::NeedsReview => AttentionState::NeedsReview,
        AgentAttention::Error => AttentionState::Error,
    }
}

pub(crate) fn reduce_pane_attention_state<I>(
    baseline: AttentionState,
    session_attentions: I,
) -> AttentionState
where
    I: IntoIterator<Item = AgentAttention>,
{
    session_attentions.into_iter().fold(baseline, |current, attention| {
        let next = agent_attention_state(attention);
        if next.summary_priority() > current.summary_priority() {
            next
        } else {
            current
        }
    })
}

pub(crate) fn summarize_workdesk_attention<I>(pane_attentions: I) -> WorkdeskAttentionSummary
where
    I: IntoIterator<Item = PaneAttention>,
{
    let mut summary = WorkdeskAttentionSummary::default();
    for attention in pane_attentions {
        summary.register(attention);
    }
    summary
}

pub(crate) fn next_attention_pane_target<I>(panes: I) -> Option<PaneId>
where
    I: IntoIterator<Item = (PaneId, PaneAttention)>,
{
    panes.into_iter()
        .filter_map(|(pane_id, attention)| {
            attention_jump_key(attention, pane_id).map(|key| (key, pane_id))
        })
        .min_by_key(|(key, _)| *key)
        .map(|(_, pane_id)| pane_id)
}

pub(crate) fn next_attention_workdesk_target<I, J>(workdesks: I) -> Option<(usize, PaneId)>
where
    I: IntoIterator<Item = (usize, J)>,
    J: IntoIterator<Item = (PaneId, PaneAttention)>,
{
    let mut best: Option<((u8, u64, usize, u64), PaneId)> = None;

    for (desk_index, panes) in workdesks {
        for (pane_id, attention) in panes {
            let Some((priority, sequence, pane_raw)) = attention_jump_key(attention, pane_id) else {
                continue;
            };
            let candidate_key = (priority, sequence, desk_index, pane_raw);
            if best
                .as_ref()
                .map_or(true, |(best_key, _)| candidate_key < *best_key)
            {
                best = Some((candidate_key, pane_id));
            }
        }
    }

    best.map(|((_, _, desk_index, _), pane_id)| (desk_index, pane_id))
}

pub(crate) fn should_notify_attention_transition(
    previous: AttentionState,
    next: AttentionState,
) -> bool {
    previous != next && next.should_notify()
}

fn attention_jump_key(attention: PaneAttention, pane_id: PaneId) -> Option<(u8, u64, u64)> {
    if !attention.unread || !attention.state.is_attention() {
        return None;
    }

    Some((
        attention.state.jump_priority(),
        attention.last_attention_sequence.max(1),
        pane_id.raw(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane_attention(
        state: AttentionState,
        unread: bool,
        last_attention_sequence: u64,
    ) -> PaneAttention {
        PaneAttention {
            state,
            unread,
            last_attention_sequence,
            last_activity_sequence: last_attention_sequence,
        }
    }

    #[test]
    fn attention_reduce_session_attention_prefers_error_over_working() {
        let reduced = reduce_pane_attention_state(
            AttentionState::Working,
            [AgentAttention::Working, AgentAttention::Error],
        );

        assert_eq!(reduced, AttentionState::Error);
    }

    #[test]
    fn attention_summarize_workdesk_attention_handles_multiple_panes() {
        let summary = summarize_workdesk_attention([
            pane_attention(AttentionState::Working, false, 1),
            pane_attention(AttentionState::NeedsReview, true, 2),
            pane_attention(AttentionState::Error, false, 3),
        ]);

        assert_eq!(
            summary,
            WorkdeskAttentionSummary {
                highest: AttentionState::Error,
                unread_count: 1,
            }
        );
    }

    #[test]
    fn attention_next_target_prefers_input_then_review_then_error() {
        let target = next_attention_workdesk_target([
            (
                0usize,
                vec![(PaneId::new(1), pane_attention(AttentionState::Error, true, 1))],
            ),
            (
                1usize,
                vec![(
                    PaneId::new(2),
                    pane_attention(AttentionState::NeedsReview, true, 2),
                )],
            ),
            (
                2usize,
                vec![(
                    PaneId::new(3),
                    pane_attention(AttentionState::NeedsInput, true, 9),
                )],
            ),
        ]);

        assert_eq!(target, Some((2, PaneId::new(3))));
    }

    #[test]
    fn attention_next_target_is_stable_for_equal_priorities() {
        let target = next_attention_workdesk_target([
            (
                1usize,
                vec![(
                    PaneId::new(4),
                    pane_attention(AttentionState::NeedsReview, true, 5),
                )],
            ),
            (
                0usize,
                vec![(
                    PaneId::new(2),
                    pane_attention(AttentionState::NeedsReview, true, 5),
                )],
            ),
        ]);

        assert_eq!(target, Some((0, PaneId::new(2))));
    }
}
