use super::*;
#[test]
fn experience_uses_game_curve_and_rejects_invalid_curve_atomically() {
    let mut progress = Progression {
        level: 1,
        xp: 0.0,
        xp_next: 10.0,
        pending_levels: 0,
    };
    assert_eq!(progress.gain(35.0, |level| level as f32 * 10.0), Ok(2));
    assert_eq!(
        (progress.level, progress.xp, progress.pending_levels),
        (3, 5.0, 2)
    );
    let previous = progress;
    assert_eq!(
        progress.gain(100.0, |_| 0.0),
        Err(ProgressionError::InvalidThreshold)
    );
    assert_eq!(progress, previous);
}
#[test]
fn rank_purchase_is_atomic_at_caps_and_insufficient_funds() {
    let (mut balance, mut rank) = (10, 1);
    assert!(!purchase_rank(&mut balance, &mut rank, 11, 2));
    assert_eq!((balance, rank), (10, 1));
    assert!(purchase_rank(&mut balance, &mut rank, 4, 2));
    assert!(!purchase_rank(&mut balance, &mut rank, 4, 2));
    assert_eq!((balance, rank), (6, 2));
}
#[test]
fn queued_grants_follow_game_curve_and_restore() {
    use bevy::prelude::*;
    let state = Progression {
        level: 1,
        xp: 0.0,
        xp_next: 10.0,
        pending_levels: 0,
    };
    let saved = serde_json::to_string(&(state, PendingExperience(35.0))).unwrap();
    let restored: (Progression, PendingExperience) = serde_json::from_str(&saved).unwrap();
    let mut app = App::new();
    app.add_plugins(ProgressionPlugin::new(Update, |level| level as f32 * 10.0));
    let first = app.world_mut().spawn((state, PendingExperience(35.0))).id();
    let second = app.world_mut().spawn(restored).id();
    app.update();
    assert_eq!(
        app.world().get::<Progression>(first),
        app.world().get::<Progression>(second)
    );
    assert_eq!(app.world().get::<Progression>(first).unwrap().level, 3);
    assert!(app.world().get::<PendingExperience>(first).is_none());
    app.world_mut().clear_trackers();
    app.update();
    assert!(
        !app.world()
            .entity(first)
            .get_ref::<Progression>()
            .unwrap()
            .is_changed()
    );
    app.world_mut()
        .entity_mut(first)
        .insert(PendingExperience(-1.0));
    app.update();
    assert_eq!(
        app.world().get::<ExperienceResult>(first).unwrap().0,
        Err(ProgressionError::InvalidExperience)
    );
}
