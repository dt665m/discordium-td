use super::components::*;
use crate::{SimulationStep, TargetIndex};
use bevy::prelude::*;
pub fn tick_projectiles(
    step: Res<SimulationStep>,
    targets: Res<TargetIndex>,
    mut projectiles: Query<&mut ProjectileState>,
) {
    for mut projectile in &mut projectiles {
        projectile.tick(step.0, &targets.0);
    }
}

pub fn advance_projectiles(
    step: Res<SimulationStep>,
    targets: Res<TargetIndex>,
    mut projectiles: Query<&mut ProjectileState>,
) {
    for mut projectile in &mut projectiles {
        projectile.advance(step.0, &targets.0);
    }
}
