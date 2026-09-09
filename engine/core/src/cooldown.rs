use bevy::prelude::*;
/// Explicit fixed duration for replay; this is not another wall-clock accumulator.
#[derive(Resource, Clone, Copy)]
pub struct SimulationStep(pub f32);
/// Countdown arithmetic for existing serialized gameplay fields. Their remaining
/// time can be reduced/scaled by combat rules; preserve their f32 tick boundaries.
/// Use Bevy's Timer for ordinary elapsed-time timers, not a parallel timer API.
pub fn advance_cooldown(remaining: &mut f32, dt: f32) {
    *remaining = (*remaining - dt).max(0.0);
}
