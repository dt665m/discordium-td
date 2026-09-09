//! Simulation-owned defenses and timed effects, with game-supplied balance and policy.
use crate::{Health, SimulationStep};
use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
use serde::{Deserialize, Serialize};

/// A stable identifier allocated by the consuming game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusId(pub u16);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimedStatus {
    pub id: StatusId,
    pub magnitude: f32,
    pub remaining: f32,
}

#[derive(Debug, Clone, Copy)]
pub enum DurationPolicy {
    /// Replace the previous duration, even when the new duration is shorter.
    Reset,
    KeepLonger,
    Extend,
}

#[derive(Debug, Clone, Copy)]
pub enum ShieldPolicy {
    Replace,
    Add,
    AddCapped(f32),
}

#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CombatState {
    pub shield: f32,
    pub shield_remaining: f32,
    pub invulnerability_remaining: f32,
    pub statuses: Vec<TimedStatus>,
}

fn nonnegative(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn duration(previous: f32, supplied: f32, policy: DurationPolicy) -> f32 {
    let previous = nonnegative(previous);
    let supplied = nonnegative(supplied);
    match policy {
        DurationPolicy::Reset => supplied,
        DurationPolicy::KeepLonger => previous.max(supplied),
        DurationPolicy::Extend => (previous + supplied).min(f32::MAX),
    }
}

impl CombatState {
    pub fn grant_shield(
        &mut self,
        amount: f32,
        seconds: f32,
        amount_policy: ShieldPolicy,
        duration_policy: DurationPolicy,
    ) {
        let amount = nonnegative(amount);
        self.shield = match amount_policy {
            ShieldPolicy::Replace => amount,
            ShieldPolicy::Add => (nonnegative(self.shield) + amount).min(f32::MAX),
            ShieldPolicy::AddCapped(cap) => {
                (nonnegative(self.shield) + amount).min(nonnegative(cap))
            }
        };
        self.shield_remaining = duration(self.shield_remaining, seconds, duration_policy);
        if self.shield_remaining == 0.0 {
            self.shield = 0.0;
        }
    }

    /// Returns the absorbed portion of the supplied damage.
    pub fn absorb_damage(&mut self, amount: f32) -> f32 {
        if self.shield_remaining <= 0.0 {
            self.shield = 0.0;
        }
        let absorbed = nonnegative(self.shield).min(amount.max(0.0));
        self.shield -= absorbed;
        absorbed
    }

    pub fn grant_invulnerability(&mut self, seconds: f32, policy: DurationPolicy) {
        self.invulnerability_remaining = duration(self.invulnerability_remaining, seconds, policy);
    }

    pub fn is_invulnerable(&self) -> bool {
        self.invulnerability_remaining > 0.0
    }

    /// Refreshes duration and replaces magnitude for an existing identity.
    /// Zero duration removes the status. Non-finite magnitudes become zero.
    pub fn apply_status(
        &mut self,
        id: StatusId,
        magnitude: f32,
        seconds: f32,
        policy: DurationPolicy,
    ) {
        let magnitude = if magnitude.is_finite() {
            magnitude
        } else {
            0.0
        };
        if let Some(status) = self.statuses.iter_mut().find(|status| status.id == id) {
            status.magnitude = magnitude;
            status.remaining = duration(status.remaining, seconds, policy);
        } else {
            self.statuses.push(TimedStatus {
                id,
                magnitude,
                remaining: duration(0.0, seconds, policy),
            });
        }
        self.statuses.retain(|status| status.remaining > 0.0);
    }

    pub fn status(&self, id: StatusId) -> Option<&TimedStatus> {
        self.statuses
            .iter()
            .find(|status| status.id == id && status.remaining > 0.0)
    }

    pub fn tick(&mut self, dt: f32) {
        let dt = nonnegative(dt);
        self.shield_remaining = (self.shield_remaining - dt).max(0.0);
        if self.shield_remaining == 0.0 {
            self.shield = 0.0;
        }
        self.invulnerability_remaining = (self.invulnerability_remaining - dt).max(0.0);
        for status in &mut self.statuses {
            status.remaining = (status.remaining - dt).max(0.0);
        }
        self.statuses.retain(|status| status.remaining > 0.0);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DamageOutcome {
    /// Nonnegative damage after the supplied multiplier, before defenses.
    pub incoming: f32,
    pub absorbed: f32,
    pub hp_damage: f32,
    /// True for an invulnerable or already depleted target.
    pub blocked: bool,
}

/// Applies immunity, shield absorption, and HP damage in that order.
/// Games compute critical hits and damage modifiers before calling, and decide
/// subsequent hit effects, immunity grants, lifesteal, and knockback themselves.
/// Invalid or negative inputs deal zero damage; overflow saturates to finite damage.
pub fn resolve_damage(
    health: &mut Health,
    combat: &mut CombatState,
    amount: f32,
    multiplier: f32,
) -> DamageOutcome {
    let incoming = (nonnegative(amount) * nonnegative(multiplier)).min(f32::MAX);
    if health.hp <= 0.0 || combat.is_invulnerable() {
        return DamageOutcome {
            incoming,
            blocked: true,
            ..Default::default()
        };
    }
    let absorbed = combat.absorb_damage(incoming);
    let hp_damage = health.damage(incoming - absorbed);
    DamageOutcome {
        incoming,
        absorbed,
        hp_damage,
        blocked: false,
    }
}

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

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct CombatStep;
pub struct CombatPlugin<S: ScheduleLabel + Clone>(pub S);
impl<S: ScheduleLabel + Clone> Plugin for CombatPlugin<S> {
    fn build(&self, app: &mut App) {
        app.add_systems(self.0.clone(), tick_combat.in_set(CombatStep));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damage_absorbs_shield_caps_hp_and_never_revives() {
        let mut health = Health::new(20.0);
        let mut combat = CombatState::default();
        combat.grant_shield(8.0, 2.0, ShieldPolicy::Add, DurationPolicy::Reset);
        assert_eq!(
            resolve_damage(&mut health, &mut combat, 10.0, 2.0),
            DamageOutcome {
                incoming: 20.0,
                absorbed: 8.0,
                hp_damage: 12.0,
                blocked: false,
            }
        );
        assert_eq!(
            resolve_damage(&mut health, &mut combat, 100.0, 1.0).hp_damage,
            8.0
        );
        health.heal(100.0);
        assert!(resolve_damage(&mut health, &mut combat, 10.0, 1.0).blocked);
        assert_eq!(health.hp, 0.0);
        assert!(resolve_damage(&mut Health::new(0.0), &mut combat, 10.0, 1.0).blocked);
    }

    #[test]
    fn grant_policies_allow_capped_and_uncapped_shields_and_shorter_immunity() {
        let mut combat = CombatState::default();
        combat.grant_shield(20.0, 4.0, ShieldPolicy::Add, DurationPolicy::Reset);
        combat.grant_shield(
            20.0,
            1.0,
            ShieldPolicy::AddCapped(30.0),
            DurationPolicy::KeepLonger,
        );
        assert_eq!((combat.shield, combat.shield_remaining), (30.0, 4.0));
        combat.grant_shield(20.0, 1.0, ShieldPolicy::Add, DurationPolicy::Reset);
        assert_eq!((combat.shield, combat.shield_remaining), (50.0, 1.0));
        combat.grant_invulnerability(4.0, DurationPolicy::Reset);
        combat.grant_invulnerability(0.5, DurationPolicy::Reset);
        let mut health = Health::new(20.0);
        assert!(resolve_damage(&mut health, &mut combat, 10.0, 1.0).blocked);
        assert_eq!(combat.shield, 50.0);
        combat.tick(0.5);
        assert!(!combat.is_invulnerable());
        combat.tick(0.5);
        assert_eq!(combat.shield, 0.0);
    }

    #[test]
    fn plugin_ticks_components_and_refreshes_status_without_duplicates() {
        let mut app = App::new();
        app.insert_resource(SimulationStep(0.5))
            .add_plugins(CombatPlugin(Update));
        let mut combat = CombatState::default();
        combat.grant_shield(3.0, 0.5, ShieldPolicy::Replace, DurationPolicy::Reset);
        combat.apply_status(StatusId(7), 0.5, 2.0, DurationPolicy::Reset);
        combat.apply_status(StatusId(7), 0.25, 0.5, DurationPolicy::Reset);
        assert_eq!(combat.statuses.len(), 1);
        assert_eq!(combat.status(StatusId(7)).unwrap().magnitude, 0.25);
        let actor = app.world_mut().spawn(combat).id();
        app.update();
        let combat = app.world().get::<CombatState>(actor).unwrap();
        assert_eq!(combat.shield, 0.0);
        assert!(combat.status(StatusId(7)).is_none());
    }

    #[test]
    fn serialized_restore_replays_identically() {
        let mut original = CombatState::default();
        original.grant_shield(8.0, 1.0, ShieldPolicy::Add, DurationPolicy::Reset);
        original.grant_invulnerability(0.25, DurationPolicy::Reset);
        original.apply_status(StatusId(3), 0.4, 0.5, DurationPolicy::Extend);
        let mut restored: CombatState =
            serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();
        let mut health = Health::new(20.0);
        let mut restored_health = health;
        for _ in 0..5 {
            original.tick(0.25);
            restored.tick(0.25);
            assert_eq!(
                resolve_damage(&mut health, &mut original, 3.0, 1.0),
                resolve_damage(&mut restored_health, &mut restored, 3.0, 1.0)
            );
            assert_eq!(original, restored);
            assert_eq!(health, restored_health);
        }
    }
}
