use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::Duration,
};
#[cfg(not(target_arch = "wasm32"))]
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    thread,
};

use bevy::dev_tools::fps_overlay::{FpsOverlayConfig, FpsOverlayPlugin, FrameTimeGraphConfig};
use bevy::{
    app::AppExit,
    asset::RenderAssetUsages,
    light::NotShadowCaster,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
#[cfg(not(target_arch = "wasm32"))]
use clap::Parser;
#[cfg(not(target_arch = "wasm32"))]
use game_server::{ServerArgs, run as run_server};
use game_shared::{
    AbilityId, AttackPhase, BASE_POSITION, BUILD_COMMAND_MAX_DISTANCE, BUILD_NODES, BuildNodeDef,
    ChargePhase, ClientCommand, DirectionalAttackStateComponent, ENEMY_REGULAR_ATTACK,
    EnemySnapshot, FIXED_DT_SECONDS, HERO_COLLIDER_RADIUS, HERO_MAX_HP, HERO_MAX_MANA,
    HERO_REGULAR_ATTACK, HERO_SPEED, HeroSnapshot, JoinSnapshot, MatchPhase, ObjectiveSnapshot,
    PROTOCOL_ID, ReliableGameEvent, ReliableServerMessage, TOWER_COLLIDER_RADIUS, TowerSnapshot,
    TowerType, WorldDelta, clamp_to_world, distance_sq, encode, enemy_collider_radius,
    is_newer_input_seq, normalize_or_zero,
};
use renet::{DefaultChannel, RenetClient};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures::spawn_local;

use super::lock_on::{LockOnPlugin, select_next_lock_target};

const ABILITY_EFFECT_DURATION_SECONDS: f32 = 0.55;
const ABILITY_EFFECT_START_RADIUS: f32 = 0.8;
const HIT_EFFECT_DURATION_SECONDS: f32 = 0.22;
const HIT_EFFECT_START_SCALE: f32 = 0.25;
const TOWER_SHOT_EFFECT_DURATION_SECONDS: f32 = 0.09;
const REGULAR_ATTACK_EFFECT_DURATION_SECONDS: f32 = 0.16;
const REGULAR_ATTACK_EFFECT_Y: f32 = 0.2;
const REGULAR_ATTACK_CONE_SEGMENTS: usize = 24;
const FACING_TURN_SMOOTH_RATE: f32 = 16.0;
const ATTACK_COMMIT_VISUAL_SMOOTH_RATE: f32 = 22.0;
const CHARGE_PULSE_SPEED: f32 = 9.0;
const CHARGE_PULSE_STARTUP_AMPLITUDE: f32 = 0.1;
const CHARGE_PULSE_ACTIVE_AMPLITUDE: f32 = 0.18;
const CHARGE_PULSE_RECOVERY_AMPLITUDE: f32 = 0.12;
const POWER_FLICKER_HZ: f32 = 13.0;
const POWER_FLICKER_HIDDEN_DUTY: f32 = 0.28;
const HERO_HIT_REACTION_DURATION_SECONDS: f32 = 0.16;
const ENEMY_HIT_REACTION_DURATION_SECONDS: f32 = 0.16;
const TOWER_FIRE_REACTION_DURATION_SECONDS: f32 = 0.14;
const ENEMY_RENDER_LEAD_SECONDS: f32 = FIXED_DT_SECONDS * 1.2;
const ENEMY_RENDER_SMOOTH_RATE: f32 = 18.0;
const REMOTE_HERO_RENDER_SMOOTH_RATE: f32 = 14.0;
const LOCAL_HERO_RECONCILE_RATE: f32 = 24.0;
const LOCAL_HERO_RENDER_SMOOTH_RATE: f32 = 30.0;
const LOCAL_HERO_SNAP_DISTANCE_SQ: f32 = 9.0;
const LOCAL_HERO_COLLISION_MAX_CORRECTION_PER_STEP: f32 = HERO_SPEED * FIXED_DT_SECONDS * 0.65;
const LOCAL_HERO_COLLISION_MAX_TOTAL_CORRECTION: f32 = HERO_SPEED * FIXED_DT_SECONDS * 1.05;
const RENDER_SNAP_DISTANCE_SQ: f32 = 20.0;
const UNIT_BAR_WIDTH: f32 = 0.9;
const UNIT_BAR_HEIGHT: f32 = 0.14;
const UNIT_BAR_DEPTH: f32 = 0.045;
const UNIT_BAR_HERO_WIDTH_SCALE: f32 = 1.55;
const UNIT_BAR_ENEMY_WIDTH_SCALE: f32 = 1.0;
const FACING_INDICATOR_WIDTH: f32 = 0.17;
const FACING_INDICATOR_HEIGHT: f32 = 0.15;
const FACING_INDICATOR_DEPTH: f32 = 0.32;
const UNIT_BAR_HERO_Y: f32 = 1.2;
const UNIT_BAR_ENEMY_Y: f32 = 0.78;
const UNIT_BAR_ROW_GAP: f32 = 0.17;
const UNIT_BAR_FILL_Z: f32 = 0.03;
const MIN_BAR_FILL_RATIO: f32 = 0.01;
#[cfg(not(target_arch = "wasm32"))]
const LOCAL_HOST_HTTP_PORT: u16 = 18080;
#[cfg(not(target_arch = "wasm32"))]
const LOCAL_HOST_UDP_PORT: u16 = 15000;
#[cfg(not(target_arch = "wasm32"))]
const LOCAL_HOST_WEBRTC_PORT: u16 = 15001;
const MENU_BUTTON_NORMAL: Color = Color::srgb(0.15, 0.19, 0.23);
const MENU_BUTTON_HOVERED: Color = Color::srgb(0.22, 0.29, 0.34);
const MENU_BUTTON_PRESSED: Color = Color::srgb(0.28, 0.43, 0.5);
const MENU_BUTTON_DISABLED: Color = Color::srgba(0.2, 0.21, 0.23, 0.74);
const MENU_PANEL_COLOR: Color = Color::srgba(0.03, 0.04, 0.05, 0.76);
const SPECIAL_SKILL_ICON_SIZE: f32 = 76.0;
const SPECIAL_SKILL_ICON_MARGIN: f32 = 18.0;
const SPECIAL_SKILL_ICON_READY_BG: Color = Color::srgba(0.11, 0.2, 0.32, 0.58);
const SPECIAL_SKILL_ICON_COOLDOWN_BG: Color = Color::srgba(0.08, 0.09, 0.1, 0.46);
const SPECIAL_SKILL_ICON_BORDER: Color = Color::srgba(0.64, 0.76, 0.88, 0.8);
const SPECIAL_SKILL_ICON_KEY_COLOR: Color = Color::srgb(0.95, 0.98, 1.0);
const SPECIAL_SKILL_ICON_OVERLAY: Color = Color::srgba(0.44, 0.46, 0.5, 0.5);
const POWER_PIE_SLOT_SIZE: f32 = 126.0;
const POWER_PIE_BASE_SIZE: f32 = 92.0;
const POWER_PIE_ACTIVE_SIZE: f32 = 106.0;
const POWER_PIE_ACTIVE_PULSE_SIZE: f32 = 8.0;
const POWER_PIE_LEFT_OFFSET: f32 = SPECIAL_SKILL_ICON_MARGIN;
const POWER_PIE_BOTTOM_OFFSET: f32 = SPECIAL_SKILL_ICON_MARGIN - 12.0;
const SPECIAL_SKILL_ICON_LEFT_OFFSET: f32 = POWER_PIE_LEFT_OFFSET + POWER_PIE_SLOT_SIZE + 12.0;
const POWER_PIE_TEXTURE_SIZE: u32 = 96;
const POWER_PIE_EMPTY_COLOR: Color = Color::srgba(0.11, 0.12, 0.16, 0.62);
const POWER_PIE_FILL_COLOR: Color = Color::srgba(0.31, 0.92, 0.98, 0.82);
const POWER_PIE_FILL_ACTIVE_COLOR: Color = Color::srgba(1.0, 0.86, 0.25, 0.9);
const POWER_PIE_BORDER_COLOR: Color = Color::srgb(0.42, 0.74, 0.98);
const POWER_PIE_BORDER_ACTIVE_COLOR: Color = Color::srgb(1.0, 0.92, 0.44);
const POWER_PIE_BG_COLOR: Color = Color::srgba(0.05, 0.08, 0.12, 0.58);
const POWER_PIE_BG_ACTIVE_COLOR: Color = Color::srgba(0.21, 0.16, 0.06, 0.64);
const POWER_PIE_FLASH_HZ: f32 = 7.0;
const POWER_PIE_FLASH_BUCKETS: u32 = 18;
const LOCKED_HEALTH_BAR_SCALE_MULTIPLIER: f32 = 1.5;
const GOLD_HUD_PANEL_COLOR: Color = Color::srgba(0.05, 0.07, 0.08, 0.6);
const GOLD_HUD_BORDER_COLOR: Color = Color::srgba(0.47, 0.55, 0.6, 0.78);
const GOLD_HUD_LABEL_COLOR: Color = Color::srgb(0.86, 0.9, 0.95);
const GOLD_HUD_VALUE_COLOR: Color = Color::srgb(1.0, 0.9, 0.45);
const GOLD_SPEND_TEXT_COLOR: Color = Color::srgba(1.0, 0.23, 0.2, 0.95);
const GOLD_HUD_RIGHT_OFFSET: f32 = 16.0;
const GOLD_HUD_TOP_OFFSET: f32 = 12.0;
const GOLD_SPEND_POPUP_DURATION_SECONDS: f32 = 0.75;
const GOLD_SPEND_POPUP_START_TOP: f32 = 34.0;
const GOLD_SPEND_POPUP_RISE_PIXELS: f32 = 16.0;
const MATCH_END_OVERLAY_BG: Color = Color::srgba(0.04, 0.07, 0.1, 0.74);
const MATCH_END_OVERLAY_BORDER: Color = Color::srgba(0.53, 0.65, 0.76, 0.84);
const MATCH_END_OVERLAY_TITLE: Color = Color::srgb(0.94, 0.97, 1.0);
const MATCH_END_OVERLAY_DEFEAT_TITLE: Color = Color::srgb(1.0, 0.78, 0.72);

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Resource, Parser)]
#[command(name = "game_client")]
pub struct ClientArgs {
    #[arg(long, env = "TD_HTTP_BASE", default_value = "http://127.0.0.1:8080")]
    http_base: String,
}

#[cfg(target_arch = "wasm32")]
#[derive(Debug, Clone, Resource)]
pub struct ClientArgs {
    http_base: String,
}

#[cfg(target_arch = "wasm32")]
impl Default for ClientArgs {
    fn default() -> Self {
        Self {
            http_base: default_http_base(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PendingMove {
    seq: u32,
    dir: [f32; 2],
}

#[cfg(not(target_arch = "wasm32"))]
type ClientTransport = renet_cross::UdpNetcodeClientTransport;
#[cfg(target_arch = "wasm32")]
type ClientTransport = renet_cross::WebRtcNetcodeClientTransport;

struct NetworkRuntime {
    client_id: u64,
    next_seq: u32,
    pending_moves: VecDeque<PendingMove>,
    renet: RenetClient,
    transport: ClientTransport,
}

impl NetworkRuntime {
    fn new(client_id: u64, renet: RenetClient, transport: ClientTransport) -> Self {
        Self {
            client_id,
            next_seq: 0,
            pending_moves: VecDeque::new(),
            renet,
            transport,
        }
    }

    fn transport_update(&mut self, duration: Duration) -> Result<(), String> {
        self.transport
            .update(duration, &mut self.renet)
            .map_err(|err| err.to_string())
    }

    fn transport_send_packets(&mut self) -> Result<(), String> {
        self.transport
            .send_packets(&mut self.renet)
            .map_err(|err| err.to_string())
    }

    fn next_command_seq(&mut self) -> u32 {
        self.next_seq = self.next_seq.wrapping_add(1);
        self.next_seq
    }
}

#[cfg(target_arch = "wasm32")]
struct PendingWebBootstrap {
    slot: std::rc::Rc<
        std::cell::RefCell<Option<Result<(RenetClient, ClientTransport, u64), String>>>,
    >,
}

#[derive(Resource, Default)]
struct InputState {
    dir: [f32; 2],
}

#[derive(Resource, Default)]
struct LocalHeroSmoothing {
    predicted_pos: Option<[f32; 2]>,
    render_pos: Option<[f32; 2]>,
}

#[derive(Resource)]
struct UnitBarCameraCache {
    world_rotation: Quat,
    revision: u64,
    initialized: bool,
}

impl Default for UnitBarCameraCache {
    fn default() -> Self {
        Self {
            world_rotation: Quat::IDENTITY,
            revision: 0,
            initialized: false,
        }
    }
}

#[derive(Resource, Default)]
struct HudState {
    last_event: String,
}

#[derive(Clone, Copy)]
enum MenuAction {
    #[cfg(not(target_arch = "wasm32"))]
    SinglePlayer,
    ConnectDev,
}

#[derive(Clone)]
#[cfg(not(target_arch = "wasm32"))]
struct LocalHostedServer {
    local_http_base: String,
    public_http_base: String,
}

#[derive(Resource)]
struct MainMenuState {
    pending_action: Option<MenuAction>,
    connect_request_in_flight: bool,
    status: String,
    #[cfg(not(target_arch = "wasm32"))]
    hosted_server: Option<LocalHostedServer>,
}

impl Default for MainMenuState {
    fn default() -> Self {
        Self {
            pending_action: None,
            connect_request_in_flight: false,
            status: "Select a mode to start.".to_owned(),
            #[cfg(not(target_arch = "wasm32"))]
            hosted_server: None,
        }
    }
}

#[derive(Resource, Default)]
struct SpecialSkillCooldownUiState {
    last_ticks_remaining: u32,
    max_ticks_remaining: u32,
}

#[derive(Resource)]
struct PowerPieUiTexture {
    handle: Handle<Image>,
}

#[derive(Resource, Default)]
struct PowerPieUiState {
    last_ratio: f32,
    last_flash_bucket: u32,
    last_power_active: bool,
    initialized: bool,
}

#[derive(Resource)]
struct PowerPieRasterCache {
    size: u32,
    inside: Vec<bool>,
    clockwise_angle: Vec<f32>,
}

#[derive(Resource, Default)]
struct GoldHudState {
    last_gold: Option<u32>,
}

#[derive(Resource)]
struct SceneAssets {
    hero_mesh: Handle<Mesh>,
    enemy_mesh: Handle<Mesh>,
    tower_mesh: Handle<Mesh>,
    unit_bar_mesh: Handle<Mesh>,
    facing_indicator_mesh: Handle<Mesh>,
    ability_effect_mesh: Handle<Mesh>,
    hit_effect_mesh: Handle<Mesh>,
    shot_effect_mesh: Handle<Mesh>,
    hero_attack_cone_mesh: Handle<Mesh>,
    enemy_attack_cone_mesh: Handle<Mesh>,
    hero_local_material: Handle<StandardMaterial>,
    hero_remote_material: Handle<StandardMaterial>,
    enemy_material: Handle<StandardMaterial>,
    tower_material: Handle<StandardMaterial>,
    node_free_material: Handle<StandardMaterial>,
    node_taken_material: Handle<StandardMaterial>,
    bar_background_material: Handle<StandardMaterial>,
    hero_health_bar_material: Handle<StandardMaterial>,
    enemy_health_bar_material: Handle<StandardMaterial>,
    hero_facing_indicator_material: Handle<StandardMaterial>,
    enemy_facing_indicator_material: Handle<StandardMaterial>,
    mana_bar_material: Handle<StandardMaterial>,
    power_bar_material: Handle<StandardMaterial>,
}

#[derive(Resource, Default)]
struct RenderIndex {
    by_id: HashMap<u64, Entity>,
}

#[derive(Resource, Default)]
struct WorldView {
    tick: u32,
    phase: MatchPhase,
    match_restart_ticks_remaining: Option<u32>,
    wave: u32,
    team_life: i32,
    you: Option<u64>,
    build_nodes: Vec<BuildNodeDef>,
    heroes: HashMap<u64, HeroSnapshot>,
    enemies: HashMap<u64, EnemySnapshot>,
    towers: HashMap<u64, TowerSnapshot>,
    objectives: Vec<ObjectiveSnapshot>,
}

impl WorldView {
    fn apply_join(&mut self, snapshot: JoinSnapshot) {
        self.you = Some(snapshot.you);
        self.build_nodes = snapshot.build_nodes;
        self.apply_delta(snapshot.world);
    }

    fn apply_delta(&mut self, delta: WorldDelta) {
        self.tick = delta.tick;
        self.phase = delta.phase;
        self.match_restart_ticks_remaining = delta.match_restart_ticks_remaining;
        self.wave = delta.wave;
        self.team_life = delta.team_life;
        self.objectives = delta.objectives;

        self.heroes.clear();
        self.heroes
            .extend(delta.heroes.into_iter().map(|hero| (hero.client_id, hero)));

        self.enemies.clear();
        self.enemies
            .extend(delta.enemies.into_iter().map(|enemy| (enemy.id, enemy)));

        self.towers.clear();
        self.towers
            .extend(delta.towers.into_iter().map(|tower| (tower.id, tower)));
    }

    fn objective(&self) -> Option<ObjectiveSnapshot> {
        self.objectives.first().copied()
    }
}

#[derive(Component)]
struct DynamicActor;

#[derive(Component)]
struct HeroActor;

#[derive(Component)]
struct LocalHeroActor;

#[derive(Component)]
struct EnemyActor;

#[derive(Component)]
struct TowerActor;

#[derive(Component, Clone, Copy, PartialEq)]
struct TowerBuildNode {
    node_id: u32,
}

#[derive(Component)]
struct BuildNodeMarker {
    node_id: u32,
}

#[derive(Component)]
struct ObjectiveMarker;

#[derive(Component)]
struct HudText;

#[derive(Component)]
struct SpecialSkillIcon;

#[derive(Component)]
struct SpecialSkillCooldownOverlay;

#[derive(Component)]
struct SpecialSkillKeyText;

#[derive(Component)]
struct PowerPieHudRoot;

#[derive(Component)]
struct PowerPieCircle;

#[derive(Component)]
struct PowerPieFillImage;

#[derive(Component)]
struct GoldHudRoot;

#[derive(Component)]
struct GoldHudValueText;

#[derive(Component)]
struct GoldSpendPopup {
    age_seconds: f32,
}

#[derive(Component)]
struct MainMenuRoot;

#[derive(Component)]
struct MainMenuStatusText;

#[derive(Component, Clone, Copy)]
struct MainMenuButton(MenuAction);

#[derive(Component)]
struct MatchEndOverlayRoot;

#[derive(Component)]
struct MatchEndOverlayText;

#[derive(Component)]
struct UnitBarCamera;

#[derive(Event, Clone, Copy)]
struct UnitBarCameraChanged {
    world_rotation: Quat,
}

#[derive(Component)]
struct AbilityEffectVisual {
    age_seconds: f32,
    duration_seconds: f32,
    max_radius: f32,
    material: Handle<StandardMaterial>,
}

#[derive(Component)]
struct HitEffectVisual {
    age_seconds: f32,
    duration_seconds: f32,
    material: Handle<StandardMaterial>,
}

#[derive(Component)]
struct RegularAttackEffectVisual {
    age_seconds: f32,
    duration_seconds: f32,
    material: Handle<StandardMaterial>,
}

#[derive(Component, Default)]
struct HeroHitReaction {
    age_seconds: f32,
}

#[derive(Component, Default)]
struct EnemyHitReaction {
    age_seconds: f32,
}

#[derive(Component, Default)]
struct TowerFireReaction {
    age_seconds: f32,
}

#[derive(Component)]
struct TowerShotEffectVisual {
    age_seconds: f32,
    duration_seconds: f32,
    material: Handle<StandardMaterial>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BarStat {
    Health,
    Mana,
    Power,
}

#[derive(Component, Clone, Copy, PartialEq)]
struct HealthStat {
    current: f32,
    max: f32,
}

#[derive(Component, Clone, Copy, PartialEq)]
struct ManaStat {
    current: f32,
    max: f32,
}

#[derive(Component, Clone, Copy, PartialEq)]
struct PowerStat {
    current: f32,
    max: f32,
}

#[derive(Component, Clone, Copy, PartialEq)]
struct ChargeStateStat {
    phase: ChargePhase,
    power_active: bool,
}

#[derive(Component)]
struct UnitBarFill {
    stat: BarStat,
}

#[derive(Component)]
struct UnitBarVisual;

#[derive(Component, Clone, Copy)]
struct UnitBarRow {
    stat: BarStat,
}

#[derive(Component, Clone, Copy)]
struct UnitBarFollow {
    target: Entity,
}

#[derive(Component, Clone, Copy)]
struct UnitBarLayout {
    row_index: u32,
    base_y: f32,
    fill: bool,
    max_scale_x: f32,
}

#[derive(Component, Clone, Copy)]
struct UnitBarBillboardState {
    camera_revision: u64,
    target_translation: Vec3,
    last_scale_x: f32,
    initialized: bool,
}

impl Default for UnitBarBillboardState {
    fn default() -> Self {
        Self {
            camera_revision: 0,
            target_translation: Vec3::ZERO,
            last_scale_x: 0.0,
            initialized: false,
        }
    }
}

#[derive(Component)]
struct FacingIndicatorVisual;

#[derive(Component, Clone, Copy, PartialEq)]
struct FacingStat {
    dir: [f32; 2],
}

#[derive(Component, Clone, Copy, PartialEq)]
struct RegularAttackStat {
    phase: AttackPhase,
    ticks_remaining: u32,
}

#[derive(Clone, Copy)]
enum ActorKind {
    Hero { local: bool },
    Enemy,
    Tower,
}

#[derive(Clone, Copy)]
struct DesiredActor {
    pos: [f32; 2],
    kind: ActorKind,
    facing: Option<FacingStat>,
    regular_attack: Option<RegularAttackStat>,
    health: Option<HealthStat>,
    mana: Option<ManaStat>,
    power: Option<PowerStat>,
    charge_state: Option<ChargeStateStat>,
    tower_node: Option<TowerBuildNode>,
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ClientUpdateSet {
    Input,
    Network,
    Visual,
    Ui,
    Shutdown,
}

pub struct GameClientPlugin;

impl Plugin for GameClientPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(InputState::default())
            .insert_resource(LocalHeroSmoothing::default())
            .insert_resource(UnitBarCameraCache::default())
            .insert_resource(HudState::default())
            .insert_resource(SpecialSkillCooldownUiState::default())
            .insert_resource(PowerPieUiState::default())
            .insert_resource(GoldHudState::default())
            .insert_resource(MainMenuState::default())
            .insert_resource(WorldView::default())
            .insert_resource(RenderIndex::default())
            .insert_resource(Time::<Fixed>::from_hz(game_shared::SERVER_TICK_HZ as f64))
            .insert_resource(ClearColor(Color::srgb(0.05, 0.06, 0.07)))
            .add_plugins(DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Discordium TD MVP Client".to_string(),
                    resolution: (1400_u32, 900_u32).into(),
                    ..Default::default()
                }),
                ..Default::default()
            }))
            .add_plugins(LockOnPlugin)
            .add_systems(Startup, (setup_scene, setup_main_menu))
            .configure_sets(
                Update,
                (
                    ClientUpdateSet::Input,
                    ClientUpdateSet::Network,
                    ClientUpdateSet::Visual,
                    ClientUpdateSet::Ui,
                    ClientUpdateSet::Shutdown,
                )
                    .chain(),
            )
            .add_systems(Update, handle_menu_buttons.in_set(ClientUpdateSet::Input))
            .add_systems(
                Update,
                process_menu_actions
                    .after(handle_menu_buttons)
                    .before(capture_input)
                    .in_set(ClientUpdateSet::Input),
            )
            .add_systems(Update, capture_input.in_set(ClientUpdateSet::Input))
            .add_systems(
                Update,
                (
                    send_action_commands,
                    #[cfg(target_arch = "wasm32")]
                    poll_web_bootstrap,
                    network_update,
                )
                    .chain()
                    .in_set(ClientUpdateSet::Network),
            )
            .add_systems(
                Update,
                (
                    update_ability_effects,
                    update_hit_effects,
                    update_tower_shot_effects,
                    update_regular_attack_effects,
                    sync_dynamic_actors,
                    cleanup_orphan_unit_bars.run_if(resource_changed::<WorldView>),
                    update_actor_facing_and_attack_visuals,
                    update_local_hero_power_flicker,
                    update_hero_hit_reactions,
                    update_enemy_hit_reactions,
                    update_tower_fire_reactions,
                    update_unit_bars.run_if(resource_changed::<WorldView>),
                    update_unit_bar_background_scales.run_if(resource_changed::<WorldView>),
                    update_unit_bar_visibility.run_if(resource_changed::<WorldView>),
                    publish_unit_bar_camera_updates,
                    orient_unit_bars_to_camera,
                    update_build_node_markers.run_if(resource_changed::<WorldView>),
                    update_objective_markers.run_if(resource_changed::<WorldView>),
                )
                    .chain()
                    .in_set(ClientUpdateSet::Visual),
            )
            .add_systems(
                Update,
                (
                    update_hud,
                    update_gold_hud.run_if(resource_changed::<WorldView>),
                    update_gold_spend_popups,
                    update_special_skill_hud.run_if(resource_changed::<WorldView>),
                    update_power_pie_hud,
                    update_match_end_overlay.run_if(resource_changed::<WorldView>),
                    sync_menu_button_visual_state,
                    sync_main_menu_state,
                )
                    .chain()
                    .in_set(ClientUpdateSet::Ui),
            )
            .add_systems(
                Update,
                graceful_disconnect_on_app_exit.in_set(ClientUpdateSet::Shutdown),
            )
            .add_observer(apply_unit_bar_camera_update)
            .add_systems(FixedUpdate, send_movement_commands);

        app.add_plugins(FpsOverlayPlugin {
            config: FpsOverlayConfig {
                text_config: TextFont {
                    font_size: 16.0,
                    ..Default::default()
                },
                text_color: Color::srgb(0.85, 0.95, 0.9),
                refresh_interval: std::time::Duration::from_millis(120),
                enabled: true,
                frame_time_graph_config: FrameTimeGraphConfig {
                    enabled: false,
                    min_fps: 30.0,
                    target_fps: 60.0,
                },
            },
        });
    }
}

fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let hero_mesh = meshes.add(Capsule3d::new(0.45, 0.8));
    let enemy_mesh = meshes.add(Sphere::new(0.5));
    let tower_mesh = meshes.add(Cuboid::new(0.9, 1.4, 0.9));
    let unit_bar_mesh = meshes.add(Cuboid::new(UNIT_BAR_WIDTH, UNIT_BAR_HEIGHT, UNIT_BAR_DEPTH));
    let facing_indicator_mesh = meshes.add(Cuboid::new(
        FACING_INDICATOR_WIDTH,
        FACING_INDICATOR_HEIGHT,
        FACING_INDICATOR_DEPTH,
    ));
    let ability_effect_mesh = meshes.add(Cylinder::new(1.0, 0.08));
    let hit_effect_mesh = meshes.add(Sphere::new(0.22));
    let shot_effect_mesh = meshes.add(Cuboid::new(1.0, 0.08, 0.08));
    let hero_attack_cone_mesh = meshes.add(build_unit_attack_cone_mesh(
        HERO_REGULAR_ATTACK.arc_dot_threshold,
        REGULAR_ATTACK_CONE_SEGMENTS,
    ));
    let enemy_attack_cone_mesh = meshes.add(build_unit_attack_cone_mesh(
        ENEMY_REGULAR_ATTACK.arc_dot_threshold,
        REGULAR_ATTACK_CONE_SEGMENTS,
    ));

    let hero_local_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.25, 0.85, 0.9),
        perceptual_roughness: 0.72,
        ..Default::default()
    });
    let hero_remote_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.4, 0.55, 0.8),
        perceptual_roughness: 0.8,
        ..Default::default()
    });
    let enemy_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.86, 0.34, 0.31),
        perceptual_roughness: 0.92,
        ..Default::default()
    });
    let tower_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.87, 0.78, 0.31),
        perceptual_roughness: 0.52,
        ..Default::default()
    });
    let node_free_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.2, 0.7, 0.3),
        emissive: LinearRgba::new(0.04, 0.15, 0.05, 0.0),
        ..Default::default()
    });
    let node_taken_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.45, 0.45, 0.45),
        perceptual_roughness: 0.9,
        ..Default::default()
    });
    let bar_background_material = materials.add(StandardMaterial {
        base_color: Color::srgba(0.03, 0.03, 0.04, 0.82),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        cull_mode: None,
        ..Default::default()
    });
    let hero_health_bar_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.22, 0.86, 0.36),
        emissive: LinearRgba::new(0.02, 0.09, 0.03, 0.0),
        unlit: true,
        ..Default::default()
    });
    let enemy_health_bar_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.9, 0.28, 0.2),
        emissive: LinearRgba::new(0.08, 0.02, 0.01, 0.0),
        unlit: true,
        ..Default::default()
    });
    let hero_facing_indicator_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.92, 0.98, 1.0),
        emissive: LinearRgba::new(0.12, 0.16, 0.2, 0.0),
        unlit: true,
        ..Default::default()
    });
    let enemy_facing_indicator_material = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.74, 0.22),
        emissive: LinearRgba::new(0.2, 0.12, 0.02, 0.0),
        unlit: true,
        ..Default::default()
    });
    let mana_bar_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.18, 0.58, 0.98),
        emissive: LinearRgba::new(0.02, 0.07, 0.12, 0.0),
        unlit: true,
        ..Default::default()
    });
    let power_bar_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.98, 0.4, 0.95),
        emissive: LinearRgba::new(0.16, 0.04, 0.14, 0.0),
        unlit: true,
        ..Default::default()
    });
    let objective_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.92, 0.45, 0.2),
        emissive: LinearRgba::new(0.03, 0.01, 0.0, 0.0),
        ..Default::default()
    });

    commands.insert_resource(SceneAssets {
        hero_mesh,
        enemy_mesh,
        tower_mesh,
        unit_bar_mesh,
        facing_indicator_mesh,
        ability_effect_mesh,
        hit_effect_mesh,
        shot_effect_mesh,
        hero_attack_cone_mesh,
        enemy_attack_cone_mesh,
        hero_local_material,
        hero_remote_material,
        enemy_material,
        tower_material,
        node_free_material: node_free_material.clone(),
        node_taken_material,
        bar_background_material,
        hero_health_bar_material,
        enemy_health_bar_material,
        hero_facing_indicator_material,
        enemy_facing_indicator_material,
        mana_bar_material,
        power_bar_material,
    });

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(
            game_shared::WORLD_HALF_WIDTH * 2.3,
            game_shared::WORLD_HALF_HEIGHT * 2.5,
        ))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.1, 0.14, 0.1),
            perceptual_roughness: 1.0,
            ..Default::default()
        })),
    ));

    for node in BUILD_NODES {
        commands.spawn((
            Mesh3d(meshes.add(Cylinder::new(0.45, 0.18))),
            MeshMaterial3d(node_free_material.clone()),
            Transform::from_translation(world_to_translation(node.pos, 0.1)),
            BuildNodeMarker {
                node_id: node.node_id,
            },
        ));
    }

    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(1.8, 2.2, 1.8))),
        MeshMaterial3d(objective_material),
        Transform::from_translation(world_to_translation(BASE_POSITION, 1.2)),
        ObjectiveMarker,
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 25_000.0,
            shadows_enabled: true,
            ..Default::default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -1.0, -0.65, 0.0)),
    ));

    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 42.0, 0.01).looking_at(Vec3::new(0.0, 0.0, 0.0), Vec3::Z),
        UnitBarCamera,
    ));

    let power_pie_raster_cache = build_power_pie_raster_cache(POWER_PIE_TEXTURE_SIZE);
    let power_pie_texture = images.add(build_power_pie_image(
        &power_pie_raster_cache,
        0.0,
        POWER_PIE_FILL_COLOR,
        POWER_PIE_EMPTY_COLOR,
    ));
    commands.insert_resource(PowerPieUiTexture {
        handle: power_pie_texture.clone(),
    });
    commands.insert_resource(power_pie_raster_cache);

    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(12.0),
            left: Val::Px(12.0),
            ..Default::default()
        },
        Text::new("connecting..."),
        TextColor(Color::WHITE),
        HudText,
    ));

    commands
        .spawn((
            MatchEndOverlayRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                display: Display::None,
                ..Default::default()
            },
            GlobalZIndex(32),
        ))
        .with_children(|parent| {
            parent.spawn((
                Node {
                    padding: UiRect::axes(Val::Px(26.0), Val::Px(16.0)),
                    border: UiRect::all(Val::Px(2.0)),
                    border_radius: BorderRadius::all(Val::Px(12.0)),
                    ..Default::default()
                },
                BorderColor::all(MATCH_END_OVERLAY_BORDER),
                BackgroundColor(MATCH_END_OVERLAY_BG),
                Text::new(""),
                TextFont {
                    font_size: 30.0,
                    ..Default::default()
                },
                TextLayout::new_with_justify(Justify::Center),
                TextColor(MATCH_END_OVERLAY_TITLE),
                MatchEndOverlayText,
            ));
        });

    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(SPECIAL_SKILL_ICON_LEFT_OFFSET),
                bottom: Val::Px(SPECIAL_SKILL_ICON_MARGIN),
                width: Val::Px(SPECIAL_SKILL_ICON_SIZE),
                height: Val::Px(SPECIAL_SKILL_ICON_SIZE),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border: UiRect::all(Val::Px(2.0)),
                border_radius: BorderRadius::all(Val::Px(7.0)),
                overflow: Overflow::clip(),
                ..Default::default()
            },
            BorderColor::all(SPECIAL_SKILL_ICON_BORDER),
            BackgroundColor(SPECIAL_SKILL_ICON_READY_BG),
            SpecialSkillIcon,
        ))
        .with_children(|parent| {
            parent.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    right: Val::Px(0.0),
                    bottom: Val::Px(0.0),
                    height: Val::Percent(0.0),
                    ..Default::default()
                },
                BackgroundColor(SPECIAL_SKILL_ICON_OVERLAY),
                Visibility::Hidden,
                SpecialSkillCooldownOverlay,
            ));
            parent.spawn((
                Text::new("K"),
                TextFont {
                    font_size: 34.0,
                    ..Default::default()
                },
                TextColor(SPECIAL_SKILL_ICON_KEY_COLOR),
                SpecialSkillKeyText,
            ));
        });

    let pie_inset = (POWER_PIE_SLOT_SIZE - POWER_PIE_BASE_SIZE) * 0.5;
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(POWER_PIE_LEFT_OFFSET),
                bottom: Val::Px(POWER_PIE_BOTTOM_OFFSET),
                width: Val::Px(POWER_PIE_SLOT_SIZE),
                height: Val::Px(POWER_PIE_SLOT_SIZE),
                ..Default::default()
            },
            PowerPieHudRoot,
        ))
        .with_children(|parent| {
            parent
                .spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(pie_inset),
                        bottom: Val::Px(pie_inset),
                        width: Val::Px(POWER_PIE_BASE_SIZE),
                        height: Val::Px(POWER_PIE_BASE_SIZE),
                        border: UiRect::all(Val::Px(2.0)),
                        border_radius: BorderRadius::all(Val::Px(9999.0)),
                        overflow: Overflow::clip(),
                        ..Default::default()
                    },
                    BorderColor::all(POWER_PIE_BORDER_COLOR),
                    BackgroundColor(POWER_PIE_BG_COLOR),
                    PowerPieCircle,
                ))
                .with_children(|circle| {
                    circle.spawn((
                        Node {
                            width: Val::Percent(100.0),
                            height: Val::Percent(100.0),
                            ..Default::default()
                        },
                        ImageNode::new(power_pie_texture.clone()),
                        PowerPieFillImage,
                    ));
                });
        });

    commands
        .spawn((
            GoldHudRoot,
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(GOLD_HUD_RIGHT_OFFSET),
                top: Val::Px(GOLD_HUD_TOP_OFFSET),
                padding: UiRect::axes(Val::Px(14.0), Val::Px(8.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(8.0)),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::FlexEnd,
                row_gap: Val::Px(2.0),
                ..Default::default()
            },
            BorderColor::all(GOLD_HUD_BORDER_COLOR),
            BackgroundColor(GOLD_HUD_PANEL_COLOR),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("GOLD"),
                TextFont {
                    font_size: 13.0,
                    ..Default::default()
                },
                TextColor(GOLD_HUD_LABEL_COLOR),
            ));
            parent.spawn((
                Text::new("0"),
                TextFont {
                    font_size: 30.0,
                    ..Default::default()
                },
                TextColor(GOLD_HUD_VALUE_COLOR),
                GoldHudValueText,
            ));
        });
}

fn setup_main_menu(mut commands: Commands) {
    commands
        .spawn((
            MainMenuRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..Default::default()
            },
        ))
        .with_children(|parent| {
            parent
                .spawn((
                    Node {
                        width: Val::Px(520.0),
                        padding: UiRect::all(Val::Px(18.0)),
                        border_radius: BorderRadius::all(Val::Px(10.0)),
                        row_gap: Val::Px(10.0),
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Stretch,
                        ..Default::default()
                    },
                    BackgroundColor(MENU_PANEL_COLOR),
                ))
                .with_children(|panel| {
                    panel.spawn((
                        Text::new("Discordium TD"),
                        TextFont {
                            font_size: 38.0,
                            ..Default::default()
                        },
                        TextColor(Color::srgb(0.93, 0.96, 0.98)),
                    ));

                    panel.spawn((
                        Text::new("Choose a connection mode"),
                        TextFont {
                            font_size: 17.0,
                            ..Default::default()
                        },
                        TextColor(Color::srgb(0.7, 0.78, 0.84)),
                    ));

                    #[cfg(not(target_arch = "wasm32"))]
                    panel
                        .spawn((
                            Button,
                            MainMenuButton(MenuAction::SinglePlayer),
                            Node {
                                width: Val::Percent(100.0),
                                height: Val::Px(54.0),
                                border: UiRect::all(Val::Px(1.0)),
                                border_radius: BorderRadius::all(Val::Px(8.0)),
                                justify_content: JustifyContent::Center,
                                align_items: AlignItems::Center,
                                margin: UiRect::top(Val::Px(8.0)),
                                ..Default::default()
                            },
                            BorderColor::all(Color::srgb(0.36, 0.44, 0.49)),
                            BackgroundColor(MENU_BUTTON_NORMAL),
                        ))
                        .with_children(|button| {
                            button.spawn((
                                Text::new("Single Player (Host + Join)"),
                                TextFont {
                                    font_size: 21.0,
                                    ..Default::default()
                                },
                                TextColor(Color::srgb(0.95, 0.97, 0.98)),
                            ));
                        });

                    panel
                        .spawn((
                            Button,
                            MainMenuButton(MenuAction::ConnectDev),
                            Node {
                                width: Val::Percent(100.0),
                                height: Val::Px(54.0),
                                border: UiRect::all(Val::Px(1.0)),
                                border_radius: BorderRadius::all(Val::Px(8.0)),
                                justify_content: JustifyContent::Center,
                                align_items: AlignItems::Center,
                                ..Default::default()
                            },
                            BorderColor::all(Color::srgb(0.36, 0.44, 0.49)),
                            BackgroundColor(MENU_BUTTON_NORMAL),
                        ))
                        .with_children(|button| {
                            button.spawn((
                                Text::new("Connect to Dev (localhost)"),
                                TextFont {
                                    font_size: 21.0,
                                    ..Default::default()
                                },
                                TextColor(Color::srgb(0.95, 0.97, 0.98)),
                            ));
                        });

                    panel.spawn((
                        Text::new("Select a mode to start."),
                        TextFont {
                            font_size: 15.0,
                            ..Default::default()
                        },
                        TextColor(Color::srgb(0.75, 0.83, 0.89)),
                        MainMenuStatusText,
                    ));
                });
        });
}

fn handle_menu_buttons(
    mut menu_state: ResMut<MainMenuState>,
    mut button_query: Query<
        (&Interaction, &MainMenuButton, &mut BackgroundColor),
        (Changed<Interaction>, With<Button>),
    >,
) {
    for (interaction, button_action, mut background_color) in &mut button_query {
        if menu_state.connect_request_in_flight {
            *background_color = MENU_BUTTON_DISABLED.into();
            continue;
        }

        match *interaction {
            Interaction::Pressed => {
                menu_state.pending_action = Some(button_action.0);
                menu_state.connect_request_in_flight = true;
                *background_color = MENU_BUTTON_DISABLED.into();
            }
            Interaction::Hovered => {
                *background_color = MENU_BUTTON_HOVERED.into();
            }
            Interaction::None => {
                *background_color = MENU_BUTTON_NORMAL.into();
            }
        }
    }
}

fn sync_menu_button_visual_state(
    menu_state: Res<MainMenuState>,
    mut buttons: Query<(&Interaction, &mut BackgroundColor), With<MainMenuButton>>,
) {
    if !menu_state.is_changed() {
        return;
    }

    for (interaction, mut background_color) in &mut buttons {
        *background_color = if menu_state.connect_request_in_flight {
            MENU_BUTTON_DISABLED
        } else {
            match *interaction {
                Interaction::Pressed => MENU_BUTTON_PRESSED,
                Interaction::Hovered => MENU_BUTTON_HOVERED,
                Interaction::None => MENU_BUTTON_NORMAL,
            }
        }
        .into();
    }
}

fn process_menu_actions(world: &mut World) {
    let action = {
        let mut menu_state = world.resource_mut::<MainMenuState>();
        if !menu_state.connect_request_in_flight {
            return;
        }
        menu_state.pending_action.take()
    };
    let Some(action) = action else {
        return;
    };

    match action {
        MenuAction::ConnectDev => {
            let http_base = world.resource::<ClientArgs>().http_base.clone();
            set_menu_status(
                world,
                format!("Connecting to dev server at {http_base} ..."),
            );
            connect_to_bootstrap(world, &http_base, 1);
        }
        #[cfg(not(target_arch = "wasm32"))]
        MenuAction::SinglePlayer => {
            let Some((local_http_base, public_http_base)) = ensure_local_server_running(world)
            else {
                return;
            };

            set_menu_status(
                world,
                format!(
                    "Local host active. Share {public_http_base} and connect peers. Joining ..."
                ),
            );
            connect_to_bootstrap(world, &local_http_base, 40);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn ensure_local_server_running(world: &mut World) -> Option<(String, String)> {
    if let Some(hosted) = world.resource::<MainMenuState>().hosted_server.clone() {
        return Some((hosted.local_http_base, hosted.public_http_base));
    }

    let host_ip = detect_lan_ip().unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    let local_http_base = format!("http://127.0.0.1:{LOCAL_HOST_HTTP_PORT}");
    let public_http_base = format!("http://{host_ip}:{LOCAL_HOST_HTTP_PORT}");

    let server_args = ServerArgs::parse_from([
        "embedded-server".to_owned(),
        "--http-bind".to_owned(),
        format!("0.0.0.0:{LOCAL_HOST_HTTP_PORT}"),
        "--udp-bind".to_owned(),
        format!("0.0.0.0:{LOCAL_HOST_UDP_PORT}"),
        "--webrtc-bind".to_owned(),
        format!("0.0.0.0:{LOCAL_HOST_WEBRTC_PORT}"),
        "--public-http-base".to_owned(),
        public_http_base.clone(),
        "--public-udp-addr".to_owned(),
        format!("{host_ip}:{LOCAL_HOST_UDP_PORT}"),
        "--public-webrtc-addr".to_owned(),
        format!("{host_ip}:{LOCAL_HOST_WEBRTC_PORT}"),
    ]);

    if let Err(err) = thread::Builder::new()
        .name("discordium-local-server".to_owned())
        .spawn(move || {
            if let Err(err) = run_server(server_args) {
                log::error!("embedded local server exited with error: {err}");
            }
        })
    {
        let message = format!("Failed to spawn local server thread: {err}");
        set_menu_status(world, message);
        return None;
    }

    world.resource_mut::<MainMenuState>().hosted_server = Some(LocalHostedServer {
        local_http_base: local_http_base.clone(),
        public_http_base: public_http_base.clone(),
    });

    Some((local_http_base, public_http_base))
}

#[cfg(not(target_arch = "wasm32"))]
fn connect_to_bootstrap(world: &mut World, http_base: &str, attempts: u32) {
    if let Some(mut existing) = world.remove_non_send_resource::<NetworkRuntime>() {
        graceful_disconnect_runtime(&mut existing, "reconnect");
    }

    let mut last_error = None;
    for attempt in 1..=attempts.max(1) {
        match renet_cross::connect_via_session_http_blocking(
            http_base,
            PROTOCOL_ID,
            renet_cross::NativeConnectOptions::default(),
        ) {
            Ok((renet, transport, client_id)) => {
                on_bootstrap_connected(world, http_base, client_id, renet, transport);
                return;
            }
            Err(err) => {
                last_error = Some(err.to_string());
                if attempt < attempts {
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }

    let message = format!(
        "Connect failed via {http_base}: {}",
        last_error.unwrap_or_else(|| "unknown error".to_owned())
    );
    on_bootstrap_failed(world, &message);
}

#[cfg(target_arch = "wasm32")]
fn connect_to_bootstrap(world: &mut World, http_base: &str, _attempts: u32) {
    if let Some(mut existing) = world.remove_non_send_resource::<NetworkRuntime>() {
        graceful_disconnect_runtime(&mut existing, "reconnect");
    }
    world.remove_non_send_resource::<PendingWebBootstrap>();

    let http_base = http_base.trim_end_matches('/').to_owned();
    let slot = std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot_clone = std::rc::Rc::clone(&slot);
    let request_base = http_base.clone();
    spawn_local(async move {
        let result = renet_cross::connect_via_sdp_http(&request_base, PROTOCOL_ID)
            .await
            .map_err(|err| err.to_string());
        *slot_clone.borrow_mut() = Some(result);
    });
    world.insert_non_send_resource(PendingWebBootstrap { slot });
    set_menu_status(
        world,
        format!("Connecting via {http_base} (WebRTC bootstrap)..."),
    );
}

fn on_bootstrap_connected(
    world: &mut World,
    http_base: &str,
    client_id: u64,
    renet: RenetClient,
    transport: ClientTransport,
) {
    world.insert_non_send_resource(NetworkRuntime::new(client_id, renet, transport));
    world.resource_mut::<WorldView>().you = Some(client_id);
    world
        .resource_mut::<MainMenuState>()
        .connect_request_in_flight = false;
    set_menu_status(
        world,
        format!("Connected via {http_base}. Waiting for join snapshot..."),
    );
    log::info!("bootstrap session created with client_id={client_id}");
}

fn on_bootstrap_failed(world: &mut World, message: &str) {
    world
        .resource_mut::<MainMenuState>()
        .connect_request_in_flight = false;
    log::error!("{message}");
    set_menu_status(world, message.to_owned());
}

#[cfg(target_arch = "wasm32")]
fn poll_web_bootstrap(world: &mut World) {
    let result = {
        let Some(pending) = world.get_non_send_resource_mut::<PendingWebBootstrap>() else {
            return;
        };
        pending.slot.borrow_mut().take()
    };
    let Some(result) = result else {
        return;
    };

    match result {
        Ok((renet, transport, client_id)) => {
            let http_base = world.resource::<ClientArgs>().http_base.clone();
            on_bootstrap_connected(world, &http_base, client_id, renet, transport);
        }
        Err(err) => {
            let http_base = world.resource::<ClientArgs>().http_base.clone();
            on_bootstrap_failed(world, &format!("Connect failed via {http_base}: {err}"));
        }
    }
    world.remove_non_send_resource::<PendingWebBootstrap>();
}

fn graceful_disconnect_runtime(runtime: &mut NetworkRuntime, reason: &str) {
    if runtime.renet.disconnect_reason().is_some() {
        return;
    }

    runtime.renet.disconnect();
    if let Err(err) = runtime.transport_update(Duration::ZERO) {
        log::warn!("failed to send disconnect packet during {reason}: {err}");
    } else {
        log::info!("sent graceful disconnect packet during {reason}");
    }
}

fn graceful_disconnect_on_app_exit(
    mut app_exit_reader: MessageReader<AppExit>,
    runtime: Option<NonSendMut<NetworkRuntime>>,
) {
    if app_exit_reader.read().next().is_none() {
        return;
    }

    let Some(mut runtime) = runtime else {
        return;
    };
    graceful_disconnect_runtime(&mut runtime, "app exit");
}

fn set_menu_status(world: &mut World, status: String) {
    world.resource_mut::<MainMenuState>().status = status.clone();
    world.resource_mut::<HudState>().last_event = status;
}

#[cfg(not(target_arch = "wasm32"))]
fn detect_lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))).ok()?;
    socket
        .connect(SocketAddr::from((Ipv4Addr::new(8, 8, 8, 8), 80)))
        .ok()?;
    Some(socket.local_addr().ok()?.ip())
}

#[cfg(target_arch = "wasm32")]
fn default_http_base() -> String {
    if let Some(configured) = option_env!("TD_WEB_HTTP_BASE") {
        let trimmed = configured.trim();
        if !trimmed.is_empty() {
            return trimmed.trim_end_matches('/').to_owned();
        }
    }

    web_sys::window()
        .and_then(|window| window.location().origin().ok())
        .unwrap_or_else(|| "http://127.0.0.1:8080".to_owned())
}

fn sync_main_menu_state(
    runtime: Option<NonSend<NetworkRuntime>>,
    menu_state: Res<MainMenuState>,
    mut menu_root: Single<&mut Node, With<MainMenuRoot>>,
    mut status_text: Single<&mut Text, With<MainMenuStatusText>>,
) {
    let runtime_active = runtime
        .as_ref()
        .map(|runtime| runtime.renet.is_connected() || runtime.renet.is_connecting())
        .unwrap_or(false);

    menu_root.display = if runtime_active {
        Display::None
    } else {
        Display::Flex
    };
    status_text.0 = menu_state.status.clone();
}

fn capture_input(keyboard: Res<ButtonInput<KeyCode>>, mut input_state: ResMut<InputState>) {
    let mut input = [0.0, 0.0];
    if keyboard.pressed(KeyCode::KeyW) {
        input[1] += 1.0;
    }
    if keyboard.pressed(KeyCode::KeyS) {
        input[1] -= 1.0;
    }
    if keyboard.pressed(KeyCode::KeyA) {
        input[0] -= 1.0;
    }
    if keyboard.pressed(KeyCode::KeyD) {
        input[0] += 1.0;
    }

    // Top-down camera view is currently mirrored on screen X relative to world X.
    // Flip horizontal input so `A` is screen-left and `D` is screen-right.
    input_state.dir = normalize_or_zero([-input[0], input[1]]);
}

fn send_movement_commands(
    input_state: Res<InputState>,
    runtime: Option<NonSendMut<NetworkRuntime>>,
) {
    let Some(mut runtime) = runtime else {
        return;
    };

    if !runtime.renet.is_connected() {
        return;
    }

    let seq = runtime.next_command_seq();
    let command = ClientCommand::Move {
        seq,
        dir: input_state.dir,
    };
    runtime
        .renet
        .send_message(DefaultChannel::Unreliable, encode(&command));
    runtime.pending_moves.push_back(PendingMove {
        seq,
        dir: input_state.dir,
    });
    while runtime.pending_moves.len() > 256 {
        runtime.pending_moves.pop_front();
    }
}

fn send_action_commands(
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    runtime: Option<NonSendMut<NetworkRuntime>>,
    world: Res<WorldView>,
    towers: Query<&TowerBuildNode, With<TowerActor>>,
    mut hud_state: ResMut<HudState>,
) {
    let Some(mut runtime) = runtime else {
        return;
    };

    if !runtime.renet.is_connected() {
        return;
    }

    if keyboard.just_pressed(KeyCode::KeyQ) {
        let Some(hero) = world.heroes.get(&runtime.client_id) else {
            return;
        };

        let next_target = select_next_lock_target(
            hero.lock_target_id,
            hero.pos,
            world.enemies.values().map(|enemy| (enemy.id, enemy.pos)),
        );

        if let Some(target_id) = next_target {
            let seq = runtime.next_command_seq();
            runtime.renet.send_message(
                DefaultChannel::ReliableOrdered,
                encode(&ClientCommand::SetLockTarget {
                    seq,
                    target_id: Some(target_id),
                }),
            );
            hud_state.last_event = format!("Locked enemy {target_id}");
        } else if hero.lock_mode_active {
            hud_state.last_event = "Lock mode active (no targets)".to_owned();
        } else {
            hud_state.last_event = "No valid lock target".to_owned();
        }
    }

    if keyboard.just_pressed(KeyCode::KeyE) {
        let seq = runtime.next_command_seq();
        runtime.renet.send_message(
            DefaultChannel::ReliableOrdered,
            encode(&ClientCommand::SetLockTarget {
                seq,
                target_id: None,
            }),
        );
        hud_state.last_event = "Lock cleared".to_owned();
    }

    if mouse.just_pressed(MouseButton::Left) || keyboard.just_pressed(KeyCode::KeyJ) {
        let seq = runtime.next_command_seq();
        runtime.renet.send_message(
            DefaultChannel::ReliableOrdered,
            encode(&ClientCommand::BasicAttack { seq }),
        );
    }

    if keyboard.just_pressed(KeyCode::KeyK) {
        let seq = runtime.next_command_seq();
        runtime.renet.send_message(
            DefaultChannel::ReliableOrdered,
            encode(&ClientCommand::CastAbility {
                seq,
                ability: AbilityId::ArcBurst,
            }),
        );
    }

    if keyboard.just_pressed(KeyCode::Space) {
        let seq = runtime.next_command_seq();
        runtime.renet.send_message(
            DefaultChannel::ReliableOrdered,
            encode(&ClientCommand::SetCharging { seq, active: true }),
        );
        hud_state.last_event = "Charge started".to_owned();
    }

    if keyboard.just_released(KeyCode::Space) {
        let seq = runtime.next_command_seq();
        runtime.renet.send_message(
            DefaultChannel::ReliableOrdered,
            encode(&ClientCommand::SetCharging { seq, active: false }),
        );
        hud_state.last_event = "Charge released".to_owned();
    }

    if keyboard.just_pressed(KeyCode::KeyB) {
        let Some(node_id) = pick_nearest_build_node(&world, runtime.client_id, &towers) else {
            hud_state.last_event =
                "No available build node in range. Move closer to a green node.".to_owned();
            return;
        };

        let seq = runtime.next_command_seq();
        runtime.renet.send_message(
            DefaultChannel::ReliableOrdered,
            encode(&ClientCommand::BuildTower {
                seq,
                node_id,
                tower_type: TowerType::Arrow,
            }),
        );
    }
}

fn network_update(
    time: Res<Time>,
    runtime: Option<NonSendMut<NetworkRuntime>>,
    mut world: ResMut<WorldView>,
    mut hud_state: ResMut<HudState>,
    render_index: Res<RenderIndex>,
    mut commands: Commands,
    scene_assets: Res<SceneAssets>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some(mut runtime) = runtime else {
        return;
    };
    let runtime_inner: &mut NetworkRuntime = &mut runtime;

    let dt = Duration::from_secs_f32(time.delta_secs().clamp(0.0, 0.1));
    runtime_inner.renet.update(dt);
    if let Err(err) = runtime_inner.transport_update(dt) {
        log::warn!("client transport update failed: {err}");
    }

    while let Some(bytes) = runtime_inner
        .renet
        .receive_message(DefaultChannel::ReliableOrdered)
    {
        match game_shared::decode::<ReliableServerMessage>(&bytes) {
            Ok(ReliableServerMessage::JoinSnapshot(snapshot)) => {
                acknowledge_pending_moves(
                    &mut runtime_inner.pending_moves,
                    snapshot.world.your_last_input_seq,
                );
                runtime_inner.client_id = snapshot.you;
                world.apply_join(snapshot);
                hud_state.last_event = "Joined authoritative match".to_owned();
            }
            Ok(ReliableServerMessage::Event(event)) => {
                match &event {
                    ReliableGameEvent::AbilityCast { pos, radius, .. } => {
                        spawn_ability_effect(
                            &mut commands,
                            &mut materials,
                            &scene_assets,
                            *pos,
                            *radius,
                        );
                    }
                    _ => {}
                }
                hud_state.last_event = describe_event(event, runtime_inner.client_id);
            }
            Err(err) => {
                log::warn!("failed to decode reliable message: {err}");
            }
        }
    }

    while let Some(bytes) = runtime_inner
        .renet
        .receive_message(DefaultChannel::Unreliable)
    {
        match game_shared::decode::<WorldDelta>(&bytes) {
            Ok(delta) => {
                let hit_enemy_ids = detect_hit_enemy_ids(&world.enemies, &delta);
                let hit_hero_ids = detect_hit_hero_ids(&world.heroes, &delta);
                let fired_tower_ids = detect_fired_tower_ids(&world.towers, &delta);
                let attack_starts = detect_regular_attack_starts(&world, &delta);

                for attack in attack_starts {
                    spawn_regular_attack_effect(
                        &mut commands,
                        &mut materials,
                        &scene_assets,
                        attack.pos,
                        attack.facing,
                        attack.range,
                        attack.enemy,
                    );
                }

                for enemy in &delta.enemies {
                    if !hit_enemy_ids.contains(&enemy.id) {
                        continue;
                    }

                    spawn_hit_effect(&mut commands, &mut materials, &scene_assets, enemy.pos);
                    if let Some(entity) = render_index.by_id.get(&enemy.id) {
                        commands.entity(*entity).insert(EnemyHitReaction::default());
                    }
                }

                for hero in &delta.heroes {
                    if !hit_hero_ids.contains(&hero.client_id) {
                        continue;
                    }

                    spawn_hit_effect(&mut commands, &mut materials, &scene_assets, hero.pos);
                    if let Some(entity) = render_index.by_id.get(&hero.client_id) {
                        commands.entity(*entity).insert(HeroHitReaction::default());
                    }
                }

                for tower in &delta.towers {
                    if !fired_tower_ids.contains(&tower.id) {
                        continue;
                    }

                    if let Some(entity) = render_index.by_id.get(&tower.id) {
                        commands
                            .entity(*entity)
                            .insert(TowerFireReaction::default());
                    }

                    if let Some(target_pos) =
                        pick_tower_shot_target(tower, &delta.enemies, &hit_enemy_ids)
                    {
                        spawn_tower_shot_effect(
                            &mut commands,
                            &mut materials,
                            &scene_assets,
                            tower.pos,
                            target_pos,
                        );
                    }
                }

                acknowledge_pending_moves(
                    &mut runtime_inner.pending_moves,
                    delta.your_last_input_seq,
                );
                world.apply_delta(delta);
            }
            Err(err) => {
                log::warn!("failed to decode WorldDelta: {err}");
            }
        }
    }

    if let Err(err) = runtime_inner.transport_send_packets() {
        log::warn!("client send_packets error: {err}");
    }
}

fn spawn_ability_effect(
    commands: &mut Commands,
    materials: &mut Assets<StandardMaterial>,
    scene_assets: &SceneAssets,
    pos: [f32; 2],
    radius: f32,
) {
    let material = materials.add(StandardMaterial {
        base_color: Color::srgba(0.25, 0.88, 1.0, 0.46),
        emissive: LinearRgba::new(0.08, 0.4, 0.6, 0.0),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        cull_mode: None,
        ..Default::default()
    });

    commands.spawn((
        Mesh3d(scene_assets.ability_effect_mesh.clone()),
        MeshMaterial3d(material.clone()),
        Transform::from_translation(world_to_translation(pos, 0.12)).with_scale(Vec3::new(
            ABILITY_EFFECT_START_RADIUS,
            1.0,
            ABILITY_EFFECT_START_RADIUS,
        )),
        AbilityEffectVisual {
            age_seconds: 0.0,
            duration_seconds: ABILITY_EFFECT_DURATION_SECONDS,
            max_radius: radius.max(0.0),
            material,
        },
    ));
}

fn spawn_hit_effect(
    commands: &mut Commands,
    materials: &mut Assets<StandardMaterial>,
    scene_assets: &SceneAssets,
    pos: [f32; 2],
) {
    let material = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.34, 0.22, 0.92),
        emissive: LinearRgba::new(0.85, 0.18, 0.05, 0.0),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        cull_mode: None,
        ..Default::default()
    });

    commands.spawn((
        Mesh3d(scene_assets.hit_effect_mesh.clone()),
        MeshMaterial3d(material.clone()),
        Transform::from_translation(world_to_translation(pos, 0.52))
            .with_scale(Vec3::splat(HIT_EFFECT_START_SCALE)),
        HitEffectVisual {
            age_seconds: 0.0,
            duration_seconds: HIT_EFFECT_DURATION_SECONDS,
            material,
        },
    ));
}

fn spawn_regular_attack_effect(
    commands: &mut Commands,
    materials: &mut Assets<StandardMaterial>,
    scene_assets: &SceneAssets,
    pos: [f32; 2],
    facing: [f32; 2],
    range: f32,
    enemy: bool,
) {
    let scene_dir = world_to_scene_plane(normalize_or_zero(facing));
    if scene_dir[0] == 0.0 && scene_dir[1] == 0.0 {
        return;
    }

    let dir = Vec3::new(scene_dir[0], 0.0, scene_dir[1]).normalize();
    let rotation = Quat::from_rotation_arc(Vec3::Z, dir);
    let cone_mesh = if enemy {
        scene_assets.enemy_attack_cone_mesh.clone()
    } else {
        scene_assets.hero_attack_cone_mesh.clone()
    };
    let range = range.max(0.0);

    let (base_color, emissive) = if enemy {
        (
            Color::srgba(1.0, 0.5, 0.33, 0.38),
            LinearRgba::new(0.7, 0.2, 0.06, 0.0),
        )
    } else {
        (
            Color::srgba(0.35, 0.92, 1.0, 0.38),
            LinearRgba::new(0.11, 0.42, 0.55, 0.0),
        )
    };

    let material = materials.add(StandardMaterial {
        base_color,
        emissive,
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        cull_mode: None,
        ..Default::default()
    });

    commands.spawn((
        Mesh3d(cone_mesh),
        MeshMaterial3d(material.clone()),
        Transform::from_translation(world_to_translation(pos, REGULAR_ATTACK_EFFECT_Y))
            .with_rotation(rotation)
            .with_scale(Vec3::new(range, 1.0, range)),
        RegularAttackEffectVisual {
            age_seconds: 0.0,
            duration_seconds: REGULAR_ATTACK_EFFECT_DURATION_SECONDS,
            material,
        },
    ));
}

fn build_unit_attack_cone_mesh(arc_dot_threshold: f32, segments: usize) -> Mesh {
    let segments = segments.max(3);
    let half_angle = arc_dot_threshold.clamp(-1.0, 1.0).acos();
    let sweep = half_angle * 2.0;

    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(segments * 3);
    let mut normals: Vec<[f32; 3]> = Vec::with_capacity(segments * 3);
    let mut uvs: Vec<[f32; 2]> = Vec::with_capacity(segments * 3);

    for i in 0..segments {
        let t0 = i as f32 / segments as f32;
        let t1 = (i + 1) as f32 / segments as f32;
        let a0 = -half_angle + sweep * t0;
        let a1 = -half_angle + sweep * t1;

        // Unit-radius cone sector on XZ plane, facing +Z.
        let p0 = [a0.sin(), 0.0, a0.cos()];
        let p1 = [a1.sin(), 0.0, a1.cos()];
        let center = [0.0, 0.0, 0.0];

        positions.push(center);
        positions.push(p0);
        positions.push(p1);

        normals.push([0.0, 1.0, 0.0]);
        normals.push([0.0, 1.0, 0.0]);
        normals.push([0.0, 1.0, 0.0]);

        uvs.push([0.5, 0.0]);
        uvs.push([0.5 + p0[0] * 0.5, p0[2]]);
        uvs.push([0.5 + p1[0] * 0.5, p1[2]]);
    }

    let mut mesh = Mesh::new(
        bevy::mesh::PrimitiveTopology::TriangleList,
        bevy::asset::RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh
}

#[derive(Clone, Copy)]
struct RegularAttackStart {
    pos: [f32; 2],
    facing: [f32; 2],
    range: f32,
    enemy: bool,
}

fn detect_hit_enemy_ids(
    previous_enemies: &HashMap<u64, EnemySnapshot>,
    delta: &WorldDelta,
) -> HashSet<u64> {
    let mut hit_enemy_ids = HashSet::new();
    for enemy in &delta.enemies {
        let Some(previous) = previous_enemies.get(&enemy.id) else {
            continue;
        };

        if enemy.hp + 0.05 < previous.hp {
            hit_enemy_ids.insert(enemy.id);
        }
    }
    hit_enemy_ids
}

fn detect_hit_hero_ids(
    previous_heroes: &HashMap<u64, HeroSnapshot>,
    delta: &WorldDelta,
) -> HashSet<u64> {
    let mut hit_hero_ids = HashSet::new();
    for hero in &delta.heroes {
        let Some(previous) = previous_heroes.get(&hero.client_id) else {
            continue;
        };

        if hero.hp + 0.05 < previous.hp {
            hit_hero_ids.insert(hero.client_id);
        }
    }
    hit_hero_ids
}

fn detect_regular_attack_starts(world: &WorldView, delta: &WorldDelta) -> Vec<RegularAttackStart> {
    let mut starts = Vec::new();

    for hero in &delta.heroes {
        let previous_phase = world
            .heroes
            .get(&hero.client_id)
            .map(|previous| previous.regular_attack.phase)
            .unwrap_or(AttackPhase::Ready);
        if previous_phase != AttackPhase::Windup && hero.regular_attack.phase == AttackPhase::Windup
        {
            starts.push(RegularAttackStart {
                pos: hero.pos,
                facing: hero.facing.dir,
                range: hero_regular_attack_effect_range(hero),
                enemy: false,
            });
        }
    }

    for enemy in &delta.enemies {
        let previous_phase = world
            .enemies
            .get(&enemy.id)
            .map(|previous| previous.regular_attack.phase)
            .unwrap_or(AttackPhase::Ready);
        if previous_phase != AttackPhase::Windup
            && enemy.regular_attack.phase == AttackPhase::Windup
        {
            starts.push(RegularAttackStart {
                pos: enemy.pos,
                facing: enemy.facing.dir,
                range: ENEMY_REGULAR_ATTACK.range,
                enemy: true,
            });
        }
    }

    starts
}

fn hero_regular_attack_effect_range(hero: &HeroSnapshot) -> f32 {
    let multiplier = if hero.charge_state.power_active {
        hero.powered_modifiers
            .regular_attack_range_multiplier
            .max(0.0)
    } else {
        1.0
    };
    HERO_REGULAR_ATTACK.range * multiplier
}

fn detect_fired_tower_ids(
    previous_towers: &HashMap<u64, TowerSnapshot>,
    delta: &WorldDelta,
) -> HashSet<u64> {
    let mut fired_tower_ids = HashSet::new();
    for tower in &delta.towers {
        let Some(previous) = previous_towers.get(&tower.id) else {
            continue;
        };

        if tower.reload_ticks_remaining > previous.reload_ticks_remaining {
            fired_tower_ids.insert(tower.id);
        }
    }
    fired_tower_ids
}

fn pick_tower_shot_target(
    tower: &TowerSnapshot,
    enemies: &[EnemySnapshot],
    hit_enemy_ids: &HashSet<u64>,
) -> Option<[f32; 2]> {
    let mut best_hit: Option<([f32; 2], f32)> = None;
    let mut best_any: Option<([f32; 2], f32)> = None;

    for enemy in enemies {
        let dist_sq = distance_sq(enemy.pos, tower.pos);
        match best_any {
            Some((_, best_dist_sq)) if dist_sq >= best_dist_sq => {}
            _ => {
                best_any = Some((enemy.pos, dist_sq));
            }
        }

        if !hit_enemy_ids.contains(&enemy.id) {
            continue;
        }

        match best_hit {
            Some((_, best_dist_sq)) if dist_sq >= best_dist_sq => {}
            _ => {
                best_hit = Some((enemy.pos, dist_sq));
            }
        }
    }

    best_hit.or(best_any).map(|(pos, _)| pos)
}

fn spawn_tower_shot_effect(
    commands: &mut Commands,
    materials: &mut Assets<StandardMaterial>,
    scene_assets: &SceneAssets,
    tower_pos: [f32; 2],
    enemy_pos: [f32; 2],
) {
    let start = world_to_translation(tower_pos, 0.82);
    let end = world_to_translation(enemy_pos, 0.56);
    let segment = end - start;
    let len = segment.length();
    if len <= 0.01 {
        return;
    }

    let direction = segment / len;
    let rotation = Quat::from_rotation_arc(Vec3::X, direction);
    let material = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.92, 0.35, 0.88),
        emissive: LinearRgba::new(0.75, 0.55, 0.08, 0.0),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        cull_mode: None,
        ..Default::default()
    });

    commands.spawn((
        Mesh3d(scene_assets.shot_effect_mesh.clone()),
        MeshMaterial3d(material.clone()),
        Transform::from_translation((start + end) * 0.5)
            .with_rotation(rotation)
            .with_scale(Vec3::new(len, 1.0, 1.0)),
        TowerShotEffectVisual {
            age_seconds: 0.0,
            duration_seconds: TOWER_SHOT_EFFECT_DURATION_SECONDS,
            material,
        },
    ));
}

fn update_ability_effects(
    mut commands: Commands,
    time: Res<Time>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut effects: Query<(Entity, &mut AbilityEffectVisual, &mut Transform)>,
) {
    let dt = time.delta_secs();

    for (entity, mut effect, mut transform) in &mut effects {
        effect.age_seconds += dt;
        let t = (effect.age_seconds / effect.duration_seconds).clamp(0.0, 1.0);

        let radius =
            ABILITY_EFFECT_START_RADIUS + (effect.max_radius - ABILITY_EFFECT_START_RADIUS) * t;
        transform.scale.x = radius;
        transform.scale.z = radius;
        transform.translation.y = 0.12 + 0.12 * t;

        if let Some(material) = materials.get_mut(&effect.material) {
            let alpha = 0.46 * (1.0 - t);
            material.base_color = Color::srgba(0.25, 0.88, 1.0, alpha);
            material.emissive =
                LinearRgba::new(0.08 * (1.0 - t), 0.4 * (1.0 - t), 0.6 * (1.0 - t), 0.0);
        }

        if effect.age_seconds >= effect.duration_seconds {
            commands.entity(entity).despawn();
        }
    }
}

fn update_hit_effects(
    mut commands: Commands,
    time: Res<Time>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut effects: Query<(Entity, &mut HitEffectVisual, &mut Transform)>,
) {
    let dt = time.delta_secs();

    for (entity, mut effect, mut transform) in &mut effects {
        effect.age_seconds += dt;
        let t = (effect.age_seconds / effect.duration_seconds).clamp(0.0, 1.0);

        let scale = HIT_EFFECT_START_SCALE + (1.05 - HIT_EFFECT_START_SCALE) * t;
        transform.scale = Vec3::splat(scale);
        transform.translation.y = 0.52 + 0.18 * t;

        if let Some(material) = materials.get_mut(&effect.material) {
            let alpha = 0.92 * (1.0 - t);
            material.base_color = Color::srgba(1.0, 0.34, 0.22, alpha);
            material.emissive =
                LinearRgba::new(0.85 * (1.0 - t), 0.18 * (1.0 - t), 0.05 * (1.0 - t), 0.0);
        }

        if effect.age_seconds >= effect.duration_seconds {
            commands.entity(entity).despawn();
        }
    }
}

fn update_tower_shot_effects(
    mut commands: Commands,
    time: Res<Time>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut effects: Query<(Entity, &mut TowerShotEffectVisual, &mut Transform)>,
) {
    let dt = time.delta_secs();

    for (entity, mut effect, mut transform) in &mut effects {
        effect.age_seconds += dt;
        let t = (effect.age_seconds / effect.duration_seconds).clamp(0.0, 1.0);

        transform.scale.y = 1.0 - 0.45 * t;
        transform.scale.z = 1.0 - 0.45 * t;

        if let Some(material) = materials.get_mut(&effect.material) {
            let alpha = 0.88 * (1.0 - t);
            material.base_color = Color::srgba(1.0, 0.92, 0.35, alpha);
            material.emissive =
                LinearRgba::new(0.75 * (1.0 - t), 0.55 * (1.0 - t), 0.08 * (1.0 - t), 0.0);
        }

        if effect.age_seconds >= effect.duration_seconds {
            commands.entity(entity).despawn();
        }
    }
}

fn update_regular_attack_effects(
    mut commands: Commands,
    time: Res<Time>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut effects: Query<(Entity, &mut RegularAttackEffectVisual, &mut Transform)>,
) {
    let dt = time.delta_secs();

    for (entity, mut effect, mut transform) in &mut effects {
        effect.age_seconds += dt;
        let t = (effect.age_seconds / effect.duration_seconds).clamp(0.0, 1.0);

        transform.translation.y = REGULAR_ATTACK_EFFECT_Y + 0.07 * t;

        if let Some(material) = materials.get_mut(&effect.material) {
            let alpha = 0.5 * (1.0 - t);
            let base = material.base_color.to_srgba();
            material.base_color = Color::srgba(base.red, base.green, base.blue, alpha);
            material.emissive = LinearRgba::new(
                material.emissive.red * (1.0 - t),
                material.emissive.green * (1.0 - t),
                material.emissive.blue * (1.0 - t),
                0.0,
            );
        }

        if effect.age_seconds >= effect.duration_seconds {
            commands.entity(entity).despawn();
        }
    }
}

fn update_hero_hit_reactions(
    mut commands: Commands,
    time: Res<Time>,
    mut heroes: Query<(Entity, &mut HeroHitReaction, &mut Transform), With<HeroActor>>,
) {
    let dt = time.delta_secs();

    for (entity, mut reaction, mut transform) in &mut heroes {
        reaction.age_seconds += dt;
        let t = (reaction.age_seconds / HERO_HIT_REACTION_DURATION_SECONDS).clamp(0.0, 1.0);
        let pulse = 1.0 - (2.0 * t - 1.0).abs();
        let stretch = 1.0 + 0.26 * pulse;
        let squash = 1.0 - 0.14 * pulse;

        transform.scale = Vec3::new(stretch, squash, stretch);

        if reaction.age_seconds >= HERO_HIT_REACTION_DURATION_SECONDS {
            transform.scale = Vec3::ONE;
            commands.entity(entity).remove::<HeroHitReaction>();
        }
    }
}

fn update_enemy_hit_reactions(
    mut commands: Commands,
    time: Res<Time>,
    mut enemies: Query<(Entity, &mut EnemyHitReaction, &mut Transform), With<EnemyActor>>,
) {
    let dt = time.delta_secs();

    for (entity, mut reaction, mut transform) in &mut enemies {
        reaction.age_seconds += dt;
        let t = (reaction.age_seconds / ENEMY_HIT_REACTION_DURATION_SECONDS).clamp(0.0, 1.0);
        let pulse = 1.0 - (2.0 * t - 1.0).abs();
        let stretch = 1.0 + 0.32 * pulse;
        let squash = 1.0 - 0.18 * pulse;

        transform.scale = Vec3::new(stretch, squash, stretch);

        if reaction.age_seconds >= ENEMY_HIT_REACTION_DURATION_SECONDS {
            transform.scale = Vec3::ONE;
            commands.entity(entity).remove::<EnemyHitReaction>();
        }
    }
}

fn update_tower_fire_reactions(
    mut commands: Commands,
    time: Res<Time>,
    mut towers: Query<(Entity, &mut TowerFireReaction, &mut Transform), With<TowerActor>>,
) {
    let dt = time.delta_secs();

    for (entity, mut reaction, mut transform) in &mut towers {
        reaction.age_seconds += dt;
        let t = (reaction.age_seconds / TOWER_FIRE_REACTION_DURATION_SECONDS).clamp(0.0, 1.0);
        let kick = 1.0 - (2.0 * t - 1.0).abs();

        transform.scale = Vec3::new(1.0 + 0.18 * kick, 1.0 - 0.11 * kick, 1.0 + 0.18 * kick);

        if reaction.age_seconds >= TOWER_FIRE_REACTION_DURATION_SECONDS {
            transform.scale = Vec3::ONE;
            commands.entity(entity).remove::<TowerFireReaction>();
        }
    }
}

fn update_actor_facing_and_attack_visuals(
    time: Res<Time>,
    mut actors: Query<
        (
            &FacingStat,
            Option<&RegularAttackStat>,
            Option<&ChargeStateStat>,
            Option<&HeroHitReaction>,
            Option<&EnemyHitReaction>,
            Option<&TowerFireReaction>,
            &mut Transform,
        ),
        With<DynamicActor>,
    >,
) {
    let dt = time.delta_secs();
    let turn_alpha = (1.0 - (-FACING_TURN_SMOOTH_RATE * dt).exp()).clamp(0.0, 1.0);
    let scale_alpha = (1.0 - (-ATTACK_COMMIT_VISUAL_SMOOTH_RATE * dt).exp()).clamp(0.0, 1.0);

    for (facing, attack, charge_state, hero_hit, enemy_hit, tower_fire, mut transform) in
        &mut actors
    {
        if let Some(target_yaw) = facing_to_scene_yaw(facing.dir) {
            let target_rot = Quat::from_rotation_y(target_yaw);
            transform.rotation = transform.rotation.slerp(target_rot, turn_alpha);
        }

        if hero_hit.is_some() || enemy_hit.is_some() || tower_fire.is_some() {
            continue;
        }

        if let Some(charge_state) = charge_state {
            let amplitude = match charge_state.phase {
                ChargePhase::Idle => {
                    if charge_state.power_active {
                        0.06
                    } else {
                        0.0
                    }
                }
                ChargePhase::Startup => CHARGE_PULSE_STARTUP_AMPLITUDE,
                ChargePhase::Charging => CHARGE_PULSE_ACTIVE_AMPLITUDE,
                ChargePhase::Recovery => CHARGE_PULSE_RECOVERY_AMPLITUDE,
            };

            if amplitude > 0.0 {
                let pulse = (time.elapsed_secs() * CHARGE_PULSE_SPEED).sin().abs();
                let stretch = 1.0 + amplitude * pulse;
                let squash = 1.0 - (amplitude * 0.55) * pulse;
                let target_scale = Vec3::new(stretch, squash, stretch);
                transform.scale = transform.scale.lerp(target_scale, scale_alpha);
                if charge_state.phase != ChargePhase::Idle {
                    continue;
                }
            }
        }

        let (phase, ticks_remaining) = attack
            .map(|attack| (attack.phase, attack.ticks_remaining))
            .unwrap_or((AttackPhase::Ready, 0));
        let target_scale = attack_phase_scale(phase, ticks_remaining);
        transform.scale = transform.scale.lerp(target_scale, scale_alpha);
    }
}

fn update_local_hero_power_flicker(
    time: Res<Time>,
    mut heroes: Query<(&ChargeStateStat, &mut Visibility), With<LocalHeroActor>>,
) {
    let flicker_phase = (time.elapsed_secs() * POWER_FLICKER_HZ).fract();
    let should_hide = flicker_phase < POWER_FLICKER_HIDDEN_DUTY;

    for (charge_state, mut visibility) in &mut heroes {
        if charge_state.power_active {
            *visibility = if should_hide {
                Visibility::Hidden
            } else {
                Visibility::Inherited
            };
        } else {
            *visibility = Visibility::Inherited;
        }
    }
}

fn sync_dynamic_actors(
    mut commands: Commands,
    time: Res<Time>,
    fixed_time: Res<Time<Fixed>>,
    input_state: Res<InputState>,
    mut local_smoothing: ResMut<LocalHeroSmoothing>,
    world: Res<WorldView>,
    mut render_index: ResMut<RenderIndex>,
    assets: Res<SceneAssets>,
    runtime: Option<NonSend<NetworkRuntime>>,
    mut actors: Query<
        (
            &mut Transform,
            Option<&mut HealthStat>,
            Option<&mut ManaStat>,
            Option<&mut PowerStat>,
            Option<&mut ChargeStateStat>,
            Option<&mut FacingStat>,
            Option<&mut RegularAttackStat>,
            Option<&mut TowerBuildNode>,
        ),
        With<DynamicActor>,
    >,
) {
    let dt = time.delta_secs();
    let local_client_id = runtime.as_ref().map(|runtime| runtime.client_id);
    let has_local_hero = local_client_id
        .and_then(|client_id| world.heroes.get(&client_id))
        .is_some();
    if !has_local_hero {
        local_smoothing.predicted_pos = None;
        local_smoothing.render_pos = None;
    }

    let mut desired = HashMap::new();
    for hero in world.heroes.values() {
        let local = Some(hero.client_id) == local_client_id;
        let movement_allowed = hero.regular_attack.phase == AttackPhase::Ready
            && hero.charge_state.phase == ChargePhase::Idle;
        let pos = if local {
            let predicted = if movement_allowed {
                runtime
                    .as_ref()
                    .map(|runtime| predict_local_position(hero.pos, &runtime.pending_moves))
                    .unwrap_or(hero.pos)
            } else {
                hero.pos
            };
            update_local_hero_smoothing(
                &mut local_smoothing,
                predicted,
                if movement_allowed {
                    input_state.dir
                } else {
                    [0.0, 0.0]
                },
                fixed_time.overstep_fraction(),
                dt,
                &world,
                hero.client_id,
            )
        } else {
            hero.pos
        };
        let visual_facing = if local {
            if hero.lock_mode_active {
                let lock_target_pos = hero
                    .lock_target_id
                    .and_then(|target_id| world.enemies.get(&target_id).map(|enemy| enemy.pos))
                    .or_else(|| {
                        world
                            .enemies
                            .values()
                            .min_by(|a, b| {
                                distance_sq(pos, a.pos)
                                    .total_cmp(&distance_sq(pos, b.pos))
                                    .then_with(|| a.id.cmp(&b.id))
                            })
                            .map(|enemy| enemy.pos)
                    });
                if let Some(target_pos) = lock_target_pos {
                    let to_target =
                        normalize_or_zero([target_pos[0] - pos[0], target_pos[1] - pos[1]]);
                    if to_target != [0.0, 0.0] {
                        to_target
                    } else {
                        hero.facing.dir
                    }
                } else if input_state.dir != [0.0, 0.0] {
                    input_state.dir
                } else {
                    hero.facing.dir
                }
            } else if movement_allowed && input_state.dir != [0.0, 0.0] {
                input_state.dir
            } else {
                hero.facing.dir
            }
        } else {
            hero.facing.dir
        };
        desired.insert(
            hero.client_id,
            DesiredActor {
                pos,
                kind: ActorKind::Hero { local },
                facing: Some(FacingStat { dir: visual_facing }),
                regular_attack: Some(attack_state_component_to_stat(hero.regular_attack)),
                health: Some(HealthStat {
                    current: hero.hp,
                    max: HERO_MAX_HP,
                }),
                mana: Some(ManaStat {
                    current: hero.mana,
                    max: HERO_MAX_MANA,
                }),
                power: None,
                charge_state: Some(ChargeStateStat {
                    phase: hero.charge_state.phase,
                    power_active: hero.charge_state.power_active,
                }),
                tower_node: None,
            },
        );
    }

    for enemy in world.enemies.values() {
        desired.insert(
            enemy.id,
            DesiredActor {
                pos: predict_enemy_position(enemy),
                kind: ActorKind::Enemy,
                facing: Some(FacingStat {
                    dir: enemy.facing.dir,
                }),
                regular_attack: Some(attack_state_component_to_stat(enemy.regular_attack)),
                health: Some(HealthStat {
                    current: enemy.hp,
                    max: enemy.max_hp,
                }),
                mana: None,
                power: None,
                charge_state: None,
                tower_node: None,
            },
        );
    }

    for tower in world.towers.values() {
        desired.insert(
            tower.id,
            DesiredActor {
                pos: tower.pos,
                kind: ActorKind::Tower,
                facing: None,
                regular_attack: None,
                health: None,
                mana: None,
                power: None,
                charge_state: None,
                tower_node: Some(TowerBuildNode {
                    node_id: tower.node_id,
                }),
            },
        );
    }

    for (id, actor) in &desired {
        if let Some(existing_entity) = render_index.by_id.get(id).copied() {
            if let Ok((
                mut transform,
                health,
                mana,
                power,
                charge_state,
                facing,
                regular_attack,
                tower_node,
            )) = actors.get_mut(existing_entity)
            {
                let target_translation = world_to_translation(actor.pos, actor_y(actor.kind));
                if let Some(smooth_rate) = actor_smoothing_rate(actor.kind) {
                    if transform.translation.distance_squared(target_translation)
                        > RENDER_SNAP_DISTANCE_SQ
                    {
                        transform.translation = target_translation;
                    } else {
                        let alpha = 1.0 - (-smooth_rate * time.delta_secs()).exp();
                        transform.translation = transform
                            .translation
                            .lerp(target_translation, alpha.clamp(0.0, 1.0));
                    }
                } else {
                    transform.translation = target_translation;
                }
                sync_existing_actor_components(
                    &mut commands,
                    existing_entity,
                    *actor,
                    health,
                    mana,
                    power,
                    charge_state,
                    facing,
                    regular_attack,
                    tower_node,
                );
                continue;
            }

            render_index.by_id.remove(id);
            commands.entity(existing_entity).despawn();
        }

        let entity = spawn_actor_entity(&mut commands, &assets, *actor);
        render_index.by_id.insert(*id, entity);
    }

    let stale_ids: Vec<u64> = render_index
        .by_id
        .keys()
        .filter(|id| !desired.contains_key(id))
        .copied()
        .collect();

    for stale_id in stale_ids {
        if let Some(entity) = render_index.by_id.remove(&stale_id) {
            commands.entity(entity).despawn();
        }
    }
}

fn local_locked_target_entity(
    world: &WorldView,
    runtime: Option<&NetworkRuntime>,
    render_index: &RenderIndex,
) -> Option<Entity> {
    let hero = world.heroes.get(&runtime?.client_id)?;
    if !hero.lock_mode_active {
        return None;
    }
    let target_id = hero.lock_target_id?;
    render_index.by_id.get(&target_id).copied()
}

fn health_bar_width_multiplier(
    stat: BarStat,
    parent_entity: Entity,
    locked_entity: Option<Entity>,
) -> f32 {
    if stat == BarStat::Health && Some(parent_entity) == locked_entity {
        LOCKED_HEALTH_BAR_SCALE_MULTIPLIER
    } else {
        1.0
    }
}

fn update_unit_bars(
    world: Res<WorldView>,
    render_index: Res<RenderIndex>,
    runtime: Option<NonSend<NetworkRuntime>>,
    mut bars: Query<(&UnitBarFill, &UnitBarLayout, &UnitBarFollow, &mut Transform)>,
    health_stats: Query<&HealthStat>,
    mana_stats: Query<&ManaStat>,
    power_stats: Query<&PowerStat>,
) {
    let locked_entity = local_locked_target_entity(&world, runtime.as_deref(), &render_index);

    for (bar, layout, follow, mut transform) in &mut bars {
        let parent_entity = follow.target;

        let ratio = match bar.stat {
            BarStat::Health => health_stats
                .get(parent_entity)
                .ok()
                .map(|health| {
                    if health.max <= f32::EPSILON {
                        0.0
                    } else {
                        (health.current / health.max).clamp(0.0, 1.0)
                    }
                })
                .unwrap_or(0.0),
            BarStat::Mana => mana_stats
                .get(parent_entity)
                .ok()
                .map(|mana| {
                    if mana.max <= f32::EPSILON {
                        0.0
                    } else {
                        (mana.current / mana.max).clamp(0.0, 1.0)
                    }
                })
                .unwrap_or(0.0),
            BarStat::Power => power_stats
                .get(parent_entity)
                .ok()
                .map(|power| {
                    if power.max <= f32::EPSILON {
                        0.0
                    } else {
                        (power.current / power.max).clamp(0.0, 1.0)
                    }
                })
                .unwrap_or(0.0),
        };

        let scaled = ratio.max(MIN_BAR_FILL_RATIO);
        let width_multiplier = health_bar_width_multiplier(bar.stat, parent_entity, locked_entity);
        transform.scale.x = scaled * layout.max_scale_x * width_multiplier;
    }
}

fn update_unit_bar_background_scales(
    world: Res<WorldView>,
    render_index: Res<RenderIndex>,
    runtime: Option<NonSend<NetworkRuntime>>,
    mut bars: Query<
        (&UnitBarRow, &UnitBarFollow, &UnitBarLayout, &mut Transform),
        (With<UnitBarVisual>, Without<UnitBarFill>),
    >,
) {
    let locked_entity = local_locked_target_entity(&world, runtime.as_deref(), &render_index);

    for (row, follow, layout, mut transform) in &mut bars {
        let width_multiplier = health_bar_width_multiplier(row.stat, follow.target, locked_entity);
        transform.scale.x = layout.max_scale_x * width_multiplier;
    }
}

fn update_unit_bar_visibility(
    mut bars: Query<(&UnitBarRow, &UnitBarFollow, &mut Visibility), With<UnitBarVisual>>,
    power_stats: Query<&PowerStat>,
) {
    for (row, follow, mut visibility) in &mut bars {
        if row.stat != BarStat::Power {
            continue;
        }

        let has_power = power_stats
            .get(follow.target)
            .ok()
            .is_some_and(|power| power.current > f32::EPSILON);

        *visibility = if has_power {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}

fn cleanup_orphan_unit_bars(
    mut commands: Commands,
    bars: Query<(Entity, &UnitBarFollow), With<UnitBarVisual>>,
    actors: Query<(), With<DynamicActor>>,
) {
    for (bar_entity, follow) in &bars {
        if actors.get(follow.target).is_err() {
            commands.entity(bar_entity).despawn();
        }
    }
}

fn update_build_node_markers(
    assets: Res<SceneAssets>,
    mut markers: Query<(&BuildNodeMarker, &mut MeshMaterial3d<StandardMaterial>)>,
    towers: Query<&TowerBuildNode, With<TowerActor>>,
) {
    let occupied_nodes: HashSet<u32> = towers.iter().map(|tower| tower.node_id).collect();

    for (marker, mut material) in &mut markers {
        let target = if occupied_nodes.contains(&marker.node_id) {
            assets.node_taken_material.clone()
        } else {
            assets.node_free_material.clone()
        };

        if material.0 != target {
            material.0 = target;
        }
    }
}

fn update_objective_markers(
    world: Res<WorldView>,
    mut markers: Query<&mut Transform, With<ObjectiveMarker>>,
) {
    let Some(objective) = world.objective() else {
        return;
    };

    for mut transform in &mut markers {
        let ratio = if objective.max_hp <= f32::EPSILON {
            0.0
        } else {
            (objective.hp / objective.max_hp).clamp(0.0, 1.0)
        };

        let y_scale = 0.35 + ratio * 1.65;
        transform.scale.y = y_scale;
        transform.translation.y = 0.35 + y_scale * 0.5;
    }
}

fn update_match_end_overlay(
    world: Res<WorldView>,
    mut root: Single<&mut Node, With<MatchEndOverlayRoot>>,
    overlay_text: Single<(&mut Text, &mut TextColor), With<MatchEndOverlayText>>,
) {
    let (title, title_color) = match world.phase {
        MatchPhase::InProgress => {
            root.display = Display::None;
            return;
        }
        MatchPhase::Victory => ("Victory", MATCH_END_OVERLAY_TITLE),
        MatchPhase::Defeat => ("Defeat", MATCH_END_OVERLAY_DEFEAT_TITLE),
    };

    root.display = Display::Flex;
    let ticks_remaining = world.match_restart_ticks_remaining.unwrap_or_default();
    let seconds_remaining = ticks_remaining as f32 * FIXED_DT_SECONDS;
    let (mut text, mut text_color) = overlay_text.into_inner();
    text.0 = format!(
        "{title}\nNext match starts in {:.1}s",
        seconds_remaining.max(0.0)
    );
    text_color.0 = title_color;
}

fn update_hud(
    world: Res<WorldView>,
    runtime: Option<NonSend<NetworkRuntime>>,
    hud_state: Res<HudState>,
    mut hud_text: Single<&mut Text, With<HudText>>,
) {
    let status = runtime
        .as_ref()
        .map(|runtime| {
            if runtime.renet.is_connected() {
                "connected"
            } else if runtime.renet.is_connecting() {
                "connecting"
            } else {
                "disconnected"
            }
        })
        .unwrap_or("offline");

    let restart_line = match (world.phase, world.match_restart_ticks_remaining) {
        (MatchPhase::InProgress, _) => "restart: --".to_owned(),
        (_, Some(ticks)) => format!("restart: {:.1}s", ticks as f32 * FIXED_DT_SECONDS),
        (_, None) => "restart: pending".to_owned(),
    };

    hud_text.0 = format!(
        "Discordium TD MVP\nstatus: {status}\nphase: {:?}\nwave: {}\nteam life: {}\n{restart_line}\ncontrols: WASD move, Left Click/J regular attack, K special attack, Space hold charge, Q lock/cycle, E clear lock\n{}",
        world.phase, world.wave, world.team_life, hud_state.last_event
    );
}

fn update_gold_hud(
    mut commands: Commands,
    world: Res<WorldView>,
    runtime: Option<NonSend<NetworkRuntime>>,
    mut gold_hud_state: ResMut<GoldHudState>,
    root_entity: Single<Entity, With<GoldHudRoot>>,
    mut value_text: Single<&mut Text, With<GoldHudValueText>>,
) {
    let local_gold = runtime
        .as_ref()
        .and_then(|runtime| world.heroes.get(&runtime.client_id))
        .map(|hero| hero.gold);

    let Some(gold) = local_gold else {
        gold_hud_state.last_gold = None;
        value_text.0 = "--".to_owned();
        return;
    };

    value_text.0 = gold.to_string();
    if let Some(previous_gold) = gold_hud_state.last_gold {
        if gold < previous_gold {
            let spent = previous_gold - gold;
            commands.entity(*root_entity).with_children(|parent| {
                parent.spawn((
                    GoldSpendPopup { age_seconds: 0.0 },
                    Node {
                        position_type: PositionType::Absolute,
                        right: Val::Px(0.0),
                        top: Val::Px(GOLD_SPEND_POPUP_START_TOP),
                        ..Default::default()
                    },
                    Text::new(format!("-{spent}")),
                    TextFont {
                        font_size: 21.0,
                        ..Default::default()
                    },
                    TextColor(GOLD_SPEND_TEXT_COLOR),
                ));
            });
        }
    }
    gold_hud_state.last_gold = Some(gold);
}

fn update_gold_spend_popups(
    mut commands: Commands,
    time: Res<Time>,
    mut popups: Query<(Entity, &mut GoldSpendPopup, &mut Node, &mut TextColor)>,
) {
    for (popup_entity, mut popup, mut node, mut text_color) in &mut popups {
        popup.age_seconds += time.delta_secs();
        let ratio = (popup.age_seconds / GOLD_SPEND_POPUP_DURATION_SECONDS).clamp(0.0, 1.0);
        node.top = Val::Px(GOLD_SPEND_POPUP_START_TOP - GOLD_SPEND_POPUP_RISE_PIXELS * ratio);
        let base = GOLD_SPEND_TEXT_COLOR.to_srgba();
        text_color.0 = Color::srgba(base.red, base.green, base.blue, 1.0 - ratio);
        if ratio >= 1.0 {
            commands.entity(popup_entity).despawn();
        }
    }
}

fn update_special_skill_hud(
    world: Res<WorldView>,
    runtime: Option<NonSend<NetworkRuntime>>,
    mut cooldown_ui_state: ResMut<SpecialSkillCooldownUiState>,
    mut icon_color: Single<&mut BackgroundColor, With<SpecialSkillIcon>>,
    mut key_color: Single<&mut TextColor, With<SpecialSkillKeyText>>,
    overlay: Single<(&mut Node, &mut Visibility), With<SpecialSkillCooldownOverlay>>,
) {
    let (mut overlay_node, mut overlay_visibility) = overlay.into_inner();

    let local_hero = runtime
        .as_ref()
        .and_then(|runtime| world.heroes.get(&runtime.client_id));
    let Some(local_hero) = local_hero else {
        cooldown_ui_state.last_ticks_remaining = 0;
        cooldown_ui_state.max_ticks_remaining = 0;
        overlay_node.height = Val::Percent(0.0);
        *overlay_visibility = Visibility::Hidden;
        icon_color.0 = SPECIAL_SKILL_ICON_READY_BG;
        key_color.0 = SPECIAL_SKILL_ICON_KEY_COLOR;
        return;
    };

    let ticks_remaining = local_hero.ability_cooldown_ticks;
    if ticks_remaining > cooldown_ui_state.last_ticks_remaining {
        cooldown_ui_state.max_ticks_remaining = ticks_remaining;
    }
    cooldown_ui_state.last_ticks_remaining = ticks_remaining;
    if ticks_remaining == 0 {
        cooldown_ui_state.max_ticks_remaining = 0;
    }

    if ticks_remaining == 0 {
        overlay_node.height = Val::Percent(0.0);
        *overlay_visibility = Visibility::Hidden;
        icon_color.0 = SPECIAL_SKILL_ICON_READY_BG;
        key_color.0 = SPECIAL_SKILL_ICON_KEY_COLOR;
        return;
    }

    let max_ticks = cooldown_ui_state.max_ticks_remaining.max(ticks_remaining);
    let fill_ratio = (ticks_remaining as f32 / max_ticks as f32).clamp(0.0, 1.0);
    overlay_node.height = Val::Percent(fill_ratio * 100.0);
    *overlay_visibility = Visibility::Inherited;
    icon_color.0 = SPECIAL_SKILL_ICON_COOLDOWN_BG;
    key_color.0 = Color::srgb(0.86, 0.89, 0.93);
}

fn update_power_pie_hud(
    time: Res<Time>,
    world: Res<WorldView>,
    runtime: Option<NonSend<NetworkRuntime>>,
    power_pie_texture: Option<Res<PowerPieUiTexture>>,
    power_pie_raster_cache: Option<Res<PowerPieRasterCache>>,
    mut power_pie_state: ResMut<PowerPieUiState>,
    mut images: ResMut<Assets<Image>>,
    circle: Single<(&mut Node, &mut BorderColor, &mut BackgroundColor), With<PowerPieCircle>>,
) {
    let Some(power_pie_texture) = power_pie_texture else {
        return;
    };
    let Some(power_pie_raster_cache) = power_pie_raster_cache else {
        return;
    };
    let (mut circle_node, mut border_color, mut background_color) = circle.into_inner();
    let Some(image) = images.get_mut(&power_pie_texture.handle) else {
        return;
    };

    let local_hero = runtime
        .as_ref()
        .and_then(|runtime| world.heroes.get(&runtime.client_id));
    let (ratio, power_active) = local_hero
        .map(|hero| {
            let max = hero.charge_profile.power_meter_max.max(f32::EPSILON);
            (
                (hero.charge_state.power_meter / max).clamp(0.0, 1.0),
                hero.charge_state.power_active,
            )
        })
        .unwrap_or((0.0, false));

    let flash = if power_active {
        let phase = time.elapsed_secs() * POWER_PIE_FLASH_HZ * std::f32::consts::TAU;
        0.5 + 0.5 * phase.sin()
    } else {
        0.0
    };
    let flash_bucket = if power_active {
        ((flash * POWER_PIE_FLASH_BUCKETS as f32).floor() as u32)
            .min(POWER_PIE_FLASH_BUCKETS.saturating_sub(1))
    } else {
        0
    };
    let ratio_changed = (ratio - power_pie_state.last_ratio).abs() > 0.0005;
    let active_changed = power_active != power_pie_state.last_power_active;
    let flash_changed = power_active && flash_bucket != power_pie_state.last_flash_bucket;
    let visuals_changed =
        !power_pie_state.initialized || ratio_changed || active_changed || flash_changed;

    if !visuals_changed {
        return;
    }

    let size = if power_active {
        POWER_PIE_ACTIVE_SIZE + flash * POWER_PIE_ACTIVE_PULSE_SIZE
    } else {
        POWER_PIE_BASE_SIZE
    };
    let inset = (POWER_PIE_SLOT_SIZE - size).max(0.0) * 0.5;
    circle_node.left = Val::Px(inset);
    circle_node.bottom = Val::Px(inset);
    circle_node.width = Val::Px(size);
    circle_node.height = Val::Px(size);

    let fill_color = if power_active {
        POWER_PIE_FILL_ACTIVE_COLOR.mix(&POWER_PIE_FILL_COLOR, 1.0 - flash * 0.45)
    } else {
        POWER_PIE_FILL_COLOR
    };
    let bg_color = if power_active {
        POWER_PIE_BG_ACTIVE_COLOR.mix(&POWER_PIE_BG_COLOR, 1.0 - flash * 0.35)
    } else {
        POWER_PIE_BG_COLOR
    };
    *border_color = BorderColor::all(if power_active {
        POWER_PIE_BORDER_ACTIVE_COLOR.mix(&POWER_PIE_BORDER_COLOR, 1.0 - flash * 0.25)
    } else {
        POWER_PIE_BORDER_COLOR
    });
    background_color.0 = bg_color;

    draw_power_pie_image(
        image,
        &power_pie_raster_cache,
        ratio,
        fill_color,
        POWER_PIE_EMPTY_COLOR,
    );
    power_pie_state.last_ratio = ratio;
    power_pie_state.last_flash_bucket = flash_bucket;
    power_pie_state.last_power_active = power_active;
    power_pie_state.initialized = true;
}

fn build_power_pie_image(
    cache: &PowerPieRasterCache,
    ratio: f32,
    fill: Color,
    empty: Color,
) -> Image {
    let size = cache.size;
    let mut image = Image::new_fill(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0, 0, 0, 0],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    draw_power_pie_image(&mut image, cache, ratio, fill, empty);
    image
}

fn build_power_pie_raster_cache(size: u32) -> PowerPieRasterCache {
    let mut inside = Vec::with_capacity((size * size) as usize);
    let mut clockwise_angle = Vec::with_capacity((size * size) as usize);

    if size == 0 {
        return PowerPieRasterCache {
            size,
            inside,
            clockwise_angle,
        };
    }

    let radius = size as f32 * 0.5;
    let center = radius - 0.5;

    for y in 0..size {
        for x in 0..size {
            let dx = (x as f32 - center) / radius;
            let dy = (y as f32 - center) / radius;
            let dist_sq = dx * dx + dy * dy;
            if dist_sq > 1.0 {
                inside.push(false);
                clockwise_angle.push(0.0);
                continue;
            }

            // Angle starts at 12 o'clock and increases clockwise.
            let mut angle = dx.atan2(-dy);
            if angle < 0.0 {
                angle += std::f32::consts::TAU;
            }
            inside.push(true);
            clockwise_angle.push(angle);
        }
    }

    PowerPieRasterCache {
        size,
        inside,
        clockwise_angle,
    }
}

fn draw_power_pie_image(
    image: &mut Image,
    cache: &PowerPieRasterCache,
    ratio: f32,
    fill: Color,
    empty: Color,
) {
    let size = cache.size;
    if size == 0 || image.texture_descriptor.size.width != size {
        return;
    }
    let Some(data) = image.data.as_mut() else {
        return;
    };

    let sweep = ratio.clamp(0.0, 1.0) * std::f32::consts::TAU;
    let fill = fill.to_srgba().to_u8_array();
    let empty = empty.to_srgba().to_u8_array();
    let width = size as usize;
    let total = width * width;
    if cache.inside.len() != total || cache.clockwise_angle.len() != total {
        return;
    }
    if data.len() < total * 4 {
        return;
    }

    for i in 0..total {
        let base = i * 4;
        if !cache.inside[i] {
            data[base..base + 4].copy_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        let color = if cache.clockwise_angle[i] <= sweep {
            fill
        } else {
            empty
        };
        data[base..base + 4].copy_from_slice(&color);
    }
}

fn publish_unit_bar_camera_updates(
    mut commands: Commands,
    camera_cache: Res<UnitBarCameraCache>,
    camera: Single<(&Transform, Ref<Transform>), With<UnitBarCamera>>,
) {
    let (camera_transform, camera_ref) = camera.into_inner();

    if camera_cache.initialized && !camera_ref.is_changed() {
        return;
    }

    let world_rotation =
        camera_billboard_world_rotation(camera_transform).unwrap_or(camera_cache.world_rotation);
    commands.trigger(UnitBarCameraChanged { world_rotation });
}

fn apply_unit_bar_camera_update(
    camera_changed: On<UnitBarCameraChanged>,
    mut camera_cache: ResMut<UnitBarCameraCache>,
) {
    let world_rotation = camera_changed.world_rotation;
    if camera_cache.initialized && quats_nearly_equal(camera_cache.world_rotation, world_rotation) {
        return;
    }

    camera_cache.world_rotation = world_rotation;
    camera_cache.revision = camera_cache.revision.wrapping_add(1);
    camera_cache.initialized = true;
}

fn orient_unit_bars_to_camera(
    camera_cache: Res<UnitBarCameraCache>,
    world: Res<WorldView>,
    render_index: Res<RenderIndex>,
    runtime: Option<NonSend<NetworkRuntime>>,
    actor_query: Query<Ref<Transform>, (With<DynamicActor>, Without<UnitBarVisual>)>,
    mut bars: Query<
        (
            &UnitBarRow,
            &UnitBarFollow,
            &UnitBarLayout,
            &mut UnitBarBillboardState,
            &mut Transform,
        ),
        (With<UnitBarVisual>, Without<DynamicActor>),
    >,
) {
    if !camera_cache.initialized {
        return;
    }

    let locked_entity = local_locked_target_entity(&world, runtime.as_deref(), &render_index);

    for (row, follow, layout, mut billboard_state, mut transform) in &mut bars {
        let Ok(actor_transform) = actor_query.get(follow.target) else {
            continue;
        };
        let camera_changed = billboard_state.camera_revision != camera_cache.revision;
        let target_translation = actor_transform.translation;
        let target_moved = !billboard_state.initialized
            || (actor_transform.is_changed()
                && !vec3_nearly_equal(target_translation, billboard_state.target_translation));
        let scale_changed = !billboard_state.initialized
            || !f32_nearly_equal(transform.scale.x, billboard_state.last_scale_x);

        if !camera_changed && !target_moved && !scale_changed {
            continue;
        }

        transform.rotation = camera_cache.world_rotation;

        let width_multiplier = health_bar_width_multiplier(row.stat, follow.target, locked_entity);
        let effective_max_scale_x = layout.max_scale_x * width_multiplier;
        let fill_ratio = (transform.scale.x / effective_max_scale_x).clamp(MIN_BAR_FILL_RATIO, 1.0);
        let local_x = if layout.fill {
            -UNIT_BAR_WIDTH * effective_max_scale_x * 0.5 * (1.0 - fill_ratio)
        } else {
            0.0
        };
        let local_y = layout.base_y + layout.row_index as f32 * UNIT_BAR_ROW_GAP;
        let local_z = if layout.fill { UNIT_BAR_FILL_Z } else { 0.0 };
        let local_offset = Vec3::new(local_x, local_y, local_z);
        transform.translation = target_translation + camera_cache.world_rotation * local_offset;
        billboard_state.camera_revision = camera_cache.revision;
        billboard_state.target_translation = target_translation;
        billboard_state.last_scale_x = transform.scale.x;
        billboard_state.initialized = true;
    }
}

fn camera_billboard_world_rotation(camera_transform: &Transform) -> Option<Quat> {
    let rotation = camera_transform.rotation;
    if !rotation.is_finite() {
        return None;
    }

    // Full camera-aligned billboard so bars always face the current camera orientation.
    Some(rotation)
}

fn quats_nearly_equal(a: Quat, b: Quat) -> bool {
    a.dot(b).abs() >= 0.999_95
}

fn vec3_nearly_equal(a: Vec3, b: Vec3) -> bool {
    (a - b).length_squared() <= 0.000_000_0001
}

fn f32_nearly_equal(a: f32, b: f32) -> bool {
    (a - b).abs() <= 0.000_01
}

fn world_to_scene_plane(pos: [f32; 2]) -> [f32; 2] {
    pos
}

fn facing_to_scene_yaw(facing: [f32; 2]) -> Option<f32> {
    let scene = world_to_scene_plane(normalize_or_zero(facing));
    if scene[0] == 0.0 && scene[1] == 0.0 {
        return None;
    }

    Some(scene[0].atan2(scene[1]))
}

fn attack_state_component_to_stat(state: DirectionalAttackStateComponent) -> RegularAttackStat {
    RegularAttackStat {
        phase: state.phase,
        ticks_remaining: state.ticks_remaining,
    }
}

fn attack_phase_scale(phase: AttackPhase, ticks_remaining: u32) -> Vec3 {
    let tick_bias = (ticks_remaining.min(8) as f32) * 0.01;
    match phase {
        AttackPhase::Ready => Vec3::ONE,
        AttackPhase::Windup => Vec3::new(1.1 + tick_bias, 0.88 - tick_bias * 0.3, 1.1 + tick_bias),
        AttackPhase::Recovery => Vec3::new(
            0.93 - tick_bias * 0.25,
            1.06 + tick_bias * 0.2,
            0.93 - tick_bias * 0.25,
        ),
    }
}

fn world_to_translation(pos: [f32; 2], y: f32) -> Vec3 {
    let scene_pos = world_to_scene_plane(pos);
    Vec3::new(scene_pos[0], y, scene_pos[1])
}

fn actor_y(kind: ActorKind) -> f32 {
    match kind {
        ActorKind::Hero { .. } => 0.9,
        ActorKind::Enemy => 0.52,
        ActorKind::Tower => 0.75,
    }
}

fn actor_smoothing_rate(kind: ActorKind) -> Option<f32> {
    match kind {
        ActorKind::Hero { local: true } | ActorKind::Tower => None,
        ActorKind::Hero { local: false } => Some(REMOTE_HERO_RENDER_SMOOTH_RATE),
        ActorKind::Enemy => Some(ENEMY_RENDER_SMOOTH_RATE),
    }
}

fn predict_enemy_position(enemy: &EnemySnapshot) -> [f32; 2] {
    clamp_to_world([
        enemy.pos[0] + enemy.vel[0] * ENEMY_RENDER_LEAD_SECONDS,
        enemy.pos[1] + enemy.vel[1] * ENEMY_RENDER_LEAD_SECONDS,
    ])
}

fn spawn_actor_entity(
    commands: &mut Commands,
    assets: &SceneAssets,
    actor: DesiredActor,
) -> Entity {
    let mut transform =
        Transform::from_translation(world_to_translation(actor.pos, actor_y(actor.kind)));
    if let Some(facing) = actor.facing {
        if let Some(yaw) = facing_to_scene_yaw(facing.dir) {
            transform.rotation = Quat::from_rotation_y(yaw);
        }
    }

    let entity = match actor.kind {
        ActorKind::Hero { local: true } => commands
            .spawn((
                Mesh3d(assets.hero_mesh.clone()),
                MeshMaterial3d(assets.hero_local_material.clone()),
                transform,
                DynamicActor,
                HeroActor,
                LocalHeroActor,
            ))
            .id(),
        ActorKind::Hero { local: false } => commands
            .spawn((
                Mesh3d(assets.hero_mesh.clone()),
                MeshMaterial3d(assets.hero_remote_material.clone()),
                transform,
                DynamicActor,
                HeroActor,
            ))
            .id(),
        ActorKind::Enemy => commands
            .spawn((
                Mesh3d(assets.enemy_mesh.clone()),
                MeshMaterial3d(assets.enemy_material.clone()),
                transform,
                DynamicActor,
                EnemyActor,
            ))
            .id(),
        ActorKind::Tower => commands
            .spawn((
                Mesh3d(assets.tower_mesh.clone()),
                MeshMaterial3d(assets.tower_material.clone()),
                transform,
                DynamicActor,
                TowerActor,
            ))
            .id(),
    };

    apply_actor_stats(commands, entity, actor);
    apply_actor_metadata(commands, entity, actor);
    attach_facing_indicator(commands, assets, entity, actor);
    attach_unit_bars(commands, assets, entity, actor);
    entity
}

fn apply_actor_stats(commands: &mut Commands, entity: Entity, actor: DesiredActor) {
    let mut entity_commands = commands.entity(entity);

    if let Some(health) = actor.health {
        entity_commands.insert(health);
    } else {
        entity_commands.remove::<HealthStat>();
    }

    if let Some(mana) = actor.mana {
        entity_commands.insert(mana);
    } else {
        entity_commands.remove::<ManaStat>();
    }

    if let Some(power) = actor.power {
        entity_commands.insert(power);
    } else {
        entity_commands.remove::<PowerStat>();
    }

    if let Some(charge_state) = actor.charge_state {
        entity_commands.insert(charge_state);
    } else {
        entity_commands.remove::<ChargeStateStat>();
    }

    if let Some(facing) = actor.facing {
        entity_commands.insert(facing);
    } else {
        entity_commands.remove::<FacingStat>();
    }

    if let Some(regular_attack) = actor.regular_attack {
        entity_commands.insert(regular_attack);
    } else {
        entity_commands.remove::<RegularAttackStat>();
    }
}

fn apply_actor_metadata(commands: &mut Commands, entity: Entity, actor: DesiredActor) {
    let mut entity_commands = commands.entity(entity);
    if let Some(tower_node) = actor.tower_node {
        entity_commands.insert(tower_node);
    } else {
        entity_commands.remove::<TowerBuildNode>();
    }
}

fn sync_existing_actor_components(
    commands: &mut Commands,
    entity: Entity,
    actor: DesiredActor,
    health: Option<Mut<HealthStat>>,
    mana: Option<Mut<ManaStat>>,
    power: Option<Mut<PowerStat>>,
    charge_state: Option<Mut<ChargeStateStat>>,
    facing: Option<Mut<FacingStat>>,
    regular_attack: Option<Mut<RegularAttackStat>>,
    tower_node: Option<Mut<TowerBuildNode>>,
) {
    sync_optional_component(commands, entity, health, actor.health);
    sync_optional_component(commands, entity, mana, actor.mana);
    sync_optional_component(commands, entity, power, actor.power);
    sync_optional_component(commands, entity, charge_state, actor.charge_state);
    sync_optional_component(commands, entity, facing, actor.facing);
    sync_optional_component(commands, entity, regular_attack, actor.regular_attack);
    sync_optional_component(commands, entity, tower_node, actor.tower_node);
}

fn sync_optional_component<T: Component + Copy + PartialEq>(
    commands: &mut Commands,
    entity: Entity,
    current: Option<Mut<T>>,
    desired: Option<T>,
) {
    match (current, desired) {
        (Some(mut current), Some(desired)) => {
            if *current != desired {
                *current = desired;
            }
        }
        (None, Some(desired)) => {
            commands.entity(entity).insert(desired);
        }
        (Some(_), None) => {
            commands.entity(entity).remove::<T>();
        }
        (None, None) => {}
    }
}

fn attach_unit_bars(
    commands: &mut Commands,
    assets: &SceneAssets,
    actor_entity: Entity,
    actor: DesiredActor,
) {
    let health_material = match actor.kind {
        ActorKind::Hero { .. } => assets.hero_health_bar_material.clone(),
        ActorKind::Enemy => assets.enemy_health_bar_material.clone(),
        ActorKind::Tower => return,
    };

    let y_base = match actor.kind {
        ActorKind::Hero { .. } => UNIT_BAR_HERO_Y,
        ActorKind::Enemy => UNIT_BAR_ENEMY_Y,
        ActorKind::Tower => return,
    };
    let width_scale = match actor.kind {
        ActorKind::Hero { .. } => UNIT_BAR_HERO_WIDTH_SCALE,
        ActorKind::Enemy => UNIT_BAR_ENEMY_WIDTH_SCALE,
        ActorKind::Tower => return,
    };

    let anchor = world_to_translation(actor.pos, actor_y(actor.kind));
    let follow = UnitBarFollow {
        target: actor_entity,
    };
    let mut row_index = 0_u32;
    let mut spawn_row =
        |commands: &mut Commands, fill_material: Handle<StandardMaterial>, stat: BarStat| {
            let y = y_base + row_index as f32 * UNIT_BAR_ROW_GAP;
            let layout_background = UnitBarLayout {
                row_index,
                base_y: y_base,
                fill: false,
                max_scale_x: width_scale,
            };
            let layout_fill = UnitBarLayout {
                row_index,
                base_y: y_base,
                fill: true,
                max_scale_x: width_scale,
            };
            commands.spawn((
                Mesh3d(assets.unit_bar_mesh.clone()),
                MeshMaterial3d(assets.bar_background_material.clone()),
                Transform::from_xyz(anchor.x, anchor.y + y, anchor.z).with_scale(Vec3::new(
                    width_scale,
                    1.0,
                    1.0,
                )),
                NotShadowCaster,
                UnitBarVisual,
                UnitBarRow { stat },
                follow,
                layout_background,
                UnitBarBillboardState::default(),
            ));
            commands.spawn((
                Mesh3d(assets.unit_bar_mesh.clone()),
                MeshMaterial3d(fill_material),
                Transform::from_xyz(anchor.x, anchor.y + y, anchor.z + UNIT_BAR_FILL_Z)
                    .with_scale(Vec3::new(width_scale, 1.0, 1.0)),
                NotShadowCaster,
                UnitBarFill { stat },
                UnitBarVisual,
                UnitBarRow { stat },
                follow,
                layout_fill,
                UnitBarBillboardState::default(),
            ));
            row_index += 1;
        };

    if actor.health.is_some() {
        spawn_row(commands, health_material, BarStat::Health);
    }
    if actor.mana.is_some() {
        spawn_row(commands, assets.mana_bar_material.clone(), BarStat::Mana);
    }
    if actor.power.is_some() {
        spawn_row(commands, assets.power_bar_material.clone(), BarStat::Power);
    }
}

fn attach_facing_indicator(
    commands: &mut Commands,
    assets: &SceneAssets,
    actor_entity: Entity,
    actor: DesiredActor,
) {
    let (material, translation) = match actor.kind {
        ActorKind::Hero { .. } => (
            assets.hero_facing_indicator_material.clone(),
            Vec3::new(0.0, 0.62, 0.72),
        ),
        ActorKind::Enemy => (
            assets.enemy_facing_indicator_material.clone(),
            Vec3::new(0.0, 0.44, 0.62),
        ),
        ActorKind::Tower => return,
    };

    commands.entity(actor_entity).with_children(|parent| {
        parent.spawn((
            Mesh3d(assets.facing_indicator_mesh.clone()),
            MeshMaterial3d(material),
            Transform::from_translation(translation),
            FacingIndicatorVisual,
        ));
    });
}

fn pick_nearest_build_node(
    world: &WorldView,
    client_id: u64,
    towers: &Query<&TowerBuildNode, With<TowerActor>>,
) -> Option<u32> {
    let hero = world.heroes.get(&client_id)?;
    let occupied_nodes: HashSet<u32> = towers.iter().map(|tower| tower.node_id).collect();

    let mut best: Option<(u32, f32)> = None;
    for node in &world.build_nodes {
        if occupied_nodes.contains(&node.node_id) {
            continue;
        }

        let dist_sq = distance_sq(hero.pos, node.pos);
        if dist_sq > BUILD_COMMAND_MAX_DISTANCE * BUILD_COMMAND_MAX_DISTANCE {
            continue;
        }

        match best {
            Some((_, best_dist_sq)) if dist_sq >= best_dist_sq => {}
            _ => {
                best = Some((node.node_id, dist_sq));
            }
        }
    }

    best.map(|(node_id, _)| node_id)
}

fn predict_local_position(base: [f32; 2], pending_moves: &VecDeque<PendingMove>) -> [f32; 2] {
    let mut pos = base;
    for pending in pending_moves {
        pos = clamp_to_world([
            pos[0] + pending.dir[0] * HERO_SPEED * FIXED_DT_SECONDS,
            pos[1] + pending.dir[1] * HERO_SPEED * FIXED_DT_SECONDS,
        ]);
    }
    pos
}

fn predict_local_overstep_position(
    base: [f32; 2],
    input_dir: [f32; 2],
    overstep_fraction: f32,
) -> [f32; 2] {
    let overstep = overstep_fraction.clamp(0.0, 1.0);
    if overstep <= f32::EPSILON {
        return base;
    }

    let dir = normalize_or_zero(input_dir);
    clamp_to_world([
        base[0] + dir[0] * HERO_SPEED * FIXED_DT_SECONDS * overstep,
        base[1] + dir[1] * HERO_SPEED * FIXED_DT_SECONDS * overstep,
    ])
}

fn update_local_hero_smoothing(
    smoothing: &mut LocalHeroSmoothing,
    authoritative_predicted: [f32; 2],
    input_dir: [f32; 2],
    overstep_fraction: f32,
    dt: f32,
    world: &WorldView,
    local_client_id: u64,
) -> [f32; 2] {
    let predicted = match smoothing.predicted_pos {
        Some(current) => {
            if distance_sq(current, authoritative_predicted) > LOCAL_HERO_SNAP_DISTANCE_SQ {
                authoritative_predicted
            } else {
                let alpha = 1.0 - (-LOCAL_HERO_RECONCILE_RATE * dt).exp();
                [
                    current[0] + (authoritative_predicted[0] - current[0]) * alpha.clamp(0.0, 1.0),
                    current[1] + (authoritative_predicted[1] - current[1]) * alpha.clamp(0.0, 1.0),
                ]
            }
        }
        None => authoritative_predicted,
    };
    let predicted = resolve_local_hero_collisions(predicted, world, local_client_id);
    smoothing.predicted_pos = Some(predicted);

    let render_target = predict_local_overstep_position(predicted, input_dir, overstep_fraction);
    let render_target = resolve_local_hero_collisions(render_target, world, local_client_id);
    let render = match smoothing.render_pos {
        Some(current) => {
            if distance_sq(current, render_target) > LOCAL_HERO_SNAP_DISTANCE_SQ {
                render_target
            } else {
                let alpha = 1.0 - (-LOCAL_HERO_RENDER_SMOOTH_RATE * dt).exp();
                [
                    current[0] + (render_target[0] - current[0]) * alpha.clamp(0.0, 1.0),
                    current[1] + (render_target[1] - current[1]) * alpha.clamp(0.0, 1.0),
                ]
            }
        }
        None => render_target,
    };
    smoothing.render_pos = Some(render);
    render
}

fn resolve_local_hero_collisions(
    mut pos: [f32; 2],
    world: &WorldView,
    local_client_id: u64,
) -> [f32; 2] {
    let start = pos;

    for tower in world.towers.values() {
        pos = push_out_of_collider(
            pos,
            tower.pos,
            HERO_COLLIDER_RADIUS + TOWER_COLLIDER_RADIUS,
            LOCAL_HERO_COLLISION_MAX_CORRECTION_PER_STEP,
        );
    }

    for enemy in world.enemies.values() {
        pos = push_out_of_collider(
            pos,
            enemy.pos,
            HERO_COLLIDER_RADIUS + enemy_collider_radius(enemy.enemy_type),
            LOCAL_HERO_COLLISION_MAX_CORRECTION_PER_STEP,
        );
    }

    for hero in world.heroes.values() {
        if hero.client_id == local_client_id {
            continue;
        }
        pos = push_out_of_collider(
            pos,
            hero.pos,
            HERO_COLLIDER_RADIUS * 2.0,
            LOCAL_HERO_COLLISION_MAX_CORRECTION_PER_STEP,
        );
    }

    let limited_total = clamp_delta_length(
        [pos[0] - start[0], pos[1] - start[1]],
        LOCAL_HERO_COLLISION_MAX_TOTAL_CORRECTION,
    );
    clamp_to_world([start[0] + limited_total[0], start[1] + limited_total[1]])
}

fn push_out_of_collider(
    point: [f32; 2],
    center: [f32; 2],
    min_distance: f32,
    max_correction: f32,
) -> [f32; 2] {
    let mut delta = [point[0] - center[0], point[1] - center[1]];
    let mut dist_sq = delta[0] * delta[0] + delta[1] * delta[1];
    let min_distance_sq = min_distance * min_distance;

    if dist_sq >= min_distance_sq {
        return point;
    }

    if dist_sq <= f32::EPSILON {
        delta = [1.0, 0.0];
        dist_sq = 1.0;
    }

    let inv_dist = dist_sq.sqrt().recip();
    let target = [
        center[0] + delta[0] * inv_dist * min_distance,
        center[1] + delta[1] * inv_dist * min_distance,
    ];
    let correction = [target[0] - point[0], target[1] - point[1]];
    let limited = clamp_delta_length(correction, max_correction);
    [point[0] + limited[0], point[1] + limited[1]]
}

fn clamp_delta_length(delta: [f32; 2], max_length: f32) -> [f32; 2] {
    let len_sq = delta[0] * delta[0] + delta[1] * delta[1];
    let max_sq = max_length * max_length;
    if len_sq <= max_sq {
        return delta;
    }

    if len_sq <= f32::EPSILON {
        return [0.0, 0.0];
    }

    let scale = (max_sq / len_sq).sqrt();
    [delta[0] * scale, delta[1] * scale]
}

fn acknowledge_pending_moves(pending_moves: &mut VecDeque<PendingMove>, ack: Option<u32>) {
    let Some(ack) = ack else {
        return;
    };

    while let Some(front) = pending_moves.front() {
        let acknowledged = front.seq == ack || !is_newer_input_seq(front.seq, ack);
        if !acknowledged {
            break;
        }

        pending_moves.pop_front();
    }
}

fn describe_event(event: ReliableGameEvent, local_client_id: u64) -> String {
    match event {
        ReliableGameEvent::WaveStarted { wave } => format!("Wave {wave} incoming"),
        ReliableGameEvent::TowerBuilt {
            owner,
            node_id,
            tower_id,
        } => {
            if owner == local_client_id {
                format!("Tower built at node {node_id} (id {tower_id})")
            } else {
                format!("Player {owner} built tower {tower_id}")
            }
        }
        ReliableGameEvent::BuildRejected {
            owner,
            node_id,
            reason,
        } => {
            if owner == local_client_id {
                format!("Build rejected at node {node_id}: {reason:?}")
            } else {
                format!("Player {owner} build rejected at node {node_id}")
            }
        }
        ReliableGameEvent::AbilityCast { owner, .. } => {
            if owner == local_client_id {
                "Arc Burst cast (AoE around hero)".to_owned()
            } else {
                format!("Player {owner} cast Arc Burst AoE")
            }
        }
        ReliableGameEvent::EnemyKilled { killer, reward, .. } => {
            if killer == local_client_id {
                format!("Enemy defeated (+{reward} gold)")
            } else {
                format!("Player {killer} killed an enemy")
            }
        }
        ReliableGameEvent::ObjectiveDamaged { team_life, .. } => {
            format!("Base hit. Team life: {team_life}")
        }
        ReliableGameEvent::Victory => "Victory".to_owned(),
        ReliableGameEvent::Defeat => "Defeat".to_owned(),
    }
}
