//! Presentation reads the same reconciled simulation as the local actor.
use super::*;

pub(super) fn presentation_hero(
    local: &LocalSimulation,
    world: &WorldView,
    client_id: Option<u64>,
) -> Option<HeroSnapshot> {
    let id = client_id?;
    if local.initialized {
        // A missing predicted actor must not be resurrected from an older snapshot.
        local
            .sim
            .world_delta()
            .heroes
            .into_iter()
            .find(|h| h.client_id == id)
    } else {
        world.heroes.get(&id).cloned()
    }
}

/// Enemy health is currently monotonic within an enemy lifetime. Keep the lowest
/// presented health across rollback, so replay and server confirmation do not
/// emit the same impact again. This is presentation-only: never clamp simulated HP.
/// If enemies gain healing, replace this watermark with explicit damage event IDs.
#[derive(Default)]
pub(super) struct EnemyHitFeedback {
    lowest_hp: HashMap<u64, f32>,
}

impl EnemyHitFeedback {
    pub(super) fn forget(&mut self, id: u64) {
        self.lowest_hp.remove(&id);
    }
    /// Establish a baseline without consuming damage discovered by rollback/replay.
    pub(super) fn seed(&mut self, id: u64, hp: f32) {
        self.lowest_hp.entry(id).or_insert(hp);
    }

    pub(super) fn observe(&mut self, id: u64, hp: f32) -> bool {
        let lowest = self.lowest_hp.entry(id).or_insert(hp);
        let hit = hp < *lowest;
        *lowest = lowest.min(hp);
        hit
    }

    pub(super) fn retain(&mut self, enemies: &[EnemySnapshot]) {
        self.lowest_hp
            .retain(|id, _| enemies.iter().any(|e| e.id == *id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impact_feedback_survives_rollback_and_confirmation() {
        let mut feedback = EnemyHitFeedback::default();
        assert!(!feedback.observe(1, 100.0));
        feedback.seed(1, 80.0); // replay discovered a hit before the next forward step
        assert!(feedback.observe(1, 80.0));
        assert!(!feedback.observe(1, 100.0)); // rollback before the attack
        assert!(!feedback.observe(1, 80.0)); // replay / confirmation
        assert!(feedback.observe(1, 60.0)); // a subsequent hit
        assert!(feedback.observe(1, 0.0)); // lethal hit
        assert!(!feedback.observe(1, 0.0));
        feedback.retain(&[]);
        assert!(!feedback.observe(1, 100.0));
    }

    #[test]
    fn hud_uses_prediction_until_authoritative_reconciliation() {
        let mut local = LocalSimulation::default();
        local.sim.add_player(1);
        local.initialized = true;
        let world = WorldView::default(); // No new network frame is required.
        let mut snapshot = local.sim.world_delta();
        let meta = snapshot.sim_meta.unwrap();
        let hero = &mut snapshot.heroes[0];
        hero.charge_profile.startup_ticks = 0;
        hero.charge_profile.power_gain_per_tick = hero.charge_profile.power_meter_max;
        hero.mana = HERO_MAX_MANA;
        hero.hp = HERO_MAX_HP;
        local.sim.apply_snapshot(&snapshot, &meta);
        local.sim.queue_command(
            1,
            ClientCommand::SetCharging {
                seq: 1,
                active: true,
            },
        );
        for _ in 0..3 {
            local.sim.step();
        }
        assert!(
            presentation_hero(&local, &world, Some(1))
                .unwrap()
                .charge_state
                .power_active
        );
        let predicted = local.sim.world_delta().heroes.remove(0);
        let displayed = presentation_hero(&local, &world, Some(1)).unwrap();
        assert_eq!(displayed.charge_state, predicted.charge_state);
        assert_eq!(
            displayed.ability_cooldown_ticks,
            predicted.ability_cooldown_ticks
        );
        assert!(presentation_hero(&local, &world, Some(2)).is_none());
    }
}

/// The pie exclusively represents power-up charge, including during startup
/// and mana refill. Those mechanics must never substitute their own progress.
pub(super) fn charge_presentation(hero: Option<&HeroSnapshot>) -> (f32, bool) {
    let Some(hero) = hero else {
        return (0.0, false);
    };
    let power = (hero.charge_state.power_meter
        / hero.charge_profile.power_meter_max.max(f32::EPSILON))
    .clamp(0.0, 1.0);
    (power, hero.charge_state.power_active)
}

#[cfg(test)]
mod charge_stage_tests {
    use super::*;
    #[test]
    fn startup_and_mana_do_not_masquerade_as_power() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        let mut delta = sim.world_delta();
        delta.heroes[0].mana = 0.0;
        sim.apply_snapshot(&delta, &delta.sim_meta.unwrap());
        sim.queue_command(
            1,
            ClientCommand::SetCharging {
                seq: 1,
                active: true,
            },
        );
        sim.step();
        let hero = sim.world_delta().heroes.remove(0);
        let (ratio, ready) = charge_presentation(Some(&hero));
        assert_eq!(hero.charge_state.phase, ChargePhase::Startup);
        assert_eq!(ratio, 0.0);
        assert!(!ready);
        for _ in 0..hero.charge_profile.startup_ticks {
            sim.step();
        }
        let hero = sim.world_delta().heroes.remove(0);
        let (ratio, ready) = charge_presentation(Some(&hero));
        assert_eq!(hero.charge_state.phase, ChargePhase::Charging);
        assert_eq!(ratio, 0.0);
        assert_eq!(hero.charge_state.power_meter, 0.0);
        assert!(!ready);
    }
}

#[cfg(test)]
mod charge_fill_regressions {
    use super::*;
    #[test]
    fn first_charge_fill_tracks_only_power_through_startup() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        sim.queue_command(
            1,
            ClientCommand::SetCharging {
                seq: 1,
                active: true,
            },
        );
        let mut previous = 0.0;
        for _ in 0..300 {
            sim.step();
            let hero = sim.world_delta().heroes.remove(0);
            let (ratio, ready) = charge_presentation(Some(&hero));
            assert_eq!(
                ratio,
                hero.charge_state.power_meter / hero.charge_profile.power_meter_max
            );
            assert!(
                ratio >= previous,
                "first charge must not flash full then reset"
            );
            if ready {
                return;
            }
            previous = ratio;
        }
        panic!("charge never completed");
    }
}

/// Cosmetic only; authority still owns the enemy lifetime.
#[derive(Component, Default)]
pub(super) struct PredictedEnemyDeath {
    age: f32,
}

pub(super) fn update_predicted_enemy_deaths(
    time: Res<Time>,
    mut enemies: Query<(&mut PredictedEnemyDeath, &mut Transform, &mut Visibility)>,
) {
    for (mut death, mut transform, mut visibility) in &mut enemies {
        death.age += time.delta_secs();
        let t = (death.age / 0.25).clamp(0.0, 1.0);
        // A tentative lethal reaction stays visible until confirmation. Keep
        // a subtle settling pulse while waiting, rather than freezing or hiding.
        let settle = if t >= 1.0 {
            0.025 * (death.age * 12.0).sin()
        } else {
            0.0
        };
        transform.scale = Vec3::new(
            1.0 + 0.2 * t + settle,
            1.0 - 0.65 * t - settle,
            1.0 + 0.2 * t + settle,
        );
        *visibility = Visibility::Inherited;
    }
}

#[cfg(test)]
mod death_animation_tests {
    use super::*;
    #[test]
    fn tentative_lethal_reaction_stays_visible_until_confirmation() {
        let mut app = App::new();
        let mut time = Time::<()>::default();
        time.advance_by(std::time::Duration::from_millis(100));
        app.insert_resource(time);
        app.add_systems(Update, update_predicted_enemy_deaths);
        let entity = app
            .world_mut()
            .spawn((
                PredictedEnemyDeath::default(),
                Transform::default(),
                Visibility::Inherited,
            ))
            .id();
        app.update();
        let first = app.world().get::<Transform>(entity).unwrap().scale;
        assert!(first.y < 1.0 && first.x > 1.0);
        assert_eq!(
            *app.world().get::<Visibility>(entity).unwrap(),
            Visibility::Inherited
        );
        app.update();
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(entity).unwrap(),
            Visibility::Inherited
        );
        assert!(app.world().get_entity(entity).is_ok());
    }
}
