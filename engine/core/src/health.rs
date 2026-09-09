use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
use serde::{Deserialize, Serialize};
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Health {
    pub hp: f32,
    pub max_hp: f32,
}
impl Health {
    /// Creates finite health capacity. Zero capacity is valid and starts depleted.
    ///
    /// # Panics
    /// Panics if capacity is negative or non-finite. Callers that change the public
    /// fields directly must preserve finite values and nonnegative capacity.
    pub fn new(max_hp: f32) -> Self {
        assert!(
            max_hp.is_finite() && max_hp >= 0.0,
            "health capacity must be finite and nonnegative"
        );
        Self { hp: max_hp, max_hp }
    }
    /// Applies nonnegative damage, capped at remaining HP. NaN and negative
    /// amounts do nothing; positive infinity depletes the actor.
    pub fn damage(&mut self, amount: f32) -> f32 {
        let dealt = amount.max(0.0).min(self.hp.max(0.0));
        self.hp -= dealt;
        dealt
    }
    pub fn heal(&mut self, amount: f32) {
        if self.hp > 0.0 {
            self.hp = (self.hp + amount.max(0.0)).min(self.max_hp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_capacity_and_saturating_damage_remain_finite_without_implicit_revival() {
        let mut zero = Health::new(0.0);
        assert_eq!(zero.damage(f32::INFINITY), 0.0);
        zero.heal(10.0);
        assert_eq!(zero.hp, 0.0);
        let mut health = Health::new(20.0);
        assert_eq!(health.damage(f32::NAN), 0.0);
        assert_eq!(health.damage(-10.0), 0.0);
        assert_eq!(health.damage(5.0), 5.0);
        health.heal(f32::INFINITY);
        assert_eq!(health.hp, 20.0);
        assert_eq!(health.damage(f32::INFINITY), 20.0);
        health.heal(20.0);
        assert_eq!(health.hp, 0.0);
    }

    #[test]
    fn invalid_capacity_is_rejected_before_it_can_produce_nan_health() {
        for capacity in [-1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(std::panic::catch_unwind(|| Health::new(capacity)).is_err());
        }
    }
}

/// Games decide how death affects their encounters; this reusable marker records it.
#[derive(Component, Debug)]
pub struct Depleted;
pub fn update_health(mut commands: Commands, actors: Query<(Entity, &Health, Has<Depleted>)>) {
    for (entity, health, depleted) in &actors {
        if health.hp <= 0.0 && !depleted {
            commands.entity(entity).insert(Depleted);
        } else if health.hp > 0.0 && depleted {
            commands.entity(entity).remove::<Depleted>();
        }
    }
}

pub struct HealthPlugin<S: ScheduleLabel + Clone>(pub S);
impl<S: ScheduleLabel + Clone> Plugin for HealthPlugin<S> {
    fn build(&self, app: &mut App) {
        app.add_systems(self.0.clone(), update_health);
    }
}
