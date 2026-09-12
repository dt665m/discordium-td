//! Dreamwake client composition. Shared simulation owns gameplay.
pub(crate) mod identity;
pub mod plugins;
use bevy::prelude::*;
use diagnostics::Playtest;
use dreamwake_sim::{DreamPresentation, DreamSimulation, DreamSnapshot, TICK_HZ};
use plugins::{audio, camera, diagnostics, graphics, input, network};
pub use plugins::{graphics::scene, ui};
#[derive(Resource)]
pub struct DreamView(pub DreamPresentation);
/// Convert a real local lobby/test simulation view into display-only data.
/// No authoritative snapshot or secret seed is retained by the client view.
pub(crate) fn offline_presentation(snapshot: DreamSnapshot) -> DreamPresentation {
    let collision = std::sync::Arc::new(snapshot.collision_manifest().clone());
    DreamPresentation {
        stamp: dreamwake_sim::replication::ReplicationStamp {
            match_epoch: 1,
            server_tick: snapshot.tick as u64,
            gameplay_tick: snapshot.tick,
            scene_revision: collision.scene_revision(),
            revision: 1,
        },
        collision: Some(collision),
        tick: snapshot.tick,
        phase: snapshot.phase,
        active_travelers: snapshot.heroes.len().try_into().unwrap_or(u16::MAX),
        room: snapshot.room,
        realm: snapshot.realm,
        encounter_name: snapshot.encounter_name,
        hero: snapshot.hero,
        heroes: snapshot.heroes,
        ready: snapshot.ready,
        awaiting_party: snapshot.awaiting_party,
        enemies: snapshot.enemies,
        covers: snapshot.covers,
        platforms: snapshot.platforms,
        cover_markers: vec![],
        projectiles: snapshot.projectiles,
        wisps: snapshot.wisps,
        presentations: snapshot.presentations,
        damage_numbers: snapshot.damage_numbers,
        rewards: snapshot.rewards,
        kills: snapshot.kills,
        elapsed: snapshot.elapsed,
        lucid: snapshot.lucid,
        paused: snapshot.paused,
        cleared: snapshot.cleared,
        enemies_remaining: snapshot.enemies_remaining,
        message: snapshot.message,
    }
}

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
    pub player_id: u64,
    pub http_base: String,
    pub share_url: String,
    pub rtt_ms: f64,
    pub party_size: usize,
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

pub fn run() {
    let mut app = build_app();
    app.add_plugins(graphics::scene::DreamScenePlugin);
    app.run();
}

/// Compose the game, networking, input, UI, and camera without selecting an art renderer.
/// Embedders can add any plugin consuming `engine_client::graphics::GraphicsFrame`.
pub fn build_app() -> App {
    let (seed, playtest, options, connection) = startup_options();
    let view = offline_presentation(DreamSimulation::new(seed, false).snapshot());
    let traveler = identity::TravelerProfile::load(&connection.http_base);
    let mut app = App::new();
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
    .insert_resource(traveler)
    .init_resource::<DreamPreferences>()
    .add_plugins((
        engine_client::network_tools::NetworkToolsPlugin,
        engine_client::camera::CameraPlugin,
        engine_client::graphics::GraphicsPlugin,
        camera::DreamCameraPlugin,
        graphics::DreamGraphicsPlugin,
        input::DreamInputPlugin,
        ui::DreamUiPlugin,
        audio::DreamAudioPlugin,
        network::DreamNetworkPlugin,
        diagnostics::DreamDiagnosticsPlugin,
    ));
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
            "--autoplay-restart" => {
                playtest.autoplay = true;
                playtest.restart_on_terminal = true;
            }
            "--spatial-playtest" => {
                playtest.spatial = match args.get(i + 1).map(String::as_str) {
                    Some("left") => diagnostics::SpatialPlaytest::Left,
                    Some("right") => diagnostics::SpatialPlaytest::Right,
                    _ => panic!("--spatial-playtest requires left or right"),
                };
                playtest.autoplay = true;
            }
            "--network-trace" => playtest.trace_path = args.get(i + 1).cloned(),
            "--network-trace-chunks" => {
                playtest.trace_options.chunks = args
                    .get(i + 1)
                    .and_then(|s| s.parse().ok())
                    .filter(|n| (1..=128).contains(n))
                    .expect("--network-trace-chunks requires 1 to 128");
            }
            "--network-trace-interval-seconds" => {
                playtest.trace_options.interval_seconds = args
                    .get(i + 1)
                    .and_then(|s| s.parse::<f64>().ok())
                    .filter(|n| n.is_finite() && (0.25..=60.0).contains(n))
                    .expect("--network-trace-interval-seconds requires 0.25 to 60");
            }
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
    if (playtest.trace_options.chunks != 1 || playtest.trace_options.interval_seconds != 0.25)
        && playtest.trace_path.is_none()
    {
        panic!("trace chunk/interval options require --network-trace PATH");
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
    let base = option_env!("GAME_WEB_HTTP_BASE")
        .unwrap_or("http://127.0.0.1:8080")
        .to_owned();
    (
        seed,
        Playtest::default(),
        network::NetworkOptions {
            seed,
            auto_connect: true,
            auto_host: false,
            expected_party: 1,
        },
        DreamConnection {
            http_base: base,
            status: "Connecting to the shared dream…".into(),
            ..default()
        },
    )
}
