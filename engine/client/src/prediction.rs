//! Bounded replay shared by client-side predictors. Game adapters decide which edges remain unacknowledged.
pub fn replay_bounded<S, F>(
    state: &mut S,
    frames: impl IntoIterator<Item = F>,
    limit: usize,
    mut step: impl FnMut(&mut S, F),
) {
    for frame in frames.into_iter().take(limit) {
        step(state, frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replay_respects_budget_and_order() {
        let mut state = Vec::new();
        replay_bounded(&mut state, 0..100, 3, |state, frame| state.push(frame));
        assert_eq!(state, vec![0, 1, 2]);
    }
}
