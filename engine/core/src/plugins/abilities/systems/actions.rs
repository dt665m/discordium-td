use super::super::ActionState;
use crate::SimulationStep;
use bevy::prelude::*;

pub fn tick_actions(step: Res<SimulationStep>, mut actions: Query<&mut ActionState>) {
    for mut action in &mut actions {
        let mut next = *action;
        next.tick(step.0);
        action.set_if_neq(next);
    }
}
