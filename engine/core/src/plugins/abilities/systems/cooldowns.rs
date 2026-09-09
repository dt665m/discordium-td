use super::super::Loadout;
use crate::SimulationStep;
use bevy::prelude::*;

pub fn tick_loadouts<K: Send + Sync + 'static, M: Send + Sync + 'static>(
    step: Res<SimulationStep>,
    mut loadouts: Query<&mut Loadout<K, M>>,
) {
    if !step.0.is_finite() || step.0 < 0.0 {
        return;
    }
    for mut loadout in &mut loadouts {
        if loadout
            .0
            .iter()
            .any(|slot| slot.cooldown != (slot.cooldown - step.0).max(0.0))
        {
            loadout.tick(step.0);
        }
    }
}
