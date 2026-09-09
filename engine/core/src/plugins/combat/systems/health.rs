use super::super::{Depleted, Health};
use bevy::prelude::*;

pub fn update_health(mut commands: Commands, actors: Query<(Entity, &Health, Has<Depleted>)>) {
    for (entity, health, depleted) in &actors {
        if health.hp <= 0.0 && !depleted {
            commands.entity(entity).insert(Depleted);
        } else if health.hp > 0.0 && depleted {
            commands.entity(entity).remove::<Depleted>();
        }
    }
}
