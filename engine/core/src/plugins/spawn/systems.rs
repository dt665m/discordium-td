use super::Lifetime;
use crate::SimulationStep;
use bevy::prelude::*;

pub fn expire_lifetimes(
    mut commands: Commands,
    step: Res<SimulationStep>,
    mut actors: Query<(Entity, &mut Lifetime)>,
) {
    if !step.0.is_finite() || step.0 < 0.0 {
        return;
    }
    for (entity, mut lifetime) in &mut actors {
        let next = lifetime.0 - step.0;
        if !next.is_finite() || next <= 0.0 {
            // An owner may already have removed this entity through linked cleanup.
            commands.entity(entity).try_despawn();
        } else {
            lifetime.set_if_neq(Lifetime(next));
        }
    }
}
