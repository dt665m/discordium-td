use super::GraphicsInstance;
use bevy::prelude::*;

pub fn age_graphics(mut commands: Commands, mut effects: Query<(Entity, &mut GraphicsInstance)>) {
    for (entity, mut effect) in &mut effects {
        effect.age_ticks = effect.age_ticks.saturating_add(1);
        if effect.age_ticks >= effect.duration_ticks {
            commands.entity(entity).despawn();
        }
    }
}
