use super::*;
pub(crate) fn advance_covers(
    run: Res<Run>,
    mut covers: Query<(&mut Cover, &mut CombatIdentity)>,
    mut batch: ResMut<CombatBatch>,
) {
    for (mut cover, mut identity) in &mut covers {
        if !cover.present {
            continue;
        }
        if run.tick >= cover.next_transition_tick {
            let Some(next) = run.tick.checked_add(COVER_PERIOD_TICKS) else {
                batch.error = Some("cover tick exhausted");
                continue;
            };
            cover.open = !cover.open;
            cover.next_transition_tick = next;
        }
        if identity.advance(run.tick).is_err() {
            batch.error = Some("cover pose exhausted");
        }
    }
}
