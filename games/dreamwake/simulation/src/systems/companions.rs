use super::*;
use engine_core::CompanionState;

pub(crate) fn spawn_wisp(
    commands: &mut Commands,
    run: &mut Run,
    owner: u64,
    origin: [f32; 2],
    essence: Option<EssenceKind>,
    damage: f32,
    index: u32,
) {
    commands.spawn((
        DreamOwned,
        Wisp { damage, essence },
        CompanionState {
            id: run.id(),
            owner,
            faction: 1,
            position: origin,
            remaining: 9.0,
            orbit_angle: index as f32 * 3.14,
            orbit_radius: 1.7,
            orbit_speed: 1.8,
            fire_remaining: 0.15 + index as f32 * 0.2,
            fire_interval: if essence == Some(EssenceKind::Haste) {
                0.42
            } else {
                0.70
            },
            range: if essence == Some(EssenceKind::Vast) {
                18.0
            } else {
                12.0
            },
            expired: false,
            pending_shot: None,
        },
    ));
}

pub(crate) fn wisp_actions(
    mut commands: Commands,
    mut run: ResMut<Run>,
    mut wisps: Query<(Entity, &Wisp, &mut CompanionState)>,
) {
    let mut ordered: Vec<_> = wisps.iter().map(|(e, _, state)| (state.id, e)).collect();
    ordered.sort_unstable();
    for (_, entity) in ordered {
        let Ok((_, payload, mut state)) = wisps.get_mut(entity) else {
            continue;
        };
        if state.expired {
            commands.entity(entity).despawn();
            continue;
        }
        if let Some(direction) = state.pending_shot.take() {
            projectile(
                &mut commands,
                &mut run,
                state.owner,
                state.position,
                direction,
                payload.damage,
                true,
                payload.essence,
                20.0,
                0.24,
            );
        }
    }
}
