use super::super::{Meter, Regeneration};
use crate::SimulationStep;
use bevy::prelude::*;

pub fn regenerate_meters<K: Send + Sync + 'static>(
    step: Res<SimulationStep>,
    mut actors: Query<(&mut Meter<K>, &Regeneration<K>)>,
) {
    if !step.0.is_finite() || step.0 <= 0.0 {
        return;
    }
    for (mut meter, regeneration) in &mut actors {
        if !regeneration.per_second.is_finite() || regeneration.per_second <= 0.0 {
            continue;
        }
        let mut next = *meter;
        // A finite rate and delta can overflow; regeneration still stops at capacity.
        let amount = (regeneration.per_second * step.0).min(next.capacity() - next.current());
        next.restore(amount);
        meter.set_if_neq(next);
    }
}
