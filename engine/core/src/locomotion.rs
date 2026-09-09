use crate::{SimulationStep, spatial};
use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CircleBounds {
    pub center: [f32; 2],
    pub radius: f32,
}
impl CircleBounds {
    pub fn clamp(self, position: [f32; 2]) -> [f32; 2] {
        let center = Vec2::from_array(self.center);
        (center
            + Circle::new(self.radius.max(0.0)).closest_point(Vec2::from_array(position) - center))
        .to_array()
    }
}
/// Canonical planar movement. Games choose acceptance, speed and immunity rules.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct MotorState {
    pub position: [f32; 2],
    pub facing: [f32; 2],
    pub velocity: [f32; 2],
    pub movement_lock: f32,
    pub dash_direction: [f32; 2],
    pub dash_remaining: f32,
    pub dash_cooldown: f32,
    pub dash_speed: f32,
}
impl MotorState {
    /// Apply instantaneous displacement without changing velocity or facing.
    /// Games supply response: zero is immunity, one is full displacement, and
    /// intermediate values resist a portion of it. Values above one amplify it.
    pub fn displace(
        &mut self,
        displacement: [f32; 2],
        response: f32,
        bounds: Option<CircleBounds>,
    ) {
        if !response.is_finite() || response <= 0.0 {
            return;
        }
        self.position = spatial::add(self.position, spatial::scale(displacement, response));
        if let Some(bounds) = bounds {
            self.position = bounds.clamp(self.position);
        }
    }
    pub fn tick(&mut self, dt: f32) {
        if !dt.is_finite() || dt < 0.0 {
            return;
        }
        for timer in [
            &mut self.movement_lock,
            &mut self.dash_remaining,
            &mut self.dash_cooldown,
        ] {
            crate::advance_cooldown(timer, dt);
        }
    }
    pub fn start_dash(&mut self, direction: [f32; 2], speed: f32, duration: f32, cooldown: f32) {
        self.dash_direction = spatial::normalize(direction);
        self.dash_speed = speed;
        self.dash_remaining = duration.max(0.0);
        self.dash_cooldown = cooldown.max(0.0);
    }
    /// Integrate once after timers/input acceptance; this does not tick timers.
    pub fn advance(
        &mut self,
        movement: [f32; 2],
        aim: [f32; 2],
        speed: f32,
        bounds: Option<CircleBounds>,
        dt: f32,
    ) {
        let aim = spatial::normalize(aim);
        if spatial::length(aim) > 0.0 {
            self.facing = aim;
        }
        self.velocity = if self.dash_remaining > 0.0 {
            spatial::scale(self.dash_direction, self.dash_speed)
        } else if self.movement_lock > 0.0 {
            [0.0; 2]
        } else {
            spatial::scale(spatial::normalize(movement), speed)
        };
        self.position = spatial::integrate(self.position, self.velocity, dt);
        if let Some(bounds) = bounds {
            self.position = bounds.clamp(self.position);
        }
    }
}
pub fn tick_motors(step: Res<SimulationStep>, mut motors: Query<&mut MotorState>) {
    for mut motor in &mut motors {
        let mut next = *motor;
        next.tick(step.0);
        motor.set_if_neq(next);
    }
}
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct MotorStep;
pub struct MotorPlugin<S: ScheduleLabel + Clone>(pub S);
impl<S: ScheduleLabel + Clone> Plugin for MotorPlugin<S> {
    fn build(&self, app: &mut App) {
        app.add_systems(self.0.clone(), tick_motors.in_set(MotorStep));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bevy_circle_clamps_translated_bounds_and_zero_radius() {
        let bounds = CircleBounds {
            center: [4.0, -2.0],
            radius: 5.0,
        };
        assert_eq!(bounds.clamp([5.0, -1.0]), [5.0, -1.0]);
        assert_eq!(bounds.clamp([7.0, 2.0]), [7.0, 2.0]);
        let clamped = bounds.clamp([10.0, 6.0]);
        assert!(spatial::distance(clamped, [7.0, 2.0]) < 0.00001);
        assert_eq!(
            CircleBounds {
                radius: 0.0,
                ..bounds
            }
            .clamp([10.0, 6.0]),
            bounds.center
        );
    }
    #[test]
    fn displacement_response_supports_immunity_resistance_and_bounds() {
        let mut motor = MotorState {
            position: [1.0, 0.0],
            velocity: [0.0, 3.0],
            ..Default::default()
        };
        motor.displace([4.0, 0.0], 0.0, None);
        assert_eq!(motor.position, [1.0, 0.0]);
        motor.displace([4.0, 0.0], 0.25, None);
        assert_eq!(motor.position, [2.0, 0.0]);
        motor.displace([4.0, 0.0], 1.0, None);
        assert_eq!(motor.position, [6.0, 0.0]);
        motor.displace(
            [4.0, 0.0],
            1.0,
            Some(CircleBounds {
                center: [1.0, 0.0],
                radius: 6.0,
            }),
        );
        assert_eq!(motor.position, [7.0, 0.0]);
        assert_eq!(motor.velocity, [0.0, 3.0]);
    }
    #[test]
    fn movement_normalizes_and_dash_overrides_lock_with_bounds() {
        let mut motor = MotorState::default();
        motor.advance([1.0; 2], [0.0, -1.0], 2.0, None, 1.0);
        assert!((spatial::length(motor.position) - 2.0).abs() < 0.0001);
        motor.position = [0.0; 2];
        motor.movement_lock = 1.0;
        motor.start_dash([1.0, 0.0], 10.0, 0.5, 2.0);
        motor.advance(
            [0.0; 2],
            [0.0; 2],
            2.0,
            Some(CircleBounds {
                center: [0.0; 2],
                radius: 3.0,
            }),
            0.5,
        );
        assert_eq!(motor.position, [3.0, 0.0]);
        motor.tick(0.5);
        motor.advance([1.0; 2], [0.0; 2], 2.0, None, 0.1);
        assert_eq!(motor.velocity, [0.0; 2]);
        assert_eq!(motor.dash_cooldown, 1.5);
    }
}
