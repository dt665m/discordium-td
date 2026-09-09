use crate::SimulationStep;
use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AbilitySlot<K, M> {
    pub kind: K,
    pub level: u8,
    pub modifier: Option<M>,
    pub cooldown: f32,
    pub max_cooldown: f32,
}
impl<K, M> AbilitySlot<K, M> {
    pub fn new(kind: K, max_cooldown: f32) -> Self {
        Self {
            kind,
            level: 1,
            modifier: None,
            cooldown: 0.0,
            max_cooldown,
        }
    }
    pub fn ready(&self) -> bool {
        self.cooldown <= 0.0
    }
    pub fn activate(&mut self) -> bool {
        if !self.ready() {
            return false;
        }
        self.cooldown = self.max_cooldown;
        true
    }
    pub fn refresh(&mut self) {
        self.cooldown = 0.0;
    }
    pub fn reduce_cooldown(&mut self, amount: f32) {
        self.cooldown = (self.cooldown - amount.max(0.0)).max(0.0);
    }
    pub fn scale_remaining(&mut self, factor: f32) {
        self.cooldown *= factor.max(0.0);
    }
    pub fn upgrade(&mut self, cap: u8) -> bool {
        if self.level >= cap {
            return false;
        }
        self.level += 1;
        true
    }
    /// Replacement policy explicitly chooses whether the socket survives.
    pub fn replace(&mut self, kind: K, level: u8, preserve_modifier: bool) {
        self.kind = kind;
        self.level = level;
        if !preserve_modifier {
            self.modifier = None;
        }
        self.refresh();
    }
}

/// Slot count and ability/modifier identifiers are supplied by each game.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Loadout<K: Send + Sync + 'static, M: Send + Sync + 'static>(pub Vec<AbilitySlot<K, M>>);
impl<K: Send + Sync + 'static, M: Send + Sync + 'static> Loadout<K, M> {
    pub fn tick(&mut self, dt: f32) {
        if !dt.is_finite() || dt < 0.0 {
            return;
        }
        for slot in &mut self.0 {
            crate::advance_cooldown(&mut slot.cooldown, dt);
        }
    }
    pub fn swap(&mut self, a: usize, b: usize) -> bool {
        if a >= self.0.len() || b >= self.0.len() {
            return false;
        }
        self.0.swap(a, b);
        true
    }
}
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
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct LoadoutStep;
pub struct LoadoutPlugin<
    K: Send + Sync + 'static,
    M: Send + Sync + 'static,
    S: ScheduleLabel + Clone,
> {
    schedule: S,
    marker: std::marker::PhantomData<(K, M)>,
}
impl<K: Send + Sync + 'static, M: Send + Sync + 'static, S: ScheduleLabel + Clone>
    LoadoutPlugin<K, M, S>
{
    pub fn new(schedule: S) -> Self {
        Self {
            schedule,
            marker: std::marker::PhantomData,
        }
    }
}
impl<K: Send + Sync + 'static, M: Send + Sync + 'static, S: ScheduleLabel + Clone> Plugin
    for LoadoutPlugin<K, M, S>
{
    fn build(&self, app: &mut App) {
        app.add_systems(
            self.schedule.clone(),
            tick_loadouts::<K, M>.in_set(LoadoutStep),
        );
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slots_activate_independently_and_preserve_selected_socket_policy() {
        let mut slot = AbilitySlot::new(1_u8, 2.0);
        slot.modifier = Some(9_u8);
        assert!(slot.activate());
        assert!(!slot.activate());
        let mut loadout = Loadout(vec![slot]);
        loadout.tick(1.0);
        assert_eq!(loadout.0[0].cooldown, 1.0);
        loadout.0[0].replace(2, 1, true);
        assert_eq!(loadout.0[0].modifier, Some(9));
        assert!(loadout.0[0].upgrade(2));
        assert!(!loadout.0[0].upgrade(2));
        assert!(!loadout.swap(0, 1));
        assert!(loadout.0[0].ready());
    }
}
