use super::super::DelayedAction;
use crate::SimulationStep;
use bevy::prelude::*;

pub fn tick_delayed_actions<T: Send + Sync + 'static>(
    step: Res<SimulationStep>,
    mut actions: Query<&mut DelayedAction<T>>,
) {
    if !step.0.is_finite() || step.0 < 0.0 {
        return;
    }
    for mut action in &mut actions {
        let remaining = (action.remaining - step.0).max(0.0);
        let ready = action.ready || remaining == 0.0;
        if action.remaining != remaining || action.ready != ready {
            action.tick(step.0);
        }
    }
}
