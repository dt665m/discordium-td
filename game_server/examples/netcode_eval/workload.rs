use game_shared::{ClientCommand, WorldDelta};
use game_sim::Simulation;
use std::time::Instant;

pub struct Workload {
    pub worlds: Vec<WorldDelta>,
    pub simulation_us: Vec<f64>,
    pub signature: u64,
}

/// Actual shared gameplay simulation, with durable actors so larger workloads do
/// not silently become cheaper through deaths during the timing window.
pub fn generate(players: usize, enemies: usize, ticks: usize) -> Workload {
    let mut sim = Simulation::new();
    for id in 1..=players {
        sim.add_player(id as u64);
    }
    for _ in 0..=game_shared::WAVE_PREP_TICKS {
        sim.step();
    }
    let mut world = sim.world_delta();
    let enemy = world.enemies[0];
    world.enemies = (0..enemies)
        .map(|index| {
            let mut e = enemy;
            e.id = 1_000_000 + index as u64;
            e.spawn.wave = 1 + (index / 84) as u32;
            e.spawn.ordinal = (index % 84) as u32;
            e.pos = [
                -15.0 + (index % 48) as f32 * 0.25,
                -7.0 + ((index / 48) % 32) as f32 * 0.375,
            ];
            e.hp = 1_000_000.0;
            e.max_hp = e.hp;
            e
        })
        .collect();
    for (index, hero) in world.heroes.iter_mut().enumerate() {
        hero.hp = 1_000_000.0;
        hero.pos = [-5.0 + (index % 8) as f32, -4.0 + (index / 8) as f32];
    }
    world.objectives[0].hp = 1_000_000.0;
    world.team_life = 1_000_000;
    let meta = world.sim_meta.as_mut().unwrap();
    meta.next_entity_id = 2_000_000;
    meta.wave_remaining = 0;
    meta.intermission_until = world.tick + ticks as u32 + 1000;
    sim = Simulation::from_snapshot(&world, world.sim_meta.as_ref().unwrap());
    let mut worlds = Vec::with_capacity(ticks);
    let mut simulation_us = Vec::with_capacity(ticks);
    let mut signature = 0xcbf29ce484222325u64;
    for tick in 0..ticks {
        let started = Instant::now();
        for id in 1..=players {
            let dir = match (tick / 20 + id) % 4 {
                0 => [1.0, 0.0],
                1 => [0.0, 1.0],
                2 => [-1.0, 0.0],
                _ => [0.0, -1.0],
            };
            sim.queue_command(
                id as u64,
                ClientCommand::Move {
                    seq: tick as u32 * 2,
                    dir,
                },
            );
            if tick % 15 == 0 {
                sim.queue_command(
                    id as u64,
                    ClientCommand::BasicAttack {
                        seq: tick as u32 * 2 + 1,
                    },
                );
            }
        }
        sim.step();
        let world = sim.world_delta();
        simulation_us.push(started.elapsed().as_secs_f64() * 1e6);
        assert_eq!(world.heroes.len(), players);
        assert_eq!(world.enemies.len(), enemies);
        for byte in serde_json::to_vec(&world).unwrap() {
            signature = (signature ^ byte as u64).wrapping_mul(0x100000001b3);
        }
        worlds.push(world);
    }
    Workload {
        worlds,
        simulation_us,
        signature,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workload_is_repeatable_and_keeps_actors_active() {
        let a = generate(4, 84, 80);
        let b = generate(4, 84, 80);
        assert_eq!(a.worlds, b.worlds);
        assert_eq!(a.signature, b.signature);
        assert_ne!(a.worlds[0].enemies[0].pos, a.worlds[79].enemies[0].pos);
        assert!(a.worlds.iter().any(|w| {
            w.enemies
                .iter()
                .any(|e| e.regular_attack.phase != game_shared::AttackPhase::Ready)
        }));
    }
}
