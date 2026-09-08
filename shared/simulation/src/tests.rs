use super::*;
#[cfg(test)]
mod tests {
    use super::*;

    fn insert_static_enemy(sim: &mut Simulation, id: u64, pos: [f32; 2], hp: f32) {
        insert_enemy(
            &mut sim.world,
            id,
            EnemyBundle {
                role: EnemyState {
                    spawn: game_shared::EnemySpawnIdentity {
                        wave: 0,
                        ordinal: id as u32,
                    },
                    id,
                    lane: 0,
                    vel: [0.0, 0.0],
                    speed: 0.0,
                    reward: 0,
                    enemy_type: EnemyType::Grunt,
                    radius: enemy_collider_radius(EnemyType::Grunt),
                    lock_target: EnemyLockTarget::Base,
                    target_pos: BASE_POSITION,
                    waypoint: BASE_POSITION,
                    repath_cooldown: 0,
                },
                position: Position { pos },
                health: Health { hp, max_hp: hp },
                facing: Facing {
                    direction: FacingComponent { dir: [-1.0, 0.0] },
                },
                attack: Attack {
                    state: DirectionalAttackStateComponent::default(),
                    profile: DirectionalAttackComponent {
                        range: 1.0,
                        arc_dot_threshold: 0.0,
                        damage: 0.0,
                        windup_ticks: 1,
                        recovery_ticks: 1,
                    },
                },
            },
        );
    }

    #[test]
    fn towers_select_simultaneously_before_applying_lethal_damage() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        hero_mut(&mut sim.world, &1).unwrap().position.pos = [-10.0, 0.0];
        insert_static_enemy(&mut sim, 1001, [2.0, 0.0], TOWER_DAMAGE * 0.5);
        insert_static_enemy(&mut sim, 1002, [4.0, 0.0], 100.0);
        for (id, pos) in [(2001, [0.0, 0.0]), (2002, [0.0, -2.0])] {
            insert_tower(
                &mut sim.world,
                id,
                TowerBundle {
                    role: TowerState {
                        id,
                        owner: 1,
                        lane: 0,
                        node_id: id as u32,
                        reload_ticks: 0,
                    },
                    position: Position { pos },
                },
            );
        }
        let output = sim.step();
        assert!(enemy(&sim.world, &1001).is_none());
        assert_eq!(enemy(&sim.world, &1002).unwrap().health.hp, 100.0);
        for id in [2001, 2002] {
            assert_eq!(
                tower(&sim.world, &id).unwrap().role.reload_ticks,
                TOWER_RELOAD_TICKS
            );
        }
        assert_eq!(
            output
                .reliable_events
                .iter()
                .filter(|event| matches!(event, ReliableGameEvent::EnemyKilled { .. }))
                .count(),
            1
        );
        assert!(output.reliable_events.iter().any(|event| matches!(
            event,
            ReliableGameEvent::EnemyKilled {
                enemy_id: 1001,
                killer: 1,
                ..
            }
        )));
    }

    #[test]
    fn lock_target_set_clear_validation() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        insert_static_enemy(&mut sim, 1001, [2.0, 0.0], 100.0);

        sim.queue_command(
            1,
            ClientCommand::SetLockTarget {
                seq: 1,
                target_id: Some(1001),
            },
        );
        sim.step();
        assert!(
            hero(&sim.world, &1)
                .map(|hero| hero.role.lock_mode_active)
                .unwrap_or(false)
        );
        assert_eq!(
            hero(&sim.world, &1).and_then(|hero| hero.role.lock_target_id),
            Some(1001)
        );

        sim.queue_command(
            1,
            ClientCommand::SetLockTarget {
                seq: 2,
                target_id: Some(999_999),
            },
        );
        sim.step();
        assert!(
            hero(&sim.world, &1)
                .map(|hero| hero.role.lock_mode_active)
                .unwrap_or(false)
        );
        assert_eq!(
            hero(&sim.world, &1).and_then(|hero| hero.role.lock_target_id),
            Some(1001)
        );

        sim.queue_command(
            1,
            ClientCommand::SetLockTarget {
                seq: 3,
                target_id: None,
            },
        );
        sim.step();
        assert!(
            !hero(&sim.world, &1)
                .map(|hero| hero.role.lock_mode_active)
                .unwrap_or(true)
        );
        assert_eq!(
            hero(&sim.world, &1).and_then(|hero| hero.role.lock_target_id),
            None
        );
    }

    #[test]
    fn hero_facing_tracks_locked_target_while_moving() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        insert_static_enemy(&mut sim, 1002, [0.0, 8.0], 100.0);

        if let Some(hero) = hero_mut(&mut sim.world, &1) {
            hero.position.pos = [0.0, 0.0];
            hero.facing.direction = FacingComponent { dir: [1.0, 0.0] };
        }

        sim.queue_command(
            1,
            ClientCommand::Move {
                seq: 1,
                dir: [1.0, 0.0],
            },
        );
        sim.queue_command(
            1,
            ClientCommand::SetLockTarget {
                seq: 2,
                target_id: Some(1002),
            },
        );
        sim.step();

        let hero = hero(&sim.world, &1).expect("hero should exist");
        assert!(hero.position.pos[0] > 0.0);
        assert!(hero.facing.direction.dir[1] > 0.9);
        assert!(hero.facing.direction.dir[0].abs() < 0.25);
    }

    #[test]
    fn objective_collision_blocks_hero_from_entering_base() {
        let mut sim = Simulation::new();
        sim.add_player(1);

        if let Some(hero) = hero_mut(&mut sim.world, &1) {
            hero.position.pos = [
                BASE_POSITION[0] + OBJECTIVE_COLLIDER_RADIUS + HERO_COLLIDER_RADIUS - 0.2,
                BASE_POSITION[1],
            ];
            hero.role.move_dir = [0.0, 0.0];
        }

        sim.step();
        let hero = hero(&sim.world, &1).expect("hero should exist");
        let dist = distance_sq(hero.position.pos, BASE_POSITION).sqrt();
        assert!(dist + 0.0001 >= OBJECTIVE_COLLIDER_RADIUS + HERO_COLLIDER_RADIUS - 0.01);
    }

    #[test]
    fn enemy_facing_tracks_locked_target_position() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        insert_static_enemy(&mut sim, 1099, [0.0, 0.0], 100.0);

        if let Some(hero) = hero_mut(&mut sim.world, &1) {
            hero.position.pos = [0.0, 3.0];
            hero.role.move_dir = [0.0, 0.0];
        }
        if let Some(enemy) = enemy_mut(&mut sim.world, &1099) {
            enemy.facing.direction = FacingComponent { dir: [1.0, 0.0] };
        }

        sim.step();
        let enemy = enemy(&sim.world, &1099).expect("enemy should exist");
        assert!(enemy.facing.direction.dir[1] > 0.95);
        assert!(enemy.facing.direction.dir[0].abs() < 0.2);
    }

    #[test]
    fn lock_target_retargets_when_enemy_removed_if_mode_active() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        insert_static_enemy(&mut sim, 1003, [2.0, 0.0], 100.0);
        insert_static_enemy(&mut sim, 1004, [4.0, 0.0], 100.0);

        sim.queue_command(
            1,
            ClientCommand::SetLockTarget {
                seq: 1,
                target_id: Some(1003),
            },
        );
        sim.step();
        assert_eq!(
            hero(&sim.world, &1).and_then(|hero| hero.role.lock_target_id),
            Some(1003)
        );

        remove_enemy(&mut sim.world, &1003);
        sim.step();
        assert_eq!(
            hero(&sim.world, &1).and_then(|hero| hero.role.lock_target_id),
            Some(1004)
        );
        assert!(
            hero(&sim.world, &1)
                .map(|hero| hero.role.lock_mode_active)
                .unwrap_or(false)
        );
    }

    #[test]
    fn lock_mode_disables_when_no_targets_remain() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        insert_static_enemy(&mut sim, 1101, [2.0, 0.0], 100.0);

        sim.queue_command(
            1,
            ClientCommand::SetLockTarget {
                seq: 1,
                target_id: Some(1101),
            },
        );
        sim.step();
        assert!(
            hero(&sim.world, &1)
                .map(|hero| hero.role.lock_mode_active)
                .unwrap_or(false)
        );
        assert_eq!(
            hero(&sim.world, &1).and_then(|hero| hero.role.lock_target_id),
            Some(1101)
        );

        remove_enemy(&mut sim.world, &1101);
        sim.step();

        assert_eq!(
            hero(&sim.world, &1).and_then(|hero| hero.role.lock_target_id),
            None
        );
        assert!(
            !hero(&sim.world, &1)
                .map(|hero| hero.role.lock_mode_active)
                .unwrap_or(true)
        );
    }

    #[test]
    fn simultaneous_hero_attacks_filter_removed_locked_targets_before_ordering() {
        let mut sim = Simulation::new();
        for (id, pos) in [(1, [-2.0, -1.0]), (2, [-2.0, 1.0])] {
            sim.add_player(id);
            let hero = hero_mut(&mut sim.world, &id).unwrap();
            hero.position.pos = pos;
            hero.facing.direction.dir = [1.0, 0.0];
            hero.attack.profile.range = 10.0;
            hero.attack.profile.windup_ticks = 1;
        }
        // Allocate out of ID order. The two survivors are equidistant from hero 2,
        // so their kill order must use the stable ID tie-breaker.
        for (id, pos, hp, reward) in [
            (1004, [2.0, 2.0], 45.0, 14),
            (1003, [6.0, 1.0], 15.0, 13),
            (1001, [6.0, -1.0], 15.0, 11),
            (1002, [2.0, 0.0], 45.0, 12),
        ] {
            insert_static_enemy(&mut sim, id, pos, hp);
            enemy_mut(&mut sim.world, &id).unwrap().role.reward = reward;
        }
        {
            let hero = hero_mut(&mut sim.world, &2).unwrap();
            hero.role.lock_mode_active = true;
            hero.role.lock_target_id = Some(1003);
        }
        for id in [1, 2] {
            sim.queue_command(id, ClientCommand::BasicAttack { seq: 1 });
        }
        let output = sim.step();
        let kills: Vec<_> = output
            .reliable_events
            .iter()
            .filter_map(|event| {
                if let ReliableGameEvent::EnemyKilled {
                    enemy_id,
                    killer,
                    reward,
                } = event
                {
                    Some((*enemy_id, *killer, *reward))
                } else {
                    None
                }
            })
            .collect();
        // Hero 1 kills hero 2's locked target. That removed target must be filtered
        // before lock prioritization, or swapping its cached slot reverses 1002/1004.
        assert_eq!(
            kills,
            vec![(1001, 1, 11), (1003, 1, 13), (1002, 2, 12), (1004, 2, 14)]
        );
        assert!(actor_ids::<EnemyState>(&sim.world).is_empty());
        assert_eq!(hero(&sim.world, &1).unwrap().role.gold, INITIAL_GOLD + 24);
        assert_eq!(hero(&sim.world, &2).unwrap().role.gold, INITIAL_GOLD + 26);
        for id in [1, 2] {
            assert_eq!(
                hero(&sim.world, &id).unwrap().attack.state.phase,
                AttackPhase::Recovery
            );
        }
    }

    #[test]
    fn regular_attack_hits_all_enemies_in_cone() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        insert_static_enemy(&mut sim, 2001, [1.0, 0.0], 120.0);
        insert_static_enemy(&mut sim, 2002, [2.0, 0.0], 120.0);
        insert_static_enemy(&mut sim, 2003, [0.0, 2.0], 120.0);

        if let Some(hero) = hero_mut(&mut sim.world, &1) {
            hero.position.pos = [0.0, 0.0];
            hero.role.move_dir = [0.0, 0.0];
            hero.facing.direction = FacingComponent { dir: [1.0, 0.0] };
        }

        sim.queue_command(
            1,
            ClientCommand::SetLockTarget {
                seq: 1,
                target_id: Some(2002),
            },
        );
        sim.queue_command(1, ClientCommand::BasicAttack { seq: 2 });
        for _ in 0..5 {
            sim.step();
        }

        let near_hp = enemy(&sim.world, &2001)
            .map(|enemy| enemy.health.hp)
            .expect("near enemy should exist");
        let locked_hp = enemy(&sim.world, &2002)
            .map(|enemy| enemy.health.hp)
            .expect("locked enemy should exist");
        let outside_hp = enemy(&sim.world, &2003)
            .map(|enemy| enemy.health.hp)
            .expect("outside-cone enemy should exist");

        assert!((near_hp - (120.0 - HERO_REGULAR_ATTACK.damage)).abs() < 0.001);
        assert!((locked_hp - (120.0 - HERO_REGULAR_ATTACK.damage)).abs() < 0.001);
        assert!((outside_hp - 120.0).abs() < 0.001);
    }

    #[test]
    fn charge_startup_and_release_lag_lock_movement() {
        let mut sim = Simulation::new();
        sim.add_player(1);

        if let Some(hero) = hero_mut(&mut sim.world, &1) {
            hero.position.pos = [0.0, 0.0];
            hero.role.move_dir = [0.0, 0.0];
        }

        sim.queue_command(
            1,
            ClientCommand::Move {
                seq: 1,
                dir: [1.0, 0.0],
            },
        );
        sim.step();
        let baseline_x = hero(&sim.world, &1)
            .map(|hero| hero.position.pos[0])
            .unwrap_or(0.0);
        assert!(baseline_x > 0.0);

        sim.queue_command(
            1,
            ClientCommand::SetCharging {
                seq: 2,
                active: true,
            },
        );
        sim.queue_command(
            1,
            ClientCommand::Move {
                seq: 3,
                dir: [1.0, 0.0],
            },
        );
        sim.step();
        let locked_x = hero(&sim.world, &1)
            .map(|hero| hero.position.pos[0])
            .unwrap_or(0.0);
        assert!((locked_x - baseline_x).abs() < 0.0001);

        let startup_ticks = hero(&sim.world, &1)
            .map(|hero| hero.role.charge_profile.startup_ticks)
            .unwrap_or(0);
        for _ in 0..startup_ticks {
            sim.step();
        }
        let charging_locked_x = hero(&sim.world, &1)
            .map(|hero| hero.position.pos[0])
            .unwrap_or(0.0);
        assert!((charging_locked_x - baseline_x).abs() < 0.0001);

        sim.queue_command(
            1,
            ClientCommand::SetCharging {
                seq: 4,
                active: false,
            },
        );
        sim.step();
        let release_x = hero(&sim.world, &1)
            .map(|hero| hero.position.pos[0])
            .unwrap_or(0.0);

        let release_lag_ticks = hero(&sim.world, &1)
            .map(|hero| hero.role.charge_profile.release_lag_ticks)
            .unwrap_or(0);
        let mut steps_after_release = 0_u32;
        let mut resumed_x = release_x;
        let mut next_seq = 5_u32;
        while steps_after_release <= release_lag_ticks.saturating_add(2) {
            // Send a move command each tick (like a real client would)
            sim.queue_command(
                1,
                ClientCommand::Move {
                    seq: next_seq,
                    dir: [1.0, 0.0],
                },
            );
            next_seq += 1;
            sim.step();
            steps_after_release += 1;
            resumed_x = hero(&sim.world, &1)
                .map(|hero| hero.position.pos[0])
                .unwrap_or(0.0);
            if resumed_x > release_x {
                break;
            }
        }

        assert!(resumed_x > release_x);
        assert!(steps_after_release >= release_lag_ticks.saturating_sub(1));
    }

    #[test]
    fn charge_fills_mana_then_power_then_health_before_auto_exit() {
        let mut sim = Simulation::new();
        sim.add_player(1);

        if let Some(hero) = hero_mut(&mut sim.world, &1) {
            hero.role.mana = 0.0;
            hero.health.hp = 60.0;
            hero.role.charge_profile.startup_ticks = 0;
            hero.role.charge_profile.release_lag_ticks = 1;
            hero.role.charge_profile.mana_per_tick = 20.0;
            hero.role.charge_profile.health_per_tick = 10.0;
            hero.role.charge_profile.power_gain_per_tick = 60.0;
            hero.role.charge_profile.power_meter_max = 100.0;
            hero.role.charge_state = ChargeStateComponent::default();
        }

        sim.queue_command(
            1,
            ClientCommand::SetCharging {
                seq: 1,
                active: true,
            },
        );
        sim.step();
        let first = hero(&sim.world, &1)
            .map(|view| view.snapshot())
            .expect("hero should exist");
        assert!(first.role.mana > 0.0);
        assert!((first.health.hp - 60.0).abs() < 0.0001);

        while hero(&sim.world, &1)
            .map(|hero| hero.role.mana + 0.0001 < HERO_MAX_MANA)
            .unwrap_or(false)
        {
            sim.step();
        }
        let mana_full_hp = hero(&sim.world, &1)
            .map(|hero| hero.health.hp)
            .unwrap_or(0.0);
        let mana_full_power = hero(&sim.world, &1)
            .map(|hero| hero.role.charge_state.power_meter)
            .unwrap_or(0.0);
        sim.step();
        let post_mana = hero(&sim.world, &1)
            .map(|view| view.snapshot())
            .expect("hero should exist");
        assert!((post_mana.health.hp - mana_full_hp).abs() < 0.0001);
        assert!(post_mana.role.charge_state.power_meter > mana_full_power);

        if let Some(hero) = hero_mut(&mut sim.world, &1) {
            hero.role.mana = HERO_MAX_MANA;
            hero.health.hp = 80.0;
            hero.role.charge_state.phase = ChargePhase::Charging;
            hero.role.charge_state.input_held = true;
            hero.role.charge_state.power_meter = 95.0;
        }
        sim.step();
        let after_full_power = hero(&sim.world, &1)
            .map(|view| view.snapshot())
            .expect("hero should exist");
        assert!(after_full_power.role.charge_state.power_active);
        assert_eq!(
            after_full_power.role.charge_state.phase,
            ChargePhase::Charging
        );
        assert!(after_full_power.health.hp > 80.0);

        sim.step();
        let charged = hero(&sim.world, &1)
            .map(|view| view.snapshot())
            .expect("hero should exist");
        assert!((charged.health.hp - HERO_MAX_HP).abs() < 0.001);
        assert!(
            charged.role.charge_state.phase == ChargePhase::Recovery
                || charged.role.charge_state.phase == ChargePhase::Idle
        );
    }

    #[test]
    fn charging_is_blocked_while_power_mode_is_active() {
        let mut sim = Simulation::new();
        sim.add_player(1);

        if let Some(hero) = hero_mut(&mut sim.world, &1) {
            hero.role.charge_state.power_active = true;
            hero.role.charge_state.power_meter = hero.role.charge_profile.power_meter_max;
            hero.role.charge_state.phase = ChargePhase::Idle;
            hero.role.charge_state.input_held = false;
            hero.role.charge_state.power_decay_ticks_remaining = 10_000;
            hero.role.power_decay_profile.interval_ticks = 10_000;
            hero.role.power_decay_profile.amount_per_interval = 0.0;
        }

        sim.queue_command(
            1,
            ClientCommand::SetCharging {
                seq: 1,
                active: true,
            },
        );
        sim.step();

        let hero = hero(&sim.world, &1)
            .map(|view| view.snapshot())
            .expect("hero should exist");
        assert_eq!(hero.role.charge_state.phase, ChargePhase::Idle);
        assert!(!hero.role.charge_state.input_held);
        assert!(hero.role.charge_state.power_active);
    }

    #[test]
    fn power_meter_buff_doubles_damage_and_decays_to_disable() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        insert_static_enemy(&mut sim, 3001, [1.0, 0.0], 200.0);

        if let Some(hero) = hero_mut(&mut sim.world, &1) {
            hero.position.pos = [0.0, 0.0];
            hero.facing.direction = FacingComponent { dir: [1.0, 0.0] };
            hero.role.charge_state.power_active = true;
            hero.role.charge_state.power_meter = 100.0;
            hero.role.charge_state.power_decay_ticks_remaining = 10_000;
            hero.role.powered_modifiers.attack_damage_multiplier = 2.0;
            hero.role.power_decay_profile.interval_ticks = 10_000;
        }

        sim.queue_command(1, ClientCommand::BasicAttack { seq: 1 });
        for _ in 0..=HERO_REGULAR_ATTACK.windup_ticks {
            sim.step();
        }

        let hp_after = enemy(&sim.world, &3001)
            .map(|enemy| enemy.health.hp)
            .expect("enemy should exist");
        assert!((hp_after - (200.0 - HERO_REGULAR_ATTACK.damage * 2.0)).abs() < 0.001);

        if let Some(hero) = hero_mut(&mut sim.world, &1) {
            hero.role.charge_state.power_active = true;
            hero.role.charge_state.power_meter = 9.0;
            hero.role.charge_state.power_decay_ticks_remaining = 1;
            hero.role.power_decay_profile.interval_ticks = 1;
            hero.role.power_decay_profile.amount_per_interval = 5.0;
        }

        sim.step();
        assert!(
            hero(&sim.world, &1)
                .map(|hero| hero.role.charge_state.power_active)
                .unwrap_or(false)
        );
        sim.step();
        let power_active = hero(&sim.world, &1)
            .map(|hero| hero.role.charge_state.power_active)
            .unwrap_or(true);
        assert!(!power_active);
    }

    #[test]
    fn powered_ability_radius_hits_farther_targets() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        insert_static_enemy(&mut sim, 3101, [9.0, 0.0], 200.0);

        if let Some(hero) = hero_mut(&mut sim.world, &1) {
            hero.position.pos = [0.0, 0.0];
            hero.facing.direction = FacingComponent { dir: [1.0, 0.0] };
            hero.role.charge_state.power_active = true;
            hero.role.charge_state.power_meter = hero.role.charge_profile.power_meter_max;
            hero.role.charge_state.power_decay_ticks_remaining = 10_000;
            hero.role.power_decay_profile.interval_ticks = 10_000;
        }

        sim.queue_command(
            1,
            ClientCommand::CastAbility {
                seq: 1,
                ability: AbilityId::ArcBurst,
            },
        );
        sim.step();

        let hp_after = enemy(&sim.world, &3101)
            .map(|enemy| enemy.health.hp)
            .expect("enemy should exist");
        assert!((hp_after - (200.0 - HERO_ABILITY_DAMAGE * 2.0)).abs() < 0.001);
    }

    #[test]
    fn powered_regular_attack_range_reaches_far_targets() {
        let mut baseline = Simulation::new();
        baseline.add_player(1);
        insert_static_enemy(&mut baseline, 3201, [8.0, 0.0], 200.0);

        if let Some(hero) = hero_mut(&mut baseline.world, &1) {
            hero.position.pos = [0.0, 0.0];
            hero.facing.direction = FacingComponent { dir: [1.0, 0.0] };
        }

        baseline.queue_command(1, ClientCommand::BasicAttack { seq: 1 });
        for _ in 0..=HERO_REGULAR_ATTACK.windup_ticks {
            baseline.step();
        }
        let baseline_hp = enemy(&baseline.world, &3201)
            .map(|enemy| enemy.health.hp)
            .expect("enemy should exist");
        assert!((baseline_hp - 200.0).abs() < 0.001);

        let mut powered = Simulation::new();
        powered.add_player(1);
        insert_static_enemy(&mut powered, 3201, [8.0, 0.0], 200.0);

        if let Some(hero) = hero_mut(&mut powered.world, &1) {
            hero.position.pos = [0.0, 0.0];
            hero.facing.direction = FacingComponent { dir: [1.0, 0.0] };
            hero.role.charge_state.power_active = true;
            hero.role.charge_state.power_meter = hero.role.charge_profile.power_meter_max;
            hero.role.charge_state.power_decay_ticks_remaining = 10_000;
            hero.role.power_decay_profile.interval_ticks = 10_000;
        }

        powered.queue_command(1, ClientCommand::BasicAttack { seq: 1 });
        for _ in 0..=HERO_REGULAR_ATTACK.windup_ticks {
            powered.step();
        }
        let powered_hp = enemy(&powered.world, &3201)
            .map(|enemy| enemy.health.hp)
            .expect("enemy should exist");
        assert!((powered_hp - (200.0 - HERO_REGULAR_ATTACK.damage * 2.0)).abs() < 0.001);
    }

    #[test]
    fn partial_power_meter_decays_without_activating_power_mode() {
        let mut sim = Simulation::new();
        sim.add_player(1);

        if let Some(hero) = hero_mut(&mut sim.world, &1) {
            hero.role.mana = HERO_MAX_MANA;
            hero.health.hp = HERO_MAX_HP;
            hero.role.charge_profile.startup_ticks = 0;
            hero.role.charge_profile.release_lag_ticks = 0;
            hero.role.charge_profile.power_gain_per_tick = 20.0;
            hero.role.charge_profile.power_meter_max = 100.0;
            hero.role.power_decay_profile.interval_ticks = 1;
            hero.role.power_decay_profile.amount_per_interval = 5.0;
        }

        sim.queue_command(
            1,
            ClientCommand::SetCharging {
                seq: 1,
                active: true,
            },
        );
        sim.step();
        sim.step();
        sim.queue_command(
            1,
            ClientCommand::SetCharging {
                seq: 2,
                active: false,
            },
        );
        sim.step();

        let partial = hero(&sim.world, &1)
            .map(|view| view.snapshot())
            .expect("hero should exist");
        assert!(partial.role.charge_state.power_meter > 0.0);
        assert!(
            partial.role.charge_state.power_meter < partial.role.charge_profile.power_meter_max
        );
        assert!(!partial.role.charge_state.power_active);

        let mut last_power_meter = partial.role.charge_state.power_meter;
        for _ in 0..4 {
            sim.step();
            let hero = hero(&sim.world, &1)
                .map(|view| view.snapshot())
                .expect("hero should exist");
            assert!(!hero.role.charge_state.power_active);
            assert!(hero.role.charge_state.power_meter <= last_power_meter + 0.0001);
            last_power_meter = hero.role.charge_state.power_meter;
        }
    }

    #[test]
    fn powered_mode_halves_special_mana_cost_and_cooldown() {
        let mut sim = Simulation::new();
        sim.add_player(1);

        if let Some(hero) = hero_mut(&mut sim.world, &1) {
            hero.role.mana = HERO_ABILITY_MANA_COST;
            hero.role.ability_cooldown_ticks = 0;
            hero.role.charge_state.power_active = true;
            hero.role.powered_modifiers.ability_mana_cost_multiplier = 0.5;
            hero.role.powered_modifiers.ability_cooldown_multiplier = 0.5;
            hero.role.power_decay_profile.interval_ticks = 10_000;
            hero.role.power_decay_profile.amount_per_interval = 0.0;
            hero.role.charge_state.power_meter = hero.role.charge_profile.power_meter_max;
            hero.role.charge_state.power_decay_ticks_remaining = 10_000;
        }

        sim.queue_command(
            1,
            ClientCommand::CastAbility {
                seq: 1,
                ability: AbilityId::ArcBurst,
            },
        );
        sim.step();

        let hero = hero(&sim.world, &1)
            .map(|view| view.snapshot())
            .expect("hero should exist");
        let expected_mana = HERO_ABILITY_MANA_COST * 0.5 + HERO_MANA_REGEN_PER_TICK;
        assert!((hero.role.mana - expected_mana).abs() < 0.001);
        assert_eq!(
            hero.role.ability_cooldown_ticks,
            HERO_ABILITY_COOLDOWN_TICKS / 2 - 1
        );
    }

    #[test]
    fn defeat_phase_auto_resets_match_state() {
        let mut sim = Simulation::new();
        sim.add_player(1);

        if let Some(hero) = hero_mut(&mut sim.world, &1) {
            hero.role.gold = 999;
            hero.health.hp = 12.0;
            hero.role.mana = 4.0;
            hero.position.pos = [7.0, -3.0];
            hero.role.lock_mode_active = true;
            hero.role.lock_target_id = Some(42);
        }
        insert_tower(
            &mut sim.world,
            777,
            TowerBundle {
                role: TowerState {
                    id: 777,
                    owner: 1,
                    lane: 0,
                    node_id: BUILD_NODES[0].node_id,
                    reload_ticks: 0,
                },
                position: Position {
                    pos: BUILD_NODES[0].pos,
                },
            },
        );
        sim.world
            .resource_mut::<Globals>()
            .node_occupancy
            .insert(BUILD_NODES[0].node_id, 777);
        sim.world.resource_mut::<Globals>().team_life = 0;
        sim.world.resource_mut::<Globals>().wave = 9;

        sim.step();
        assert_eq!(
            sim.world.resource_mut::<Globals>().phase,
            MatchPhase::Defeat
        );
        let reset_at = sim
            .world
            .resource_mut::<Globals>()
            .reset_at_tick
            .expect("reset should be scheduled");

        while sim.tick() < reset_at {
            sim.step();
        }

        assert_eq!(
            sim.world.resource_mut::<Globals>().phase,
            MatchPhase::InProgress
        );
        assert_eq!(sim.world.resource_mut::<Globals>().wave, 0);
        assert_eq!(
            sim.world.resource_mut::<Globals>().team_life,
            INITIAL_TEAM_LIFE
        );
        assert_eq!(
            sim.world.resource_mut::<Globals>().objective_hp,
            OBJECTIVE_MAX_HP
        );
        assert!(actor_ids::<TowerState>(&sim.world).is_empty());
        assert!(
            sim.world
                .resource_mut::<Globals>()
                .node_occupancy
                .is_empty()
        );
        assert!(sim.world.resource_mut::<Globals>().reset_at_tick.is_none());
        let hero = hero(&sim.world, &1).expect("hero should exist");
        assert_eq!(hero.role.gold, INITIAL_GOLD);
        assert!((hero.health.hp - HERO_MAX_HP).abs() < 0.001);
        assert!((hero.role.mana - HERO_MAX_MANA).abs() < 0.001);
        assert!(!hero.role.lock_mode_active);
        assert!(hero.role.lock_target_id.is_none());
    }

    #[test]
    fn victory_phase_auto_resets_match_state() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        sim.world.resource_mut::<Globals>().wave = MAX_WAVES;
        sim.world.resource_mut::<Globals>().wave_remaining = 0;
        sim.world.resource_mut::<Globals>().intermission_until = 0;

        sim.step();
        assert_eq!(
            sim.world.resource_mut::<Globals>().phase,
            MatchPhase::Victory
        );
        let reset_at = sim
            .world
            .resource_mut::<Globals>()
            .reset_at_tick
            .expect("reset should be scheduled");

        while sim.tick() < reset_at {
            sim.step();
        }

        assert_eq!(
            sim.world.resource_mut::<Globals>().phase,
            MatchPhase::InProgress
        );
        assert_eq!(sim.world.resource_mut::<Globals>().wave, 0);
        assert_eq!(
            sim.world.resource_mut::<Globals>().team_life,
            INITIAL_TEAM_LIFE
        );
        assert!(sim.world.resource_mut::<Globals>().intermission_until > sim.tick());
        assert!(sim.world.resource_mut::<Globals>().reset_at_tick.is_none());
    }

    #[test]
    fn enemy_target_keeps_current_hero_until_disengage_then_retargets() {
        let heroes = vec![(1_u64, [0.0, 0.0]), (2_u64, [4.0, 0.0])];

        let kept = choose_enemy_target([10.5, 0.0], EnemyLockTarget::Hero(1), &heroes);
        assert_eq!(kept, EnemyLockTarget::Hero(1));

        let retargeted = choose_enemy_target([12.0, 0.0], EnemyLockTarget::Hero(1), &heroes);
        assert_eq!(retargeted, EnemyLockTarget::Hero(2));
    }
}

#[cfg(test)]
mod netcode_regressions {
    use super::*;

    #[test]
    fn delayed_reliable_action_survives_newer_movement() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        sim.queue_command(
            1,
            ClientCommand::Move {
                seq: 101,
                dir: [0.0, 0.0],
            },
        );
        sim.step();
        sim.queue_command(1, ClientCommand::BasicAttack { seq: 100 });
        sim.step();
        assert_eq!(
            hero(&sim.world, &1).unwrap().attack.state.phase,
            AttackPhase::Windup
        );
    }

    #[test]
    fn actions_cannot_fast_forward_queued_movement() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        let start = hero(&sim.world, &1).unwrap().position.pos;
        for seq in 1..=10 {
            sim.queue_command(
                1,
                ClientCommand::Move {
                    seq,
                    dir: [1.0, 0.0],
                },
            );
        }
        sim.queue_command(
            1,
            ClientCommand::SetLockTarget {
                seq: 11,
                target_id: None,
            },
        );
        sim.step();
        assert!(
            hero(&sim.world, &1).unwrap().position.pos[0] - start[0]
                <= HERO_SPEED * FIXED_DT_SECONDS + 0.00001
        );
    }

    #[test]
    fn restored_world_replays_identically_with_movement_backlog() {
        let mut original = Simulation::new();
        original.add_player(1);
        original.add_player(2);
        hero_mut(&mut original.world, &1).unwrap().position.pos = [0.0, 3.0];
        hero_mut(&mut original.world, &2).unwrap().position.pos = [1.0, 3.0];
        for i in 0..8 {
            original.spawn_enemy(i % ENEMY_SPAWN_POINTS.len(), 1);
        }
        for (i, id) in actor_ids::<EnemyState>(&original.world)
            .into_iter()
            .enumerate()
        {
            let enemy = enemy_mut(&mut original.world, &id).unwrap();
            enemy.position.pos = [i as f32 * 0.35, 3.5];
        }
        for seq in 1..=5 {
            original.queue_command(
                1,
                ClientCommand::Move {
                    seq,
                    dir: [1.0, 0.0],
                },
            );
            original.queue_command(
                2,
                ClientCommand::Move {
                    seq,
                    dir: [-1.0, 0.0],
                },
            );
        }
        original.step();
        let delta = original.world_delta();
        let mut restored = Simulation::from_snapshot(&delta, &original.sim_meta());
        assert_eq!(delta, restored.world_delta());
        for tick in 0..30 {
            let a = original.step();
            let b = restored.step();
            assert_eq!(
                a.reliable_events, b.reliable_events,
                "events at replay {tick}"
            );
            assert_eq!(
                original.world_delta(),
                restored.world_delta(),
                "replay {tick}"
            );
        }
    }

    #[test]
    fn snapshot_restores_enemy_decision_state() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        sim.spawn_enemy(0, 1);
        let id = actor_ids::<EnemyState>(&sim.world)[0];
        let enemy = enemy_mut(&mut sim.world, &id).unwrap();
        enemy.role.lock_target = EnemyLockTarget::Hero(1);
        enemy.role.target_pos = [3.0, 4.0];
        enemy.role.waypoint = [2.0, 3.0];
        enemy.role.repath_cooldown = 5;
        let delta = sim.world_delta();
        let restored = Simulation::from_snapshot(&delta, &sim.sim_meta());
        let a = enemies(&sim.world).next().unwrap();
        let b = enemies(&restored.world).next().unwrap();
        assert_eq!(a.role.lock_target, b.role.lock_target);
        assert_eq!(a.role.target_pos, b.role.target_pos);
        assert_eq!(a.role.waypoint, b.role.waypoint);
        assert_eq!(a.role.repath_cooldown, b.role.repath_cooldown);
    }
}

#[cfg(test)]
mod protocol_edge_tests {
    use super::*;

    #[test]
    fn duplicate_and_reordered_redundant_moves_are_consumed_once() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        let start = hero(&sim.world, &1).unwrap().position.pos;
        for seq in [u32::MAX, 0, 1, u32::MAX, 0, 1] {
            sim.queue_command(
                1,
                ClientCommand::Move {
                    seq,
                    dir: [1.0, 0.0],
                },
            );
        }
        for _ in 0..4 {
            sim.step();
        }
        assert_eq!(
            hero(&sim.world, &1).unwrap().role.last_processed_seq,
            Some(1)
        );
        assert!(
            (hero(&sim.world, &1).unwrap().position.pos[0]
                - start[0]
                - 3.0 * HERO_SPEED * FIXED_DT_SECONDS)
                .abs()
                < 0.00001
        );
        sim.queue_command(
            1,
            ClientCommand::Move {
                seq: 0,
                dir: [1.0, 0.0],
            },
        );
        sim.step();
        assert_eq!(
            hero(&sim.world, &1).unwrap().role.last_processed_seq,
            Some(1)
        );
    }

    #[test]
    fn nonfinite_input_and_disconnect_cannot_leak_into_new_session() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        sim.queue_command(
            1,
            ClientCommand::Move {
                seq: 100,
                dir: [f32::NAN, 0.0],
            },
        );
        sim.queue_command(1, ClientCommand::BasicAttack { seq: 101 });
        sim.remove_player(1);
        sim.add_player(1);
        sim.step();
        assert_eq!(hero(&sim.world, &1).unwrap().role.last_action_seq, None);
        assert_eq!(hero(&sim.world, &1).unwrap().role.last_processed_seq, None);
        assert!(
            hero(&sim.world, &1)
                .unwrap()
                .position
                .pos
                .iter()
                .all(|x| x.is_finite())
        );
    }

    #[test]
    fn duplicate_actions_do_not_restart_after_cooldown() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        sim.queue_command(1, ClientCommand::BasicAttack { seq: u32::MAX });
        sim.step();
        for _ in 0..60 {
            sim.step();
        }
        sim.queue_command(1, ClientCommand::BasicAttack { seq: u32::MAX });
        sim.step();
        assert_eq!(
            hero(&sim.world, &1).unwrap().attack.state.phase,
            AttackPhase::Ready
        );
        sim.queue_command(1, ClientCommand::BasicAttack { seq: 0 });
        sim.step();
        assert_eq!(
            hero(&sim.world, &1).unwrap().attack.state.phase,
            AttackPhase::Windup
        );
    }

    #[test]
    fn match_reset_waits_through_tick_wrap_and_discards_old_moves() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        sim.set_tick(u32::MAX - 2);
        sim.world.resource_mut::<Globals>().phase = MatchPhase::Defeat;
        hero_mut(&mut sim.world, &1)
            .unwrap()
            .role
            .pending_moves
            .push_back((5, [1.0, 0.0]));
        sim.schedule_match_reset();
        assert_eq!(sim.match_restart_ticks_remaining(), Some(MATCH_RESET_TICKS));
        for _ in 0..MATCH_RESET_TICKS - 1 {
            sim.step();
        }
        assert_eq!(
            sim.world.resource_mut::<Globals>().phase,
            MatchPhase::Defeat
        );
        sim.step();
        assert_eq!(
            sim.world.resource_mut::<Globals>().phase,
            MatchPhase::InProgress
        );
        assert!(hero(&sim.world, &1).unwrap().role.pending_moves.is_empty());
        assert_eq!(
            sim.world.resource::<Globals>().intermission_until,
            sim.tick().wrapping_add(WAVE_PREP_TICKS)
        );
    }
}

#[cfg(test)]
mod action_timeline_tests {
    use super::*;
    use game_shared::ClientAction;

    #[test]
    fn early_attack_waits_for_walk_and_snapshot_restore_keeps_boundary() {
        let mut server = Simulation::new();
        server.add_player(1);
        let start = hero(&server.world, &1).unwrap().position.pos;
        let action = ClientAction {
            view_tick: None,
            match_epoch: 0,
            command: ClientCommand::BasicAttack { seq: 6 },
            after_move_seq: Some(5),
        };
        server.queue_action(1, action); // reliable channel wins the race
        server.step();
        assert_eq!(
            hero(&server.world, &1).unwrap().attack.state.phase,
            AttackPhase::Ready
        );
        for seq in 1..=2 {
            server.queue_command(
                1,
                ClientCommand::Move {
                    seq,
                    dir: [1.0, 0.0],
                },
            );
            server.step();
        }
        let snapshot = server.world_delta();
        let mut restored = Simulation::from_snapshot(&snapshot, &server.sim_meta());
        for seq in 3..=5 {
            server.queue_action(1, action); // repeated in movement packets
            restored.queue_action(1, action);
            let movement = ClientCommand::Move {
                seq,
                dir: [1.0, 0.0],
            };
            server.queue_command(1, movement);
            restored.queue_command(1, movement);
            server.step();
            restored.step();
            assert_eq!(server.world_delta(), restored.world_delta());
            assert_eq!(
                hero(&server.world, &1).unwrap().attack.state.phase,
                AttackPhase::Ready
            );
        }
        let stop = hero(&server.world, &1).unwrap().position.pos;
        assert!((stop[0] - start[0] - 5.0 * HERO_SPEED * FIXED_DT_SECONDS).abs() < 0.0001);
        server.queue_command(
            1,
            ClientCommand::Move {
                seq: 7,
                dir: [1.0, 0.0],
            },
        );
        server.step();
        assert_eq!(hero(&server.world, &1).unwrap().position.pos, stop);
        assert_eq!(
            hero(&server.world, &1).unwrap().attack.state.phase,
            AttackPhase::Windup
        );
        assert_eq!(
            hero(&server.world, &1).unwrap().role.last_action_seq,
            Some(6)
        );
        assert!(
            hero(&server.world, &1)
                .unwrap()
                .role
                .pending_actions
                .is_empty()
        );
    }

    #[test]
    fn boundary_wrap_and_invalid_dependency_are_handled() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        sim.queue_command(
            1,
            ClientCommand::Move {
                seq: u32::MAX,
                dir: [0.0, 0.0],
            },
        );
        sim.step();
        sim.queue_action(
            1,
            ClientAction {
                view_tick: None,
                match_epoch: 0,
                command: ClientCommand::BasicAttack { seq: 0 },
                after_move_seq: Some(u32::MAX),
            },
        );
        sim.step();
        assert_eq!(hero(&sim.world, &1).unwrap().role.last_action_seq, Some(0));
        sim.queue_action(
            1,
            ClientAction {
                view_tick: None,
                match_epoch: 0,
                command: ClientCommand::BasicAttack { seq: 1 },
                after_move_seq: Some(2),
            },
        );
        assert!(
            hero(&sim.world, &1)
                .unwrap()
                .role
                .pending_actions
                .is_empty()
        );
    }
}

#[cfg(test)]
mod round_input_tests {
    use super::*;
    #[test]
    fn old_round_actions_cannot_execute_after_reset() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        let action = game_shared::ClientAction {
            view_tick: None,
            match_epoch: 0,
            command: ClientCommand::BasicAttack { seq: 2 },
            after_move_seq: Some(1),
        };
        sim.queue_action(1, action);
        sim.reset_match_state();
        sim.queue_action(1, action);
        assert!(
            hero(&sim.world, &1)
                .unwrap()
                .role
                .pending_actions
                .is_empty()
        );
        assert_eq!(sim.sim_meta().match_epoch, 1);
        sim.queue_action(
            1,
            game_shared::ClientAction {
                view_tick: None,
                match_epoch: 1,
                after_move_seq: None,
                ..action
            },
        );
        sim.step();
        assert_eq!(hero(&sim.world, &1).unwrap().role.last_attack_seq, Some(2));
    }
    #[test]
    fn deferred_actions_are_bounded_and_removed_with_player() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        for seq in 2..200 {
            sim.queue_action(
                1,
                game_shared::ClientAction {
                    view_tick: None,
                    match_epoch: 0,
                    command: ClientCommand::BasicAttack { seq },
                    after_move_seq: Some(1),
                },
            );
        }
        assert_eq!(hero(&sim.world, &1).unwrap().role.pending_actions.len(), 64);
        sim.remove_player(1);
        sim.add_player(1);
        assert!(
            hero(&sim.world, &1)
                .unwrap()
                .role
                .pending_actions
                .is_empty()
        );
    }
}

#[cfg(test)]
mod presentation_contract_tests {
    use super::*;
    #[test]
    fn ability_state_survives_restore_and_replay_without_duplicate_identity() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        let before = sim.world_delta();
        let command = ClientCommand::CastAbility {
            seq: 7,
            ability: AbilityId::ArcBurst,
        };
        sim.queue_command(1, command);
        sim.step();
        let predicted = sim.world_delta();
        assert_eq!(predicted.presentations.len(), 1);
        let instance = predicted.presentations[0];
        assert_eq!(instance.id.action_seq, 7);
        assert_eq!(instance.age_ticks, 0);
        let mut replay = Simulation::from_snapshot(&before, &before.sim_meta.unwrap());
        replay.queue_command(1, command);
        replay.step();
        assert_eq!(replay.world_delta().presentations, predicted.presentations);
        let mut confirmed = Simulation::from_snapshot(&predicted, &predicted.sim_meta.unwrap());
        confirmed.queue_command(1, command);
        confirmed.step();
        let state = confirmed.world_delta();
        assert_eq!(state.presentations.len(), 1);
        assert_eq!(state.presentations[0].id, instance.id);
        assert_eq!(state.presentations[0].age_ticks, 1);
        for _ in 0..instance.duration_ticks {
            confirmed.step();
        }
        assert!(confirmed.world_delta().presentations.is_empty());
        sim.reset_match_state();
        assert!(sim.world_delta().presentations.is_empty());
    }

    #[test]
    fn rejected_ability_does_not_create_presentation_state() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        hero_mut(&mut sim.world, &1).unwrap().role.mana = 0.0;
        sim.queue_command(
            1,
            ClientCommand::CastAbility {
                seq: 1,
                ability: AbilityId::ArcBurst,
            },
        );
        sim.step();
        assert!(sim.world_delta().presentations.is_empty());
    }
}

#[cfg(test)]
mod historical_ability_tests {
    use super::*;
    #[test]
    fn history_changes_hit_geometry_but_not_origin_or_ability_validation() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        // Let the real spawner create an enemy, then put it beyond current range.
        for _ in 0..1000 {
            sim.step();
            if !actor_ids::<EnemyState>(&sim.world).is_empty() {
                break;
            }
        }
        let id = actor_ids::<EnemyState>(&sim.world)[0];
        let origin = hero(&sim.world, &1).unwrap().position.pos;
        enemy_mut(&mut sim.world, &id).unwrap().position.pos = [origin[0] + 20.0, origin[1]];
        enemy_mut(&mut sim.world, &id).unwrap().health.hp = 1000.0;
        sim.set_historical_targets(1, 1, vec![(id, origin)]);
        sim.queue_command(
            1,
            ClientCommand::CastAbility {
                seq: 1,
                ability: AbilityId::ArcBurst,
            },
        );
        sim.step();
        assert_eq!(
            enemy(&sim.world, &id).unwrap().health.hp,
            1000.0 - HERO_ABILITY_DAMAGE
        );
        let hp = enemy(&sim.world, &id).unwrap().health.hp;
        sim.set_historical_targets(1, 1, vec![(id, origin)]);
        sim.queue_command(
            1,
            ClientCommand::CastAbility {
                seq: 1,
                ability: AbilityId::ArcBurst,
            },
        );
        sim.step();
        assert_eq!(enemy(&sim.world, &id).unwrap().health.hp, hp); // duplicate cannot damage twice
        sim.set_historical_targets(1, 2, vec![(id, origin)]);
        sim.queue_command(
            1,
            ClientCommand::CastAbility {
                seq: 2,
                ability: AbilityId::ArcBurst,
            },
        );
        sim.step();
        assert_eq!(enemy(&sim.world, &id).unwrap().health.hp, hp); // history cannot bypass cooldown
        assert!(sim.world.resource::<HistoricalTargets>().0.is_empty());
    }
}

#[cfg(test)]
mod snapshot_navigation_tests {
    use super::*;

    #[test]
    fn snapshot_restore_reuses_navigation_until_obstacles_change() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        let original = sim.navmesh_for_tick().get();
        let mut delta = sim.world_delta();
        let meta = sim.sim_meta();
        for tick in 1..=30 {
            delta.tick = tick;
            delta.heroes[0].pos[0] += 0.1;
            sim.apply_snapshot(&delta, &meta);
            assert!(std::sync::Arc::ptr_eq(
                &original,
                &sim.navmesh_for_tick().get()
            ));
        }
        delta.towers.push(TowerSnapshot {
            id: 100,
            owner: 1,
            lane: 0,
            node_id: 0,
            pos: [-5.0, 0.0],
            reload_ticks_remaining: 0,
        });
        sim.apply_snapshot(&delta, &meta);
        let with_tower = sim.navmesh_for_tick().get();
        assert!(!std::sync::Arc::ptr_eq(&original, &with_tower));
        delta.towers[0].reload_ticks_remaining = 10;
        sim.apply_snapshot(&delta, &meta);
        assert!(std::sync::Arc::ptr_eq(
            &with_tower,
            &sim.navmesh_for_tick().get()
        ));
        delta.towers[0].pos[0] += 1.0;
        sim.apply_snapshot(&delta, &meta);
        let moved = sim.navmesh_for_tick().get();
        assert!(!std::sync::Arc::ptr_eq(&with_tower, &moved));
        delta.towers.clear();
        sim.apply_snapshot(&delta, &meta);
        assert!(!std::sync::Arc::ptr_eq(
            &moved,
            &sim.navmesh_for_tick().get()
        ));
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn recovered_burst_retires_stale_time_and_action_executes_once() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        let start = hero(&sim.world, &1).unwrap().position.pos;
        let action = game_shared::ClientAction {
            match_epoch: 0,
            view_tick: None,
            after_move_seq: Some(9),
            command: ClientCommand::BasicAttack { seq: 10 },
        };
        sim.queue_action(1, action);
        for seq in 1..=9 {
            sim.queue_command(
                1,
                ClientCommand::Move {
                    seq,
                    dir: [1.0, 0.0],
                },
            );
        }
        sim.queue_command(
            1,
            ClientCommand::Move {
                seq: 11,
                dir: [0.0; 2],
            },
        );
        sim.step();
        assert_eq!(
            hero(&sim.world, &1).unwrap().role.last_processed_seq,
            Some(8)
        );
        assert_eq!(hero(&sim.world, &1).unwrap().role.last_action_seq, None);
        assert!(
            hero(&sim.world, &1).unwrap().role.pending_moves.len()
                < game_shared::MAX_MOVEMENT_BACKLOG
        );
        sim.step();
        assert_eq!(
            hero(&sim.world, &1).unwrap().role.last_processed_seq,
            Some(9)
        );
        sim.step();
        assert_eq!(hero(&sim.world, &1).unwrap().role.last_attack_seq, Some(10));
        assert_eq!(
            hero(&sim.world, &1).unwrap().role.last_processed_seq,
            Some(11)
        );
        assert!(
            (hero(&sim.world, &1).unwrap().position.pos[0]
                - start[0]
                - 2.0 * HERO_SPEED * FIXED_DT_SECONDS)
                .abs()
                < 0.0001
        );
        for seq in 12..80 {
            sim.queue_action(1, action);
            sim.queue_command(1, ClientCommand::Move { seq, dir: [0.0; 2] });
            sim.step();
            assert!(
                hero(&sim.world, &1).unwrap().role.pending_moves.is_empty(),
                "backlog must not persist after recovery"
            );
        }
        assert_eq!(
            hero(&sim.world, &1).unwrap().attack.state.phase,
            AttackPhase::Ready
        );
    }
    #[test]
    fn respawn_generation_survives_snapshot_restore() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        sim.apply_hero_damage(1, HERO_MAX_HP);
        assert_eq!(hero(&sim.world, &1).unwrap().role.respawn_generation, 1);
        let restored = Simulation::from_snapshot(&sim.world_delta(), &sim.sim_meta());
        assert_eq!(
            hero(&restored.world, &1).unwrap().role.respawn_generation,
            1
        );
        assert_eq!(restored.world_delta(), sim.world_delta());
    }
}

impl Simulation {
    fn reset_match_state(&mut self) {
        self.world
            .resource_scope(|world, mut g: Mut<Globals>| g.reset_match_state(world));
    }
    fn schedule_match_reset(&mut self) {
        self.world
            .resource_scope(|world, mut g: Mut<Globals>| g.schedule_match_reset(world));
    }
    fn match_restart_ticks_remaining(&self) -> Option<u32> {
        self.world
            .resource::<Globals>()
            .match_restart_ticks_remaining(&self.world)
    }
    fn spawn_enemy(&mut self, point: usize, wave: u32) {
        self.world
            .resource_scope(|world, mut g: Mut<Globals>| g.spawn_enemy(world, point, wave));
    }
    fn navmesh_for_tick(&mut self) -> NavMesh {
        self.world
            .resource_scope(|world, mut g: Mut<Globals>| g.navmesh_for_tick(world))
    }
    fn apply_hero_damage(&mut self, id: u64, damage: f32) {
        self.world
            .resource_scope(|world, mut g: Mut<Globals>| g.apply_hero_damage(world, id, damage));
    }
}

mod ecs_restore_tests {
    use super::*;
    use game_replication::{NetId, entity};

    #[derive(Component)]
    struct UnrelatedEntity;
    #[derive(Resource)]
    struct UnrelatedResource(u32);

    #[test]
    fn match_reset_rebinds_heroes_to_new_epoch_and_preserves_unrelated_state() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        sim.spawn_enemy(0, 1);
        let epoch = sim.sim_meta().match_epoch;
        let old_hero = NetId::new(epoch, 0, 1);
        let old_enemy = NetId::new(epoch, 1, 1_000_000);
        let unrelated_id = NetId::new(epoch, 9, 7);
        let unrelated =
            game_replication::spawn(&mut sim.world, unrelated_id, UnrelatedEntity).unwrap();
        sim.world.insert_resource(UnrelatedResource(42));
        sim.reset_match_state();
        assert_eq!(sim.sim_meta().match_epoch, epoch.wrapping_add(1));
        assert_eq!(entity(&sim.world, old_hero), None);
        assert_eq!(entity(&sim.world, old_enemy), None);
        assert!(entity(&sim.world, NetId::new(epoch.wrapping_add(1), 0, 1)).is_some());
        assert_eq!(entity(&sim.world, unrelated_id), Some(unrelated));
        assert_eq!(sim.world.resource::<UnrelatedResource>().0, 42);
        sim.queue_command(
            1,
            ClientCommand::Move {
                seq: 1,
                dir: [1.0, 0.0],
            },
        );
        sim.step();
        assert_eq!(sim.world_delta().heroes[0].last_move_seq, Some(1));
    }

    #[test]
    fn different_local_allocations_replay_the_same_snapshot_and_commands() {
        let mut source = Simulation::new();
        source.add_player(1);
        source.add_player(2);
        for point in 0..4 {
            source.spawn_enemy(point % ENEMY_SPAWN_POINTS.len(), 1);
        }
        let snapshot = source.world_delta();
        let meta = source.sim_meta();
        let mut restored = Simulation::new();
        for _ in 0..11 {
            restored.world.spawn(UnrelatedEntity);
        }
        restored.world.insert_resource(UnrelatedResource(42));
        restored.apply_snapshot(&snapshot, &meta);
        let id = NetId::new(meta.match_epoch, 0, 1);
        assert_ne!(entity(&source.world, id), entity(&restored.world, id));
        assert_eq!(source.world_delta(), restored.world_delta());
        for tick in 1..=60 {
            for client in [1, 2] {
                let command = ClientCommand::Move {
                    seq: tick * 2,
                    dir: if client == 1 { [1.0, 0.0] } else { [-1.0, 0.0] },
                };
                source.queue_command(client, command);
                restored.queue_command(client, command);
                if tick % 10 == 0 {
                    let attack = ClientCommand::BasicAttack { seq: tick * 2 + 1 };
                    source.queue_command(client, attack);
                    restored.queue_command(client, attack);
                }
            }
            assert_eq!(
                source.step().reliable_events,
                restored.step().reliable_events,
                "events at {tick}"
            );
            assert_eq!(
                source.world_delta(),
                restored.world_delta(),
                "state at {tick}"
            );
        }
        assert_eq!(restored.world.resource::<UnrelatedResource>().0, 42);
        assert_eq!(
            restored
                .world
                .query::<&UnrelatedEntity>()
                .iter(&restored.world)
                .count(),
            11
        );
    }

    #[test]
    fn restore_upserts_survivors_removes_absent_and_cleans_old_epochs_only_for_simulation() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        sim.add_player(1_000_000);
        sim.spawn_enemy(0, 1);
        let mut snapshot = sim.world_delta();
        snapshot.towers.push(TowerSnapshot {
            id: 1_000_000,
            owner: 1,
            lane: 0,
            node_id: BUILD_NODES[0].node_id,
            pos: BUILD_NODES[0].pos,
            reload_ticks_remaining: 0,
        });
        let mut meta = sim.sim_meta();
        sim.apply_snapshot(&snapshot, &meta);
        let hero = NetId::new(meta.match_epoch, 0, 1_000_000);
        let enemy = NetId::new(meta.match_epoch, 1, 1_000_000);
        let tower = NetId::new(meta.match_epoch, 2, 1_000_000);
        let handles = [hero, enemy, tower].map(|id| entity(&sim.world, id).unwrap());
        let unrelated_id = NetId::new(meta.match_epoch, 9, 99);
        let unrelated =
            game_replication::spawn(&mut sim.world, unrelated_id, UnrelatedEntity).unwrap();
        sim.world.insert_resource(UnrelatedResource(7));
        snapshot.heroes[0].pos[0] += 0.25;
        sim.apply_snapshot(&snapshot, &meta);
        assert_eq!(
            [hero, enemy, tower].map(|id| entity(&sim.world, id).unwrap()),
            handles
        );

        snapshot.heroes.retain(|h| h.client_id != hero.value);
        sim.apply_snapshot(&snapshot, &meta);
        assert_eq!(entity(&sim.world, hero), None);
        assert!(sim.world.get_entity(handles[0]).is_err());
        assert_eq!(entity(&sim.world, enemy), Some(handles[1]));
        assert_eq!(entity(&sim.world, tower), Some(handles[2]));

        meta.match_epoch += 1;
        snapshot.sim_meta = Some(meta);
        sim.apply_snapshot(&snapshot, &meta);
        assert_eq!(entity(&sim.world, enemy), None);
        assert_eq!(entity(&sim.world, tower), None);
        assert!(sim.world.get_entity(handles[1]).is_err());
        assert!(sim.world.get_entity(handles[2]).is_err());
        assert!(entity(&sim.world, NetId::new(meta.match_epoch, 1, enemy.value)).is_some());
        assert_eq!(entity(&sim.world, unrelated_id), Some(unrelated));
        assert_eq!(sim.world.resource::<UnrelatedResource>().0, 7);
        assert_eq!(sim.world_delta().heroes, snapshot.heroes);
        assert_eq!(sim.world_delta().enemies, snapshot.enemies);
        assert_eq!(sim.world_delta().towers, snapshot.towers);
    }
}

mod ecs_presentation_tests {
    use super::*;

    fn presentation_entity(sim: &mut Simulation, id: game_shared::PresentationId) -> Entity {
        sim.world
            .query::<(Entity, &Presentation)>()
            .iter(&sim.world)
            .find_map(|(entity, presentation)| (presentation.instance.id == id).then_some(entity))
            .expect("presentation entity exists")
    }

    #[test]
    fn presentation_entities_preserve_full_identity_order_and_replay_lifecycle() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        sim.add_player(2);
        let before = sim.world_delta();
        let command = ClientCommand::CastAbility {
            seq: 7,
            ability: AbilityId::ArcBurst,
        };
        for owner in [1, 2] {
            sim.queue_command(owner, command);
        }
        sim.step();
        let predicted = sim.world_delta();
        assert_eq!(predicted.presentations.len(), 2);
        let ids = [predicted.presentations[0].id, predicted.presentations[1].id];
        assert_eq!(ids[0].action_seq, ids[1].action_seq);
        assert_ne!(ids[0].owner, ids[1].owner);
        let handles = ids.map(|id| presentation_entity(&mut sim, id));
        assert_ne!(handles[0], handles[1]);

        let mut reordered = predicted.clone();
        reordered.presentations.reverse();
        sim.apply_snapshot(&reordered, &reordered.sim_meta.unwrap());
        assert_eq!(ids.map(|id| presentation_entity(&mut sim, id)), handles);
        assert_eq!(sim.world_delta().presentations, reordered.presentations);
        assert_eq!(sim.world.get::<Presentation>(handles[1]).unwrap().order, 0);
        assert_eq!(sim.world.get::<Presentation>(handles[0]).unwrap().order, 1);

        // Rejection removes the speculative instances; replay restores their wire
        // identities even though local entity generations have changed.
        sim.apply_snapshot(&before, &before.sim_meta.unwrap());
        assert!(sim.presentations().is_empty());
        assert!(
            handles
                .into_iter()
                .all(|entity| sim.world.get_entity(entity).is_err())
        );
        for owner in [1, 2] {
            sim.queue_command(owner, command);
        }
        sim.step();
        assert_eq!(sim.world_delta().presentations, predicted.presentations);
        let replay_handles = ids.map(|id| presentation_entity(&mut sim, id));
        assert_ne!(handles, replay_handles);
        sim.apply_snapshot(&predicted, &predicted.sim_meta.unwrap());
        assert_eq!(
            ids.map(|id| presentation_entity(&mut sim, id)),
            replay_handles
        );
        for owner in [1, 2] {
            sim.queue_command(owner, command);
        }
        sim.step();
        assert_eq!(sim.presentations().len(), 2);
        assert_eq!(
            ids.map(|id| presentation_entity(&mut sim, id)),
            replay_handles
        );
        for _ in 0..predicted.presentations[0].duration_ticks {
            sim.step();
        }
        assert!(sim.presentations().is_empty());
        assert!(
            replay_handles
                .into_iter()
                .all(|entity| sim.world.get_entity(entity).is_err())
        );

        sim.apply_snapshot(&predicted, &predicted.sim_meta.unwrap());
        let reset_handles = ids.map(|id| presentation_entity(&mut sim, id));
        sim.reset_match_state();
        assert!(sim.presentations().is_empty());
        assert!(
            reset_handles
                .into_iter()
                .all(|entity| sim.world.get_entity(entity).is_err())
        );
    }
}
