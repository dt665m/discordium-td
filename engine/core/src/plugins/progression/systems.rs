use super::{ExperienceCurve, ExperienceResult, PendingExperience, Progression};
use bevy::prelude::*;

pub(super) fn grant_experience(
    mut commands: Commands,
    curve: Res<ExperienceCurve>,
    mut actors: Query<(Entity, &mut Progression, &PendingExperience)>,
) {
    for (entity, mut progression, grant) in &mut actors {
        let mut next = *progression;
        let result = next.gain(grant.0, curve.0);
        progression.set_if_neq(next);
        commands
            .entity(entity)
            .remove::<PendingExperience>()
            .insert(ExperienceResult(result));
    }
}
