use super::super::*;
use crate::SimulationStep;

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
        .add_plugins(CombatEffectsPlugin(Update));
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
