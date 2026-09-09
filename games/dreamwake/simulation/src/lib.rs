//! Dreamwake's deterministic, server-authoritative cooperative roguelite world.
//! Clients submit authenticated player inputs and render per-player snapshots.
mod catalog;
#[cfg(test)]
mod coop_tests;
mod encounters;
mod plugin;
pub use plugin::{DreamInputs, DreamStep, DreamSystems, DreamwakePlugin};
mod snapshot_access;
mod state;
mod systems;
#[cfg(test)]
mod tests;
mod types;
use bevy::{ecs::query::QueryState, prelude::*};
use engine_core::Health;
use state::*;
use std::sync::Mutex;
pub use types::*;

type SnapshotData = (
    Option<&'static Hero>,
    Option<&'static Enemy>,
    Option<&'static Projectile>,
    Option<&'static Wisp>,
    Option<&'static Effect>,
    Option<&'static Number>,
    Option<&'static DelayedCast>,
    Option<&'static Health>,
    Option<&'static engine_core::CombatState>,
    Option<&'static engine_core::MotorState>,
    Option<&'static engine_core::ActionState>,
    Option<&'static MemoryLoadout>,
    Option<&'static engine_core::Progression>,
    Option<&'static engine_core::ProjectileState>,
    Option<&'static engine_core::SummonState>,
);

pub struct DreamSimulation {
    world: World,
    snapshot_query: Mutex<QueryState<SnapshotData, With<DreamOwned>>>,
}
impl DreamSimulation {
    /// Creates a world with player 1. Dedicated hosts may remove that initial player
    /// before admitting authenticated connections.
    pub fn new(seed: u64, lucid: bool) -> Self {
        let mut app = App::new();
        app.add_plugins(DreamwakePlugin::new(seed, lucid));
        Self::from_app(app)
    }
    /// Consume a precomposed headless app containing DreamwakePlugin.
    pub fn from_app(mut app: App) -> Self {
        assert!(
            app.world().contains_resource::<Run>(),
            "DreamwakePlugin is required"
        );
        let mut world = std::mem::take(app.world_mut());
        let snapshot_query = Mutex::new(world.query_filtered());
        Self {
            world,
            snapshot_query,
        }
    }
    pub fn world(&self) -> &World {
        &self.world
    }
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }
    pub fn step(&mut self, input: DreamInput) {
        self.step_multiplayer(&[(1, input)]);
    }
    /// Owners originate from authenticated connections; duplicate owners execute once.
    pub fn step_multiplayer(&mut self, inputs: &[(u64, DreamInput)]) {
        self.world.resource_mut::<DreamInputs>().0 = inputs.to_vec();
        self.world.run_schedule(DreamStep);
    }
    pub fn snapshot(&self) -> DreamSnapshot {
        self.snapshot_for(1)
    }
    pub fn snapshot_for(&self, player_id: u64) -> DreamSnapshot {
        let run = self.world.resource::<Run>().clone();
        let mut saved = SavedState {
            run: run.clone(),
            heroes: vec![],
            enemies: vec![],
            projectiles: vec![],
            wisps: vec![],
            effects: vec![],
            numbers: vec![],
            delayed: vec![],
        };
        for (
            hero,
            enemy,
            projectile,
            wisp,
            effect,
            number,
            delayed,
            health,
            combat,
            motor,
            action,
            loadout,
            progression,
            projectile_state,
            summon_state,
        ) in self
            .snapshot_query
            .lock()
            .expect("Dream snapshot query poisoned")
            .iter(&self.world)
        {
            if let Some(v) = hero {
                saved.heroes.push(SavedHero {
                    actor: v.clone(),
                    health: *health.expect("actor health"),
                    combat: combat.expect("actor combat").clone(),
                    motor: *motor.expect("actor motor"),
                    action: *action.expect("actor action"),
                    loadout: loadout.expect("hero loadout").clone(),
                    progression: *progression.expect("hero progression"),
                });
            }
            if let Some(v) = enemy {
                saved.enemies.push(SavedEnemy {
                    actor: v.clone(),
                    health: *health.expect("actor health"),
                    combat: combat.expect("actor combat").clone(),
                    motor: *motor.expect("actor motor"),
                    action: *action.expect("actor action"),
                });
            }
            if let Some(v) = projectile {
                saved.projectiles.push(SavedProjectile {
                    payload: v.clone(),
                    state: projectile_state.expect("projectile state").clone(),
                });
            }
            if let Some(v) = wisp {
                saved.wisps.push(SavedWisp {
                    payload: v.clone(),
                    state: summon_state.expect("summon state").clone(),
                });
            }
            if let Some(v) = effect {
                saved.effects.push(v.clone());
            }
            if let Some(v) = number {
                saved.numbers.push(v.clone());
            }
            if let Some(v) = delayed {
                saved.delayed.push(v.clone());
            }
        }
        saved.heroes.sort_by_key(|v| v.view.id);
        saved.enemies.sort_by_key(|v| v.view.id);
        saved.projectiles.sort_by_key(|v| v.state.id);
        saved.wisps.sort_by_key(|v| v.state.id);
        saved
            .effects
            .sort_by_key(|v| (v.id.owner, v.id.action_seq, v.id.slot));
        saved.numbers.sort_by_key(|v| v.0.id);
        saved.delayed.sort_by_key(|v| v.payload.id);
        let fallback = SavedHero::new(player_id);
        let local = saved
            .heroes
            .iter()
            .find(|h| h.view.id == player_id)
            .unwrap_or(&fallback);
        DreamSnapshot {
            tick: run.tick,
            seed: run.seed,
            phase: run.phase,
            room: run.room,
            realm: (run.room / 3).min(2),
            encounter_name: run.encounter_name,
            hero: local.snapshot(),
            heroes: saved.heroes.iter().map(SavedHero::snapshot).collect(),
            ready: local.ready,
            awaiting_party: local.ready
                && matches!(
                    run.phase,
                    RunPhase::Intro | RunPhase::Reward | RunPhase::Rest | RunPhase::Transition
                ),
            enemies: saved.enemies.iter().map(SavedEnemy::snapshot).collect(),
            projectiles: saved
                .projectiles
                .iter()
                .map(SavedProjectile::snapshot)
                .collect(),
            wisps: saved.wisps.iter().map(SavedWisp::snapshot).collect(),
            presentations: saved.effects.clone(),
            damage_numbers: saved.numbers.iter().map(|v| v.0.clone()).collect(),
            rewards: local.rewards.clone(),
            kills: run.kills,
            elapsed: run.elapsed,
            lucid: run.lucid,
            paused: run.paused,
            cleared: run.cleared,
            enemies_remaining: saved.enemies.len() + run.reinforcements as usize,
            message: run.message,
            state: saved,
        }
    }
    pub fn from_snapshot(snapshot: &DreamSnapshot) -> Self {
        let mut sim = Self::new(snapshot.seed, snapshot.lucid);
        sim.restore(snapshot);
        sim
    }
    /// Restores all players and latent state, not just the requesting player's view.
    pub fn restore(&mut self, snapshot: &DreamSnapshot) {
        let owned: Vec<_> = self
            .world
            .query_filtered::<Entity, With<DreamOwned>>()
            .iter(&self.world)
            .collect();
        for entity in owned {
            self.world.despawn(entity);
        }
        let state = &snapshot.state;
        self.world.insert_resource(state.run.clone());
        for v in &state.heroes {
            v.clone().spawn(&mut self.world);
        }
        for v in &state.enemies {
            v.clone().spawn(&mut self.world);
        }
        for v in &state.projectiles {
            self.world
                .spawn((DreamOwned, v.payload.clone(), v.state.clone()));
        }
        for v in &state.wisps {
            self.world
                .spawn((DreamOwned, v.payload.clone(), v.state.clone()));
        }
        for v in &state.effects {
            self.world.spawn((DreamOwned, v.clone()));
        }
        for v in &state.numbers {
            self.world.spawn((DreamOwned, v.clone()));
        }
        for v in &state.delayed {
            self.world.spawn((DreamOwned, v.clone()));
        }
    }
    pub fn add_player(&mut self, id: u64) -> bool {
        if id == 0 || id >= 1 << 63 {
            return false;
        }
        let mut query = self.world.query::<HeroActorReadOnly>();
        if query.iter(&self.world).any(|h| h.view.id == id) {
            return true;
        }
        let count = query.iter(&self.world).len();
        let mut hero = SavedHero::new(id);
        if let Some(first) = query.iter(&self.world).min_by_key(|h| h.view.id) {
            hero.actor.view = first.view.clone();
            hero.view.id = id;
            hero.health = *first.health;
            hero.health.hp = hero.health.max_hp;
            hero.motor.position = first.motor.position;
            hero.motor.facing = first.motor.facing;
            hero.motor.position[0] = (hero.motor.position[0] + 1.5).clamp(-16.0, 16.0);
            hero.loadout = first.loadout.clone();
            hero.progression = *first.progression;
            hero.progression.pending_levels = 0;
            for memory in &mut hero.loadout.0 {
                memory.refresh();
            }
        }
        let run = self.world.resource::<Run>();
        if matches!(run.phase, RunPhase::Reward | RunPhase::Rest) {
            hero.rewards = run.rewards.clone();
        }
        hero.combat
            .grant_invulnerability(1.5, engine_core::DurationPolicy::Reset);
        hero.spawn(&mut self.world);
        self.resize_party(count, count + 1);
        true
    }
    pub fn remove_player(&mut self, id: u64) -> bool {
        let found = self
            .world
            .query::<(Entity, HeroActorReadOnly)>()
            .iter(&self.world)
            .find(|(_, h)| h.view.id == id)
            .map(|(e, _)| e);
        let Some(entity) = found else {
            return false;
        };
        self.world.despawn(entity);
        let owned: Vec<_> = self
            .world
            .query_filtered::<(
                Entity,
                Option<&engine_core::ProjectileState>,
                Option<&engine_core::SummonState>,
                Option<&DelayedCast>,
            ), With<DreamOwned>>()
            .iter(&self.world)
            .filter(|(_, p, w, c)| {
                p.is_some_and(|v| v.owner == id && v.faction == 1)
                    || w.is_some_and(|v| v.owner == id)
                    || c.is_some_and(|v| v.payload.owner == id)
            })
            .map(|(e, ..)| e)
            .collect();
        for entity in owned {
            self.world.despawn(entity);
        }
        let before = self.world.resource::<Run>().party_size;
        self.resize_party(before, before.saturating_sub(1));
        encounters::reconcile_ready(&mut self.world);
        true
    }
    fn resize_party(&mut self, before: usize, after: usize) {
        self.world.resource_mut::<Run>().party_size = after;
        if after == 0 {
            return;
        }
        let factor = (1.0 + after.saturating_sub(1) as f32 * 0.65)
            / (1.0 + before.saturating_sub(1) as f32 * 0.65);
        for mut enemy in self.world.query::<EnemyActor>().iter_mut(&mut self.world) {
            enemy.health.hp *= factor;
            enemy.health.max_hp *= factor;
        }
    }
    pub fn restart(&mut self, seed: u64, lucid: bool) {
        *self = Self::new(seed, lucid);
    }
    pub fn restart_party(&mut self, seed: u64, lucid: bool) {
        let ids: Vec<_> = self
            .world
            .query::<HeroActorReadOnly>()
            .iter(&self.world)
            .map(|h| h.view.id)
            .collect();
        *self = Self::new(seed, lucid);
        self.remove_player(1);
        for id in ids {
            self.add_player(id);
        }
    }
    pub fn set_paused(&mut self, paused: bool) {
        self.world.resource_mut::<Run>().paused = paused;
    }
    pub fn choose_reward(&mut self, choice: usize, slot: usize) -> bool {
        self.choose_reward_for(1, choice, slot)
    }
    pub fn choose_reward_for(&mut self, id: u64, choice: usize, slot: usize) -> bool {
        encounters::choose_reward(&mut self.world, id, choice, slot)
    }
    pub fn swap_memories(&mut self, a: usize, b: usize) -> bool {
        self.swap_memories_for(1, a, b)
    }
    pub fn swap_memories_for(&mut self, id: u64, a: usize, b: usize) -> bool {
        let run = self.world.resource::<Run>();
        if a >= 4 || b >= 4 || (run.phase == RunPhase::Combat && !run.paused) {
            return false;
        }
        let mut query = self.world.query::<HeroActor>();
        let Some(mut hero) = query.iter_mut(&mut self.world).find(|h| h.view.id == id) else {
            return false;
        };
        hero.loadout.swap(a, b);
        true
    }
    pub fn continue_run(&mut self) {
        self.continue_run_for(1);
    }
    pub fn continue_run_for(&mut self, id: u64) -> bool {
        encounters::continue_run(&mut self.world, id)
    }
    pub fn buy_memory_upgrade(&mut self, slot: usize) -> bool {
        self.buy_memory_upgrade_for(1, slot)
    }
    pub fn buy_memory_upgrade_for(&mut self, id: u64, slot: usize) -> bool {
        encounters::buy_memory_upgrade(&mut self.world, id, slot)
    }
}
