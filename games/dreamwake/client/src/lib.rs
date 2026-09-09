//! Dreamwake presentation host. The isolated shared simulation owns all gameplay.
mod audio;
mod network;
mod presentation;
pub mod scene;
mod typography;
pub mod ui;

use bevy::{input::InputSystems, prelude::*, window::PrimaryWindow};
use dreamwake_sim::{DreamInput, DreamSimulation, DreamSnapshot, RunPhase, TICK_HZ};
use engine_client::camera::{CameraRig as DreamCameraRig, CameraSettings as DreamCamera};
use ui::{UiAction, UiActions};

#[derive(Resource)]
pub struct DreamView(pub DreamSnapshot);
#[derive(Resource, Default)]
pub struct DreamPreferences {
    pub paused: bool,
    pub build_open: bool,
    pub muted: bool,
    pub selected_slot: usize,
    pub lucid: bool,
}
#[derive(Resource, Default)]
pub struct DreamConnection {
    pub status: String,
    pub connected: bool,
    pub client_id: u64,
    pub http_base: String,
    pub share_url: String,
    pub rtt_ms: f64,
    pub party_size: usize,
}
#[derive(Resource, Default)]
struct CapturedInput(DreamInput);

/// Input stages all run in PreUpdate, before the fixed prediction loop.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DreamInputSystems {
    Receive,
    Keyboard,
    Buttons,
    Apply,
    Capture,
}

fn configure_input_schedule(app: &mut App) {
    // In Bevy 0.19 UI focus and input capture both run in PreUpdate. Explicitly
    // consume current Interaction before routing clicks or suppressing attacks.
    app.configure_sets(
        PreUpdate,
        (
            DreamInputSystems::Receive,
            DreamInputSystems::Keyboard.after(InputSystems),
            DreamInputSystems::Buttons.after(bevy::ui::UiSystems::Focus),
            DreamInputSystems::Apply,
            DreamInputSystems::Capture,
        )
            .chain(),
    );
}
#[derive(Resource, Default)]
struct Playtest {
    autoplay: bool,
    manual_rewards: bool,
    #[cfg(not(target_arch = "wasm32"))]
    capture_dir: Option<String>,
    #[cfg(not(target_arch = "wasm32"))]
    quit_after: f32,
    #[cfg(not(target_arch = "wasm32"))]
    next_capture: f32,
    #[cfg(not(target_arch = "wasm32"))]
    captures: u32,
    next_decision: f64,
}

fn fresh_seed() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(8192)
    }
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now() as u64
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub enum Renderer {
    #[default]
    Prototype,
    Legacy,
}
impl Renderer {
    pub fn from_environment() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let legacy = std::env::args()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|a| a[0] == "--renderer" && a[1] == "legacy");
        #[cfg(target_arch = "wasm32")]
        let legacy = web_sys::window()
            .and_then(|w| w.location().search().ok())
            .is_some_and(|s| {
                s.trim_start_matches('?')
                    .split('&')
                    .any(|v| v == "renderer=legacy")
            });
        if legacy {
            Self::Legacy
        } else {
            Self::Prototype
        }
    }
}

pub fn run(renderer: Renderer) {
    let mut app = build_app();
    match renderer {
        Renderer::Prototype => {
            app.add_plugins(engine_client::presentation::PrototypeRendererPlugin);
        }
        Renderer::Legacy => {
            app.add_plugins(scene::DreamScenePlugin);
        }
    }
    app.run();
}

/// Compose the game, networking, input, UI, and camera without selecting an art renderer.
/// Embedders can add any plugin consuming `engine_client::presentation::PresentationFrame`.
pub fn build_app() -> App {
    let (seed, playtest, options, connection) = startup_options();
    let view = DreamSimulation::new(seed, false).snapshot();
    let mut app = App::new();
    configure_input_schedule(&mut app);
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "DREAMWAKE — The place between waking".into(),
            resolution: (1440, 900).into(),
            canvas: Some("#game-canvas".into()),
            fit_canvas_to_parent: true,
            prevent_default_event_handling: true,
            ..default()
        }),
        ..default()
    }))
    .insert_resource(Time::<Fixed>::from_hz(TICK_HZ as f64))
    .insert_resource(DreamView(view))
    .insert_resource(playtest)
    .insert_resource(options)
    .insert_resource(connection)
    .init_resource::<DreamPreferences>()
    .init_resource::<CapturedInput>()
    .add_plugins((
        engine_client::network_tools::NetworkToolsPlugin,
        engine_client::camera::CameraPlugin,
        engine_client::presentation::PresentationPlugin,
        presentation::DreamPresentationPlugin,
        ui::DreamUiPlugin,
        audio::DreamAudioPlugin,
        network::DreamNetworkPlugin,
        typography::DreamTypographyPlugin,
    ))
    .add_systems(
        PreUpdate,
        (
            keyboard_actions.in_set(DreamInputSystems::Keyboard),
            apply_ui_actions.in_set(DreamInputSystems::Apply),
            capture_input.in_set(DreamInputSystems::Capture),
        ),
    )
    .add_systems(Update, playtest_capture);
    app
}

#[cfg(not(target_arch = "wasm32"))]
fn startup_options() -> (u64, Playtest, network::NetworkOptions, DreamConnection) {
    let mut seed = fresh_seed();
    let mut playtest = Playtest::default();
    let mut options = network::NetworkOptions {
        seed,
        auto_connect: false,
        auto_host: false,
        expected_party: 1,
        hosted: false,
        max_clients: 8,
    };
    let mut connection = DreamConnection {
        http_base: std::env::var("DREAMWAKE_HTTP_BASE")
            .or_else(|_| std::env::var("TD_DREAM_HTTP_BASE"))
            .unwrap_or_else(|_| "http://127.0.0.1:8080".into()),
        status: "Host a dream or join a running server.".into(),
        ..default()
    };
    let args: Vec<String> = std::env::args().collect();
    for (i, arg) in args.iter().enumerate() {
        match arg.as_str() {
            "--seed" => seed = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(seed),
            "--autoplay" => playtest.autoplay = true,
            "--capture-dir" => playtest.capture_dir = args.get(i + 1).cloned(),
            "--quit-after" => {
                playtest.quit_after = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(0.0)
            }
            "--http-base" => {
                if let Some(base) = args.get(i + 1) {
                    connection.http_base = base.clone();
                    options.auto_connect = true;
                }
            }
            "--connect" => options.auto_connect = true,
            "--host" => options.auto_host = true,
            "--wait-players" => {
                options.expected_party = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(1)
            }
            "--max-clients" => {
                options.max_clients = args
                    .get(i + 1)
                    .and_then(|s| s.parse::<u16>().ok())
                    .unwrap_or(8)
                    .clamp(1, 1024)
            }
            _ => {}
        }
    }
    if playtest.autoplay && !options.auto_connect {
        options.auto_host = true;
    }
    options.seed = seed;
    (seed, playtest, options, connection)
}
#[cfg(target_arch = "wasm32")]
fn startup_options() -> (u64, Playtest, network::NetworkOptions, DreamConnection) {
    let seed = fresh_seed();
    let mut base = option_env!("GAME_WEB_HTTP_BASE")
        .or(option_env!("TD_WEB_HTTP_BASE"))
        .unwrap_or("http://127.0.0.1:8080")
        .to_owned();
    let mut playtest = Playtest::default();
    let mut expected_party = 1;
    if let Some(location) = web_sys::window().map(|w| w.location()) {
        if let Ok(search) = location.search() {
            for pair in search.trim_start_matches('?').split('&') {
                if let Some(value) = pair.strip_prefix("server=") {
                    if let Ok(decoded) = js_sys::decode_uri_component(value) {
                        base = decoded.into();
                    }
                }
                if pair == "autoplay=1" {
                    playtest.autoplay = true;
                }
                if pair == "manualRewards=1" {
                    playtest.manual_rewards = true;
                }
                if let Some(value) = pair.strip_prefix("waitPlayers=") {
                    expected_party = value.parse().unwrap_or(1);
                }
            }
        }
    }
    (
        seed,
        playtest,
        network::NetworkOptions {
            seed,
            auto_connect: true,
            auto_host: false,
            expected_party,
        },
        DreamConnection {
            http_base: base,
            status: "Connecting to the shared dream…".into(),
            ..default()
        },
    )
}

fn keyboard_actions(
    keys: Res<ButtonInput<KeyCode>>,
    view: Res<DreamView>,
    prefs: Res<DreamPreferences>,
    connection: Res<DreamConnection>,
    mut actions: ResMut<UiActions>,
    mut camera: ResMut<DreamCamera>,
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
    if keys.just_pressed(KeyCode::Equal) || keys.just_pressed(KeyCode::NumpadAdd) {
        camera.zoom = (camera.zoom - 0.1).max(camera.zoom_bounds[0]);
    }
    if keys.just_pressed(KeyCode::Minus) || keys.just_pressed(KeyCode::NumpadSubtract) {
        camera.zoom = (camera.zoom + 0.1).min(camera.zoom_bounds[1]);
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

fn apply_ui_actions(world: &mut World) {
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
fn capture_input(
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
    // Camera has no yaw: screen right is +X and screen up is -Z. Cursor is
    // projected once onto the canonical XZ plane; no downstream axis flips.
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
            aim = right.normalize();
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
    captured.0.movement = movement.clamp_length_max(1.0).to_array();
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

#[cfg(not(target_arch = "wasm32"))]
fn playtest_capture(
    mut commands: Commands,
    time: Res<Time<Real>>,
    mut test: ResMut<Playtest>,
    keys: Res<ButtonInput<KeyCode>>,
    view: Res<DreamView>,
    mut exit: MessageWriter<AppExit>,
) {
    #[cfg(not(target_arch = "wasm32"))]
    if keys.just_pressed(KeyCode::F9)
        || (test.capture_dir.is_some() && time.elapsed_secs() > test.next_capture)
    {
        use bevy::render::view::screenshot::{Screenshot, save_to_disk};
        let dir = test.capture_dir.as_deref().unwrap_or("/tmp/dreamwake");
        let _ = std::fs::create_dir_all(dir);
        let path = format!("{dir}/dreamwake-{:03}.png", test.captures);
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
        info!(
            "Dreamwake capture: room={} phase={:?} hp={:.0} kills={}",
            view.0.room + 1,
            view.0.phase,
            view.0.hero.hp,
            view.0.kills
        );
        test.captures += 1;
        test.next_capture = time.elapsed_secs() + 8.0;
    }
    if test.quit_after > 0.0 && time.elapsed_secs() >= test.quit_after {
        exit.write(AppExit::Success);
    }
}

#[cfg(target_arch = "wasm32")]
fn playtest_capture(
    test: Res<Playtest>,
    view: Res<DreamView>,
    time: Res<Time<Real>>,
    mut previous: Local<Option<(usize, RunPhase, u8)>>,
    mut sample: Local<(f32, u32)>,
) {
    if !test.autoplay {
        return;
    }
    sample.0 += time.delta_secs();
    sample.1 += 1;
    let boss_phase = view.0.enemies.iter().map(|e| e.phase).max().unwrap_or(0);
    let state = (view.0.room, view.0.phase, boss_phase);
    if previous.as_ref() != Some(&state) {
        info!(
            "Dreamwake browser playtest: room={} phase={:?} enemy_phase={} party={} hp={:.0} kills={} elapsed={:.1}s mean_fps={:.1}",
            view.0.room + 1,
            view.0.phase,
            boss_phase,
            view.0.heroes.len(),
            view.0.hero.hp,
            view.0.kills,
            view.0.elapsed,
            sample.1 as f32 / sample.0.max(0.001)
        );
        *previous = Some(state);
        *sample = (0.0, 0);
    }
}
