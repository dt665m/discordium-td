use super::super::*;
struct Mana;
struct Stamina;
#[test]
fn spending_and_capacity_are_atomic_and_bounded() {
    let mut meter = Meter::<Mana>::new(8.0, 10.0).unwrap();
    for invalid in [f32::NAN, f32::INFINITY, -1.0, 9.0] {
        assert!(!meter.try_spend(invalid));
        assert_eq!(meter.current(), 8.0);
    }
    assert!(meter.try_spend(3.0));
    assert_eq!(meter.restore(100.0), 5.0);
    assert_eq!(meter.restore(f32::INFINITY), 0.0);
    assert!(!meter.set_capacity(f32::NAN));
    assert!(meter.set_capacity(4.0));
    assert_eq!(meter.current(), 4.0);
    assert!(meter.set_capacity(20.0));
    assert_eq!(meter.current(), 4.0);
    assert!(Meter::<Mana>::new(2.0, 1.0).is_none());
    assert!(serde_json::from_str::<Meter<Mana>>(r#"{"current":2,"capacity":1}"#).is_err());
}
#[test]
fn regeneration_is_typed_replayable_and_idle_preserves_change_detection() {
    let mut app = App::new();
    app.add_plugins((
        crate::SystemPlugin::new(0.5),
        MeterPlugin::<Mana, _>::new(Update),
        MeterPlugin::<Stamina, _>::new(Update),
    ));
    let actor = app
        .world_mut()
        .spawn((
            Meter::<Mana>::new(0.0, 2.0).unwrap(),
            Regeneration::<Mana>::new(2.0),
            Meter::<Stamina>::new(1.0, 10.0).unwrap(),
        ))
        .id();
    app.update();
    let saved = serde_json::to_string(&(
        app.world().get::<Meter<Mana>>(actor).unwrap(),
        app.world().get::<Regeneration<Mana>>(actor).unwrap(),
    ))
    .unwrap();
    let restored_state: (Meter<Mana>, Regeneration<Mana>) = serde_json::from_str(&saved).unwrap();
    let restored = app.world_mut().spawn(restored_state).id();
    app.update();
    assert_eq!(
        app.world().get::<Meter<Mana>>(actor).unwrap().current(),
        app.world().get::<Meter<Mana>>(restored).unwrap().current()
    );
    assert_eq!(
        app.world().get::<Meter<Stamina>>(actor).unwrap().current(),
        1.0
    );
    app.world_mut().clear_trackers();
    app.update();
    assert!(
        !app.world()
            .entity(actor)
            .get_ref::<Meter<Mana>>()
            .unwrap()
            .is_changed()
    );
    app.world_mut()
        .entity_mut(actor)
        .get_mut::<Regeneration<Mana>>()
        .unwrap()
        .per_second = f32::NAN;
    app.update();
    assert_eq!(
        app.world().get::<Meter<Mana>>(actor).unwrap().current(),
        2.0
    );
}

#[test]
fn regeneration_caps_finite_overflow_and_zero_capacity() {
    let mut app = App::new();
    app.add_plugins((
        crate::SystemPlugin::new(2.0),
        MeterPlugin::<Mana, _>::new(Update),
    ));
    let full = app
        .world_mut()
        .spawn((
            Meter::<Mana>::new(1.0, 5.0).unwrap(),
            Regeneration::<Mana>::new(f32::MAX),
        ))
        .id();
    let empty = app
        .world_mut()
        .spawn((
            Meter::<Mana>::new(0.0, 0.0).unwrap(),
            Regeneration::<Mana>::new(f32::MAX),
        ))
        .id();
    app.update();
    assert_eq!(app.world().get::<Meter<Mana>>(full).unwrap().current(), 5.0);
    assert_eq!(
        app.world().get::<Meter<Mana>>(empty).unwrap().current(),
        0.0
    );
    app.world_mut().clear_trackers();
    app.update();
    assert!(
        !app.world()
            .entity(full)
            .get_ref::<Meter<Mana>>()
            .unwrap()
            .is_changed()
    );
    assert!(
        !app.world()
            .entity(empty)
            .get_ref::<Meter<Mana>>()
            .unwrap()
            .is_changed()
    );
}
