//! Dreamwake's deterministic, server-authoritative cooperative roguelite world.
//! Clients submit authenticated player inputs and render per-player snapshots.
mod catalog;
#[cfg(test)]
mod coop_tests;
mod encounters;
mod snapshot_access;
mod state;
mod systems;
#[cfg(test)]
mod tests;
mod types;
use bevy::{
    ecs::{query::QueryState, schedule::ScheduleLabel},
    prelude::*,
};
use engine_core::Health;
use state::*;
use std::sync::Mutex;
pub use types::*;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct DreamTick;
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct DreamAmbient;
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct PresentationTick;
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct HealthTick;
fn update_actor_health(world: &mut World) {
    world.run_schedule(HealthTick);
}
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct CooldownTick;
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct MovementTick;
fn move_projectiles(world: &mut World) {
    world.run_schedule(MovementTick);
}
type SnapshotData = (
    Option<&'static Hero>,
    Option<&'static Enemy>,
    Option<&'static Projectile>,
    Option<&'static Wisp>,
    Option<&'static Effect>,
    Option<&'static Number>,
    Option<&'static DelayedCast>,
    Option<&'static Health>,
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
        app.init_schedule(DreamTick)
            .init_schedule(DreamAmbient)
            .init_schedule(PresentationTick)
            .init_schedule(HealthTick)
            .init_schedule(CooldownTick)
            .init_schedule(MovementTick)
            .add_plugins((
                engine_core::PresentationPlugin(PresentationTick),
                engine_core::HealthPlugin(HealthTick),
            ))
            .add_plugins((
                engine_core::CooldownPlugin::<Hero, _>::new(CooldownTick),
                engine_core::CooldownPlugin::<Enemy, _>::new(CooldownTick),
                engine_core::MovementPlugin::<Projectile, _>::new(MovementTick),
            ))
            .insert_resource(engine_core::SimulationStep(DT))
            .add_systems(DreamAmbient, systems::age_transients)
            .insert_resource(Input::default())
            .insert_resource(Run {
                tick: 0,
                seed,
                rng: seed.max(1),
                phase: RunPhase::Intro,
                room: 0,
                encounter_name: "The dream is waiting".into(),
                rewards: vec![],
                kills: 0,
                elapsed: 0.0,
                lucid,
                paused: false,
                cleared: 0,
                next_id: 1 << 63,
                message: "Vesper · Moonbound".into(),
                party_size: 1,
                reinforcements: 0,
            })
            .add_systems(
                DreamTick,
                (
                    systems::begin_tick,
                    systems::age_transients,
                    systems::player_actions,
                    systems::delayed_casts,
                    systems::wisp_actions,
                    systems::enemy_actions,
                    move_projectiles,
                    systems::projectile_actions,
                    update_actor_health,
                    systems::resolve_deaths,
                    systems::finish_encounter,
                )
                    .chain(),
            );
        app.world_mut()
            .spawn((DreamOwned, Hero::default(), Health::new(220.0)));
        let mut world = std::mem::take(app.world_mut());
        let snapshot_query = Mutex::new(world.query_filtered());
        Self {
            world,
            snapshot_query,
        }
    }
    pub fn step(&mut self, input: DreamInput) {
        self.step_multiplayer(&[(1, input)]);
    }
    /// Input owners come from authenticated server connections. Missing inputs are
    /// neutral; a duplicate owner within a tick is accepted only once.
    pub fn step_multiplayer(&mut self, inputs: &[(u64, DreamInput)]) {
        let run = self.world.resource::<Run>();
        if run.paused || run.phase == RunPhase::Intro || run.party_size == 0 {
            return;
        }
        let phase = run.phase;
        self.world.run_schedule(PresentationTick);
        if phase != RunPhase::Combat {
            self.world.run_schedule(DreamAmbient);
            return;
        }
        let mut ordered = inputs.to_vec();
        ordered.sort_by_key(|(id, _)| *id);
        ordered.dedup_by_key(|(id, _)| *id);
        self.world.resource_mut::<Input>().0 = ordered;
        self.world.run_schedule(CooldownTick);
        self.world.run_schedule(DreamTick);
        self.world.run_schedule(HealthTick);
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
        for (hero, enemy, projectile, wisp, effect, number, delayed, health) in self
            .snapshot_query
            .lock()
            .expect("Dream snapshot query poisoned")
            .iter(&self.world)
        {
            if let Some(v) = hero {
                saved.heroes.push(SavedActor {
                    actor: v.clone(),
                    health: *health.expect("actor health"),
                });
            }
            if let Some(v) = enemy {
                saved.enemies.push(SavedActor {
                    actor: v.clone(),
                    health: *health.expect("actor health"),
                });
            }
            if let Some(v) = projectile {
                saved.projectiles.push(v.clone());
            }
            if let Some(v) = wisp {
                saved.wisps.push(v.clone());
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
        saved.projectiles.sort_by_key(|v| v.view.id);
        saved.wisps.sort_by_key(|v| v.view.id);
        saved
            .effects
            .sort_by_key(|v| (v.id.owner, v.id.action_seq, v.id.slot));
        saved.numbers.sort_by_key(|v| v.0.id);
        saved.delayed.sort_by_key(|v| v.id);
        let fallback = SavedActor {
            health: Health::new(220.0),
            actor: Hero {
                view: HeroBody {
                    id: player_id,
                    ..Hero::default().view
                },
                ..Hero::default()
            },
        };
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
            hero: local.view.snapshot(&local.health),
            heroes: saved
                .heroes
                .iter()
                .map(|h| h.view.snapshot(&h.health))
                .collect(),
            ready: local.ready,
            awaiting_party: local.ready
                && matches!(
                    run.phase,
                    RunPhase::Intro | RunPhase::Reward | RunPhase::Rest | RunPhase::Transition
                ),
            enemies: saved
                .enemies
                .iter()
                .map(|v| v.view.snapshot(&v.health))
                .collect(),
            projectiles: saved.projectiles.iter().map(|v| v.view.clone()).collect(),
            wisps: saved.wisps.iter().map(|v| v.view.clone()).collect(),
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
            self.world.spawn((DreamOwned, v.actor.clone(), v.health));
        }
        for v in &state.enemies {
            self.world.spawn((DreamOwned, v.actor.clone(), v.health));
        }
        for v in &state.projectiles {
            self.world.spawn((DreamOwned, v.clone()));
        }
        for v in &state.wisps {
            self.world.spawn((DreamOwned, v.clone()));
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
        let mut hero = SavedActor {
            actor: Hero::default(),
            health: Health::new(220.0),
        };
        hero.view.id = id;
        if let Some(first) = query.iter(&self.world).min_by_key(|h| h.view.id) {
            hero.view = first.view.clone();
            hero.health = *first.health;
            hero.view.id = id;
            hero.health.hp = hero.health.max_hp;
            hero.view.shield = 0.0;
            hero.view.position[0] += 1.5;
            hero.view.position[0] = hero.view.position[0].clamp(-16.0, 16.0);
            hero.view.velocity = [0.0; 2];
            hero.view.dashing = false;
            hero.view.invulnerable = true;
            hero.view.attack_cooldown = 0.0;
            hero.view.dash_cooldown = 0.0;
            for memory in &mut hero.view.memories {
                memory.cooldown = 0.0;
            }
        }
        let run = self.world.resource::<Run>();
        if matches!(run.phase, RunPhase::Reward | RunPhase::Rest) {
            hero.rewards = run.rewards.clone();
        }
        hero.dodge_left = 1.5;
        self.world.spawn((DreamOwned, hero.actor, hero.health));
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
                Option<&Projectile>,
                Option<&Wisp>,
                Option<&DelayedCast>,
            ), With<DreamOwned>>()
            .iter(&self.world)
            .filter(|(_, p, w, c)| {
                p.is_some_and(|v| v.view.owner == id && v.view.friendly)
                    || w.is_some_and(|v| v.view.owner == id)
                    || c.is_some_and(|v| v.owner == id)
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
        hero.view.memories.swap(a, b);
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
