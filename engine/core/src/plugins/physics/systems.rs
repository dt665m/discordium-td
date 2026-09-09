use super::MotorState;
use crate::SimulationStep;
use bevy::prelude::*;

pub fn tick_motors(step: Res<SimulationStep>, mut motors: Query<&mut MotorState>) {
    for mut motor in &mut motors {
        let mut next = *motor;
        next.tick(step.0);
        motor.set_if_neq(next);
    }
}
