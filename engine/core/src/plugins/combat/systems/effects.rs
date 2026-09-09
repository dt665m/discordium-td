use super::super::CombatState;
use super::super::effects::nonnegative;
use crate::SimulationStep;
use bevy::prelude::*;

pub fn tick_combat(step: Res<SimulationStep>, mut actors: Query<&mut CombatState>) {
    let dt = nonnegative(step.0);
    for mut combat in &mut actors {
        let shield_remaining = (combat.shield_remaining - dt).max(0.0);
        let invulnerability_remaining = (combat.invulnerability_remaining - dt).max(0.0);
        // Inspect without mutable dereferencing: idle components should not
        // invalidate consumers of Changed<CombatState>.
        if combat.shield_remaining != shield_remaining
            || combat.invulnerability_remaining != invulnerability_remaining
            || (shield_remaining == 0.0 && combat.shield != 0.0)
            || combat.statuses.iter().any(|status| {
                status.remaining <= 0.0 || status.remaining != (status.remaining - dt).max(0.0)
            })
        {
            combat.tick(step.0);
        }
    }
}
