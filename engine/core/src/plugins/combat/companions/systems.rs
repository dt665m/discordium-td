use super::components::*;
use crate::{SimulationStep, TargetIndex};
use bevy::prelude::*;
pub fn tick_companions(
    step: Res<SimulationStep>,
    targets: Res<TargetIndex>,
    mut summons: Query<&mut CompanionState>,
) {
    for mut summon in &mut summons {
        summon.tick(step.0, &targets.0);
    }
}
