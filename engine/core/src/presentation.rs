use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct PresentationId {
    pub match_epoch: u32,
    pub owner: u64,
    pub action_seq: u32,
    pub slot: u16,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum PresentationKind {
    RadialPulse,
}
#[derive(Component, Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct PresentationInstance {
    pub id: PresentationId,
    pub kind: PresentationKind,
    pub pos: [f32; 2],
    pub radius: f32,
    pub age_ticks: u32,
    pub duration_ticks: u32,
}
pub fn age_presentations(
    mut commands: Commands,
    mut effects: Query<(Entity, &mut PresentationInstance)>,
) {
    for (entity, mut effect) in &mut effects {
        effect.age_ticks = effect.age_ticks.saturating_add(1);
        if effect.age_ticks >= effect.duration_ticks {
            commands.entity(entity).despawn();
        }
    }
}

/// Register lifecycle systems in a game's explicitly ordered simulation schedule.
pub struct PresentationPlugin<S: ScheduleLabel + Clone>(pub S);
impl<S: ScheduleLabel + Clone> Plugin for PresentationPlugin<S> {
    fn build(&self, app: &mut App) {
        app.add_systems(self.0.clone(), age_presentations);
    }
}
