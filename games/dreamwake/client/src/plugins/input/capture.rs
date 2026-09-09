use super::*;
use crate::plugins::{
    diagnostics::Playtest,
    network,
    ui::{UiAction, UiActions},
};
use crate::{DreamConnection, DreamPreferences, DreamView};
use bevy::window::PrimaryWindow;
use dreamwake_sim::{DreamSnapshot, RunPhase};
use engine_client::camera::CameraRig as DreamCameraRig;
pub(super) fn keyboard_actions(
    keys: Res<ButtonInput<KeyCode>>,
    view: Res<DreamView>,
    prefs: Res<DreamPreferences>,
    connection: Res<DreamConnection>,
    mut actions: ResMut<UiActions>,
    gamepads: Query<&Gamepad>,
) {
    if keys.just_pressed(KeyCode::Escape) {
        actions.0.push(UiAction::TogglePause);
    }
    if keys.just_pressed(KeyCode::Tab) {
        actions.0.push(UiAction::ToggleBuild);
    }
    if keys.just_pressed(KeyCode::KeyM) {
        actions.0.push(UiAction::ToggleMute);
    }
    if keys.just_pressed(KeyCode::F5)
        && (prefs.paused || matches!(view.0.phase, RunPhase::Victory | RunPhase::Defeat))
    {
        actions.0.push(UiAction::Restart);
    }
    if keys.just_pressed(KeyCode::Enter) {
        if prefs.paused || prefs.build_open {
            actions.0.push(UiAction::TogglePause);
        } else {
            match view.0.phase {
                RunPhase::Intro if !connection.connected => actions.0.push(UiAction::Connect),
                RunPhase::Intro if !view.0.ready => {
                    actions.0.push(UiAction::Start { lucid: prefs.lucid })
                }
                RunPhase::Transition | RunPhase::Rest => actions.0.push(UiAction::Continue),
                RunPhase::Victory | RunPhase::Defeat => actions.0.push(UiAction::Restart),
                _ if prefs.paused => actions.0.push(UiAction::TogglePause),
                _ => {}
            }
        }
    }
    for (i, key) in [KeyCode::KeyZ, KeyCode::KeyX, KeyCode::KeyC, KeyCode::KeyV]
        .into_iter()
        .enumerate()
    {
        if keys.just_pressed(key) {
            actions.0.push(UiAction::SelectSlot(i));
        }
    }
    if matches!(view.0.phase, RunPhase::Reward | RunPhase::Rest)
        && !prefs.paused
        && !prefs.build_open
    {
        for (i, key) in [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3]
            .into_iter()
            .enumerate()
        {
            if keys.just_pressed(key) {
                actions.0.push(UiAction::Choose {
                    choice: i,
                    slot: prefs.selected_slot,
                });
            }
        }
    }
    for pad in &gamepads {
        if pad.just_pressed(GamepadButton::Start) {
            actions.0.push(UiAction::TogglePause);
        }
        if pad.just_pressed(GamepadButton::Select) {
            actions.0.push(UiAction::ToggleBuild);
        }
        if view.0.phase != RunPhase::Combat || prefs.build_open {
            if pad.just_pressed(GamepadButton::DPadLeft) {
                actions
                    .0
                    .push(UiAction::SelectSlot((prefs.selected_slot + 3) % 4));
            }
            if pad.just_pressed(GamepadButton::DPadRight) {
                actions
                    .0
                    .push(UiAction::SelectSlot((prefs.selected_slot + 1) % 4));
            }
            if !prefs.paused && !prefs.build_open {
                for (i, button) in [
                    GamepadButton::South,
                    GamepadButton::West,
                    GamepadButton::North,
                ]
                .into_iter()
                .enumerate()
                {
                    if pad.just_pressed(button) {
                        match view.0.phase {
                            RunPhase::Reward | RunPhase::Rest => actions.0.push(UiAction::Choose {
                                choice: i,
                                slot: prefs.selected_slot,
                            }),
                            RunPhase::Intro if !connection.connected => {
                                actions.0.push(UiAction::Connect)
                            }
                            RunPhase::Intro if !view.0.ready => {
                                actions.0.push(UiAction::Start { lucid: false })
                            }
                            RunPhase::Transition => actions.0.push(UiAction::Continue),
                            RunPhase::Victory | RunPhase::Defeat => {
                                actions.0.push(UiAction::Restart)
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn apply_ui_actions(world: &mut World) {
    use dreamwake_protocol::DreamAction;
    let actions = std::mem::take(&mut world.resource_mut::<UiActions>().0);
    if actions.is_empty() {
        return;
    }
    for action in actions {
        match action {
            UiAction::Host => network::host(world),
            UiAction::Connect => network::connect(world, 1),
            UiAction::Disconnect => network::disconnect(world),
            UiAction::Start { lucid } => {
                world.resource_mut::<DreamPreferences>().lucid = lucid;
                network::send_action(world, DreamAction::Start { lucid });
            }
            UiAction::Choose { choice, slot } => network::send_action(
                world,
                DreamAction::Choose {
                    choice: choice as u8,
                    slot: slot as u8,
                },
            ),
            UiAction::Continue => network::send_action(world, DreamAction::Continue),
            UiAction::Restart => {
                network::send_action(world, DreamAction::Restart);
                let mut prefs = world.resource_mut::<DreamPreferences>();
                prefs.paused = false;
                prefs.build_open = false;
            }
            UiAction::TogglePause | UiAction::ToggleBuild => {
                let mut prefs = world.resource_mut::<DreamPreferences>();
                if matches!(action, UiAction::TogglePause) {
                    if prefs.build_open {
                        prefs.build_open = false;
                    } else {
                        prefs.paused = !prefs.paused;
                    }
                } else {
                    prefs.build_open = !prefs.build_open;
                    prefs.paused = false;
                }
                let paused = prefs.paused || prefs.build_open;
                let connection = world.resource::<DreamConnection>();
                if connection.party_size == 1 {
                    network::send_action(world, DreamAction::Pause { paused });
                }
            }
            UiAction::ToggleMute => {
                let mut prefs = world.resource_mut::<DreamPreferences>();
                prefs.muted = !prefs.muted;
            }
            UiAction::SelectSlot(slot) => {
                world.resource_mut::<DreamPreferences>().selected_slot = slot.min(3)
            }
            UiAction::Swap(a, b) => network::send_action(
                world,
                DreamAction::Swap {
                    a: a as u8,
                    b: b as u8,
                },
            ),
            UiAction::BuyMemoryUpgrade(slot) => {
                network::send_action(world, DreamAction::BuyMemoryUpgrade { slot: slot as u8 })
            }
        }
    }
    world.resource_mut::<CapturedInput>().0 = DreamInput::default();
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn capture_input(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<DreamCameraRig>>,
    buttons: Query<&Interaction, With<Node>>,
    gamepads: Query<&Gamepad>,
    view: Res<DreamView>,
    prefs: Res<DreamPreferences>,
    mut captured: ResMut<CapturedInput>,
    playtest: Res<Playtest>,
) {
    if prefs.paused || prefs.build_open || view.0.phase != RunPhase::Combat {
        captured.0 = DreamInput::default();
        return;
    }
    if playtest.autoplay {
        captured.0 = autopilot(&view.0);
        return;
    }
    let axis =
        |positive, negative| f32::from(keys.pressed(positive)) - f32::from(keys.pressed(negative));
    let mut movement = Vec2::new(
        axis(KeyCode::KeyD, KeyCode::KeyA),
        axis(KeyCode::KeyS, KeyCode::KeyW),
    );
    let mut aim = Vec2::from_array(view.0.hero.facing);
    // Cursor projection already produces canonical world coordinates.
    if let (Ok(window), Ok((camera, transform))) = (windows.single(), cameras.single()) {
        if let Some(cursor) = window.cursor_position() {
            if let Some(point) = engine_client::camera::cursor_on_ground(camera, transform, cursor)
            {
                aim = (point - Vec2::from_array(view.0.hero.position)).normalize_or_zero();
            }
        }
    }
    let over_ui = buttons
        .iter()
        .any(|interaction| *interaction != Interaction::None);
    captured.0.attack = mouse.pressed(MouseButton::Left) && !over_ui;
    captured.0.dash |= keys.just_pressed(KeyCode::Space);
    for (i, key) in [KeyCode::KeyQ, KeyCode::KeyE, KeyCode::KeyR, KeyCode::KeyF]
        .into_iter()
        .enumerate()
    {
        captured.0.casts[i] |= keys.just_pressed(key);
    }
    for pad in &gamepads {
        let left = Vec2::new(
            pad.get(GamepadAxis::LeftStickX).unwrap_or_default(),
            -pad.get(GamepadAxis::LeftStickY).unwrap_or_default(),
        );
        if left.length() > 0.18 {
            movement = left;
        }
        let right = Vec2::new(
            pad.get(GamepadAxis::RightStickX).unwrap_or_default(),
            -pad.get(GamepadAxis::RightStickY).unwrap_or_default(),
        );
        if right.length() > 0.22 {
            aim = cameras
                .single()
                .map_or(right.normalize(), |(_, transform)| {
                    engine_client::camera::screen_axes_to_world(transform, right.normalize())
                });
        }
        captured.0.attack |= pad.pressed(GamepadButton::RightTrigger2);
        captured.0.dash |= pad.just_pressed(GamepadButton::LeftTrigger);
        for (i, button) in [
            GamepadButton::South,
            GamepadButton::West,
            GamepadButton::North,
            GamepadButton::East,
        ]
        .into_iter()
        .enumerate()
        {
            captured.0.casts[i] |= pad.just_pressed(button);
        }
    }
    let movement = movement.clamp_length_max(1.0);
    captured.0.movement = cameras
        .single()
        .map_or(movement, |(_, transform)| {
            engine_client::camera::screen_axes_to_world(transform, movement)
        })
        .to_array();
    captured.0.aim = aim.to_array();
}

fn autopilot(view: &DreamSnapshot) -> DreamInput {
    let p = Vec2::from_array(view.hero.position);
    let closest = view.enemies.iter().min_by(|a, b| {
        Vec2::from_array(a.position)
            .distance_squared(p)
            .total_cmp(&Vec2::from_array(b.position).distance_squared(p))
    });
    let Some(enemy) = closest else {
        return DreamInput::default();
    };
    let offset = Vec2::from_array(enemy.position) - p;
    let aim = offset.normalize_or_zero();
    let mut movement = if offset.length() > 2.3 {
        aim
    } else {
        Vec2::new(-aim.y, aim.x) * 0.8
    };
    let mut danger = false;
    for e in &view.enemies {
        let away = p - Vec2::from_array(e.target);
        if e.windup > 0.0 && away.length() < e.warn_radius + 1.6 {
            movement += away.try_normalize().unwrap_or(Vec2::new(-aim.y, aim.x)) * 3.0;
            danger = true;
        }
    }
    for shot in view.projectiles.iter().filter(|s| !s.friendly) {
        let away = p - Vec2::from_array(shot.position);
        if away.length() < 2.0 {
            movement += away.normalize_or_zero() * 2.0;
        }
    }
    if p.length() > 15.0 {
        movement -= p.normalize() * 2.0;
    }
    DreamInput {
        movement: movement.normalize_or_zero().to_array(),
        aim: aim.to_array(),
        attack: true,
        dash: danger && view.hero.dash_cooldown <= 0.0,
        casts: std::array::from_fn(|i| view.hero.memories[i].ready()),
        ..Default::default()
    }
}
