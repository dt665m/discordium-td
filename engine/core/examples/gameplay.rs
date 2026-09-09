//! A headless robot arena assembled only from reusable engine components.
//! Run with `cargo run -p engine_core --example gameplay`.
use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
use engine_core::*;
use serde::{Deserialize, Serialize};

#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
struct ArenaTick;
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
enum ArenaStep {
    Timers,
    Move,
    Index,
    Mechanics,
    Resolve,
}
#[derive(Resource, Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Events {
    hits: Vec<u64>,
    companion_shots: u32,
    completed_actions: u32,
}

fn move_robots(step: Res<SimulationStep>, mut robots: Query<&mut MotorState>) {
    for mut motor in &mut robots {
        motor.advance([0.0; 2], [1.0, 0.0], 1.0, None, step.0);
    }
}
fn refresh_targets(
    mut index: ResMut<TargetIndex>,
    mut robots: Query<(&mut CollisionTarget, &MotorState, &Health)>,
) {
    index.0.clear();
    for (mut target, motor, health) in &mut robots {
        target.position = motor.position;
        target.active = health.hp > 0.0;
        index.0.push(*target);
    }
    index.0.sort_by_key(|target| target.id);
}
fn consume_effects(
    mut commands: Commands,
    mut events: ResMut<Events>,
    mut robots: Query<(
        &CollisionTarget,
        &mut Health,
        &mut CombatState,
        &mut ActionState,
        &mut Progression,
    )>,
    mut bolts: Query<&mut ProjectileState>,
    mut companions: Query<&mut SummonState>,
    delays: Query<(Entity, &DelayedAction<u64>)>,
) {
    let mut ordered: Vec<_> = bolts.iter_mut().collect();
    ordered.sort_by_key(|bolt| bolt.id);
    for mut bolt in ordered {
        for impact in std::mem::take(&mut bolt.pending_impacts) {
            let Some((_, mut health, mut combat, _, _)) = robots
                .iter_mut()
                .find(|(target, ..)| target.id == impact.target_id)
            else {
                continue;
            };
            if health.hp > 0.0 && bolt.record_hit(impact.target_id) {
                resolve_damage(&mut health, &mut combat, 7.0, 1.0);
                events.hits.push(impact.target_id);
            }
        }
    }
    for mut companion in &mut companions {
        if companion.pending_shot.take().is_some() {
            events.companion_shots += 1;
        }
    }
    for (entity, delay) in &delays {
        if !delay.ready {
            continue;
        }
        if let Some((_, _, _, mut action, mut progression)) = robots
            .iter_mut()
            .find(|(target, ..)| target.id == delay.payload)
            && action.take_due()
        {
            action.begin_recovery(0.5);
            progression.gain(11.0, |level| level as f32 * 5.0).unwrap();
            events.completed_actions += 1;
        }
        commands.entity(entity).despawn();
    }
}

pub fn arena() -> App {
    let mut app = App::new();
    app.init_schedule(ArenaTick)
        .insert_resource(SimulationStep(0.25))
        .init_resource::<Events>()
        .add_plugins((
            CombatPlugin(ArenaTick),
            ActionPlugin(ArenaTick),
            MotorPlugin(ArenaTick),
            LoadoutPlugin::<u8, u8, _>::new(ArenaTick),
            DelayedActionPlugin::<u64, _>::new(ArenaTick),
            ProjectilePlugin::new(ArenaTick),
            SummonPlugin::new(ArenaTick),
        ))
        .configure_sets(
            ArenaTick,
            (
                ArenaStep::Timers,
                ArenaStep::Move,
                ArenaStep::Index,
                ArenaStep::Mechanics,
                ArenaStep::Resolve,
            )
                .chain(),
        )
        .configure_sets(
            ArenaTick,
            (
                CombatStep,
                ActionStep,
                MotorStep,
                LoadoutStep,
                DelayedActionStep,
            )
                .in_set(ArenaStep::Timers),
        )
        .configure_sets(
            ArenaTick,
            (ProjectileStep, SummonStep).in_set(ArenaStep::Mechanics),
        )
        .add_systems(ArenaTick, move_robots.in_set(ArenaStep::Move))
        .add_systems(ArenaTick, refresh_targets.in_set(ArenaStep::Index))
        .add_systems(ArenaTick, consume_effects.in_set(ArenaStep::Resolve));
    app
}

pub fn populate(app: &mut App) {
    for (id, faction, x) in [(1, 1, 0.0), (2, 1, 0.5), (3, 2, 1.0), (4, 2, 2.0)] {
        let mut combat = CombatState::default();
        combat.grant_shield(2.0, 0.75, ShieldPolicy::Add, DurationPolicy::Reset);
        combat.apply_status(StatusId(40), 0.8, 1.0, DurationPolicy::Reset);
        let mut action = ActionState::default();
        if id == 1 {
            action.commit([2.0, 0.0], 0.25);
        }
        let mut loadout: Loadout<u8, u8> =
            Loadout((0..3).map(|kind| AbilitySlot::new(kind, 0.5)).collect());
        loadout.0[2].modifier = Some(8);
        loadout.0[2].activate();
        app.world_mut().spawn((
            CollisionTarget {
                id,
                faction,
                position: [x, 0.0],
                radius: 0.1,
                active: true,
            },
            Health::new(20.0),
            combat,
            MotorState {
                position: [x, 0.0],
                ..Default::default()
            },
            action,
            loadout,
            Progression {
                level: 1,
                xp: 0.0,
                xp_next: 5.0,
                pending_levels: 0,
            },
        ));
    }
    app.world_mut().spawn(ProjectileState {
        id: 100,
        owner: 1,
        faction: 1,
        position: [0.0; 2],
        previous_position: [0.0; 2],
        direction: [1.0, 0.0],
        speed: 4.0,
        remaining: 1.5,
        radius: 0.05,
        hits_remaining: 3,
        hit_ids: vec![],
        max_distance: None,
        source_policy: ProjectileSourcePolicy::RequireActive,
        expired: false,
        pending_impacts: vec![],
    });
    app.world_mut().spawn(SummonState {
        id: 200,
        owner: 1,
        faction: 1,
        position: [0.0; 2],
        remaining: 1.0,
        orbit_angle: 0.0,
        orbit_radius: 0.0,
        orbit_speed: 1.0,
        fire_remaining: 0.0,
        fire_interval: 0.25,
        range: 3.0,
        expired: false,
        pending_shot: None,
    });
    app.world_mut().spawn(DelayedAction::new(0.75, 1_u64));
}

type Robot = (
    CollisionTarget,
    Health,
    CombatState,
    MotorState,
    ActionState,
    Loadout<u8, u8>,
    Progression,
);
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedArena {
    robots: Vec<Robot>,
    projectiles: Vec<ProjectileState>,
    summons: Vec<SummonState>,
    delays: Vec<DelayedAction<u64>>,
    events: Events,
}
pub fn save(app: &mut App) -> SavedArena {
    let world = app.world_mut();
    let mut robots: Vec<_> = world
        .query::<(
            &CollisionTarget,
            &Health,
            &CombatState,
            &MotorState,
            &ActionState,
            &Loadout<u8, u8>,
            &Progression,
        )>()
        .iter(world)
        .map(|(t, h, c, m, a, l, p)| (*t, *h, c.clone(), *m, *a, l.clone(), *p))
        .collect();
    robots.sort_by_key(|robot| robot.0.id);
    let mut projectiles: Vec<_> = world
        .query::<&ProjectileState>()
        .iter(world)
        .cloned()
        .collect();
    projectiles.sort_by_key(|bolt| bolt.id);
    let mut summons: Vec<_> = world.query::<&SummonState>().iter(world).cloned().collect();
    summons.sort_by_key(|summon| summon.id);
    let mut delays: Vec<_> = world
        .query::<&DelayedAction<u64>>()
        .iter(world)
        .cloned()
        .collect();
    delays.sort_by_key(|delay| delay.payload);
    SavedArena {
        robots,
        projectiles,
        summons,
        delays,
        events: world.resource::<Events>().clone(),
    }
}
pub fn restore(saved: SavedArena) -> App {
    let mut app = arena();
    for robot in saved.robots {
        app.world_mut().spawn(robot);
    }
    for projectile in saved.projectiles {
        app.world_mut().spawn(projectile);
    }
    for summon in saved.summons {
        app.world_mut().spawn(summon);
    }
    for delay in saved.delays {
        app.world_mut().spawn(delay);
    }
    app.insert_resource(saved.events);
    app
}
pub fn tick(app: &mut App) {
    app.world_mut().run_schedule(ArenaTick);
}

pub fn demonstrate() {
    let mut original = arena();
    populate(&mut original);
    tick(&mut original);
    let checkpoint = save(&mut original);
    assert_eq!(checkpoint.events.hits, vec![3]);
    assert!(checkpoint.robots[0].4.due);
    assert_eq!(checkpoint.robots[0].4.target, [2.0, 0.0]);
    assert_eq!(checkpoint.robots[0].5.0.len(), 3);
    assert_eq!(checkpoint.projectiles[0].hit_ids, vec![3]);
    let bytes = serde_json::to_vec(&checkpoint).unwrap();
    let mut restored = restore(serde_json::from_slice(&bytes).unwrap());
    assert_eq!(checkpoint, save(&mut restored));
    for _ in 0..8 {
        tick(&mut original);
        tick(&mut restored);
        assert_eq!(save(&mut original), save(&mut restored));
    }
    let result = save(&mut original);
    assert_eq!(result.events.hits, vec![3, 4]);
    assert_eq!(result.events.completed_actions, 1);
    assert_eq!(result.events.companion_shots, 3);
    assert!(result.projectiles[0].expired && result.summons[0].expired);
    assert!(result.delays.is_empty());
    assert_eq!(result.robots[1].1.hp, 20.0); // Same-faction robot is untouched.
    assert_eq!(result.robots[0].6.level, 2);
    assert_eq!(result.robots[0].6.xp, 6.0);
    assert!(result.robots[0].4.idle());
    assert!(result.robots[0].5.0.iter().all(AbilitySlot::ready));
    assert!(result.robots[0].2.statuses.is_empty());
}
fn main() {
    demonstrate();
    println!(
        "Robot arena: two hostile hits, three companion shots, one completed action; restored replay matches."
    );
}
