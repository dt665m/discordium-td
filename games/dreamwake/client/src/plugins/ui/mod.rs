//! Dreamwake's interface: scene-composed panels with snapshot-driven HUD data.
use crate::{DreamConnection, DreamPreferences, DreamView};
mod typography;
use bevy::{prelude::*, ui::FocusPolicy, window::PrimaryWindow};
use dreamwake_sim::{
    EnemyKind, MemoryKind, Rarity, Reward, RewardKind, RunPhase, TOTAL_ROOMS, UpgradeKind,
};

const INK: Color = Color::srgb(0.028, 0.044, 0.077);
const PANEL: Color = Color::srgba(0.025, 0.041, 0.071, 0.95);
const WHITE: Color = Color::srgb(0.91, 0.95, 0.95);
const MUTED: Color = Color::srgb(0.53, 0.64, 0.72);
const TEAL: Color = Color::srgb(0.40, 0.94, 0.85);
const GOLD: Color = Color::srgb(0.94, 0.78, 0.46);
const BORDER: Color = Color::srgba(0.39, 0.64, 0.70, 0.30);
const KEYS: [&str; 4] = ["Q", "E", "R", "F"];

#[derive(Clone, Copy, Debug, Default)]
pub enum UiAction {
    Start {
        lucid: bool,
    },
    Choose {
        choice: usize,
        slot: usize,
    },
    Continue,
    Restart,
    #[default]
    TogglePause,
    ToggleBuild,
    ToggleMute,
    SelectSlot(usize),
    Swap(usize, usize),
    BuyMemoryUpgrade(usize),
    Host,
    Connect,
    Disconnect,
}

#[derive(Resource, Default)]
pub struct UiActions(pub Vec<UiAction>);

pub struct DreamUiPlugin;
impl Plugin for DreamUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(typography::DreamTypographyPlugin);
        app.init_resource::<UiActions>()
            .add_systems(Startup, spawn_hud)
            .add_systems(
                PreUpdate,
                route_buttons.in_set(crate::plugins::input::DreamInputSystems::Buttons),
            )
            .add_systems(
                Update,
                (
                    scale_interface,
                    sync_overlay,
                    sync_hud,
                    sync_memory_emblems,
                    sync_party_hud,
                    sync_context_hud,
                )
                    .chain(),
            );
    }
}

#[derive(Component, Default, Clone)]
struct HudRoot;
#[derive(Component, Default, Clone)]
struct ContextHud;
#[derive(Component)]
struct OverlayRoot;
#[derive(Component, Default, Clone)]
struct BossPanel;
#[derive(Component, Default, Clone)]
struct PartyPanel(usize);
#[derive(Component, Default, Clone)]
struct ActionButton(UiAction);
#[derive(Component, Default, Clone)]
struct MemoryFrame(usize);
#[derive(Component, Default, Clone)]
struct MemoryEmblem(usize, Option<MemoryKind>);
#[derive(Component, Default, Clone)]
struct ReadyPulse {
    was_ready: bool,
    until_tick: u32,
}
#[derive(Component, Clone, Copy, Default)]
enum HudText {
    #[default]
    Health,
    Level,
    Shards,
    Objective,
    Progress,
    Boss,
    Dash,
    Combo,
    MemoryName(usize),
    MemoryEssence(usize),
    MemoryCooldown(usize),
    Audio,
    Connection,
    Notice,
    Companion(usize),
    PartyOverflow,
}
#[derive(Component, Clone, Copy, Default)]
enum Fill {
    #[default]
    Health,
    Experience,
    Boss,
    Memory(usize),
    Companion(usize),
}

/// Scene functions keep typography and interaction styling consistent across screens.
fn label(value: impl Into<String>, size: f32, color: Color) -> impl Scene {
    let value: String = value.into();
    bsn! {
        Text(value)
        TextFont { font_size: px(size) }
        TextColor(color)
        template_value(FocusPolicy::Pass)
    }
}

fn button(action: UiAction, value: impl Into<String>, emphasis: bool) -> impl Scene {
    let background = if emphasis {
        Color::srgb(0.15, 0.31, 0.31)
    } else {
        INK
    };
    let border = if emphasis { TEAL } else { BORDER };
    bsn! {
        Button
        ActionButton(action)
        Node {
            min_height: px(48),
            padding: UiRect::axes(px(20), px(12)),
            border: px(1),
            border_radius: BorderRadius::all(px(6)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
        }
        BackgroundColor(background)
        BorderColor::all(border)
        Children [label(value, 16.0, WHITE)]
    }
}

fn meter(kind: Fill, color: Color, height: f32) -> impl Scene {
    bsn! {
        Node { width: percent(100), height: px(height), border_radius: BorderRadius::all(px(3)), overflow: Overflow::clip() }
        BackgroundColor(Color::srgba(0.3, 0.43, 0.5, 0.18))
        Children [(
            template_value(kind)
            Node { width: percent(100), height: percent(100), border_radius: BorderRadius::all(px(3)) }
            BackgroundColor(color)
        )]
    }
}

fn memory_slot(index: usize) -> impl Scene {
    bsn! {
        Button
        ActionButton(UiAction::SelectSlot(index))
        MemoryFrame(index)
        ReadyPulse
        Node {
            width: px(166),
            height: px(112),
            padding: px(12),
            border: px(1),
            border_radius: BorderRadius::all(px(7)),
            row_gap: px(5),
            flex_direction: FlexDirection::Column,
            justify_content: JustifyContent::SpaceBetween,
            overflow: Overflow::clip(),
        }
        BackgroundColor(PANEL)
        BorderColor::all(BORDER)
        Children [
            (
                Node { width: percent(100), justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::Center }
                Children [
                    label(KEYS[index], 13.0, MUTED),
                    (MemoryEmblem(index, None) Node { width: px(24), height: px(24) } template_value(FocusPolicy::Pass)),
                    (label("READY", 11.0, TEAL) template_value(HudText::MemoryCooldown(index))),
                ]
            ),
            (label("Memory", 19.0, WHITE) template_value(HudText::MemoryName(index))),
            (label("No Essence", 11.0, MUTED) template_value(HudText::MemoryEssence(index))),
            meter(Fill::Memory(index), TEAL, 3.0),
        ]
    }
}

fn companion_panel(index: usize) -> impl Scene {
    bsn! {
        PartyPanel(index)
        Node { width: px(220), padding: px(11), row_gap: px(7), flex_direction: FlexDirection::Column, border_radius: BorderRadius::all(px(6)) }
        BackgroundColor(PANEL)
        template_value(FocusPolicy::Pass)
        Children [
            (label("COMPANION", 11.0, WHITE) template_value(HudText::Companion(index))),
            meter(Fill::Companion(index), Color::srgb(0.67, 0.66, 1.0), 4.0),
        ]
    }
}

fn spawn_hud(mut commands: Commands) {
    commands.spawn_scene(bsn! {
        HudRoot
        Node { width: percent(100), height: percent(100), position_type: PositionType::Absolute }
        GlobalZIndex(10)
        template_value(FocusPolicy::Pass)
        Children [
            (
                ContextHud
                Node {
                    position_type: PositionType::Absolute, left: px(28), top: px(26), width: px(285),
                    padding: px(16), row_gap: px(9), flex_direction: FlexDirection::Column,
                    border: px(1), border_radius: BorderRadius::all(px(8)),
                }
                BackgroundColor(PANEL)
                BorderColor::all(BORDER)
                Children [
                    (
                        Node { justify_content: JustifyContent::SpaceBetween }
                        Children [label("VESPER", 14.0, TEAL), (label("LEVEL 1", 12.0, GOLD) template_value(HudText::Level))]
                    ),
                    (label("220 / 220", 25.0, WHITE) template_value(HudText::Health)),
                    meter(Fill::Health, TEAL, 7.0),
                    meter(Fill::Experience, GOLD, 3.0),
                    (label("MOONBOUND  ·  0 / 3", 10.0, MUTED) template_value(HudText::Combo)),
                ]
            ),
            (
                ContextHud
                Node { position_type: PositionType::Absolute, left: px(28), top: px(190), row_gap: px(7), flex_direction: FlexDirection::Column }
                template_value(FocusPolicy::Pass)
                Children [
                    companion_panel(0), companion_panel(1), companion_panel(2), companion_panel(3),
                    companion_panel(4), companion_panel(5), companion_panel(6),
                    (label("", 11.0, MUTED) template_value(HudText::PartyOverflow)),
                ]
            ),
            (
                ContextHud
                Node {
                    position_type: PositionType::Absolute, left: percent(32), right: percent(32), top: px(31),
                    flex_direction: FlexDirection::Column, align_items: AlignItems::Center, row_gap: px(7),
                }
                Children [
                    label("D R E A M W A K E", 11.0, GOLD),
                    (label("THE GLASS GARDEN", 21.0, WHITE) template_value(HudText::Objective)),
                    (label("DREAM 01 / 10", 11.0, MUTED) template_value(HudText::Progress)),
                ]
            ),
            (
                Node {
                    position_type: PositionType::Absolute, right: px(28), top: px(28),
                    flex_direction: FlexDirection::Column, align_items: AlignItems::End, row_gap: px(12),
                }
                Children [
                    (label("0  DREAM SHARDS", 14.0, GOLD) template_value(HudText::Shards)),
                    (label("CONNECTING", 10.0, MUTED) template_value(HudText::Connection)),
                    (label("", 11.0, GOLD) template_value(HudText::Notice) Node { max_width: px(270) }),
                    (
                        ContextHud
                        Node { column_gap: px(7) }
                        Children [
                            button(UiAction::ToggleBuild, "Tab  Build", false),
                            button(UiAction::TogglePause, "Esc  Pause", false),
                        ]
                    ),
                    (
                        ContextHud
                        Button ActionButton(UiAction::ToggleMute)
                        Node { padding: px(6), border: px(0) }
                        BackgroundColor(Color::NONE) BorderColor::all(Color::NONE)
                        Children [(label("M  Sound on", 11.0, MUTED) template_value(HudText::Audio))]
                    ),
                ]
            ),
            (
                BossPanel
                Node {
                    position_type: PositionType::Absolute, left: percent(32), right: percent(32), top: px(110),
                    flex_direction: FlexDirection::Column, align_items: AlignItems::Center, row_gap: px(8),
                }
                Children [
                    (label("THE SOMNARCH", 14.0, GOLD) template_value(HudText::Boss)),
                    meter(Fill::Boss, Color::srgb(0.94, 0.36, 0.45), 6.0),
                ]
            ),
            (
                Node {
                    position_type: PositionType::Absolute, left: px(0), right: px(0), bottom: px(23),
                    flex_direction: FlexDirection::Column, align_items: AlignItems::Center, row_gap: px(11),
                }
                Children [
                    (
                        Node { column_gap: px(10), align_items: AlignItems::End }
                        Children [memory_slot(0), memory_slot(1), memory_slot(2), memory_slot(3)]
                    ),
                    (
                        Node { column_gap: px(23), align_items: AlignItems::Center }
                        Children [
                            label("W A S D   Move", 11.0, MUTED),
                            label("LMB   Slash", 11.0, MUTED),
                            (label("SPACE   Dash ready", 11.0, TEAL) template_value(HudText::Dash)),
                            label("TAB   Build  /  ESC   Pause  /  F3   Metrics", 11.0, MUTED),
                        ]
                    ),
                ]
            ),
        ]
    });
}

fn scale_interface(windows: Query<&Window, With<PrimaryWindow>>, mut scale: ResMut<UiScale>) {
    if let Ok(window) = windows.single() {
        let desired = (window.width() / 1160.0)
            .min(window.height() / 820.0)
            .clamp(0.5, 1.4);
        if (scale.0 - desired).abs() > 0.001 {
            scale.0 = desired;
        }
    }
}

fn sync_context_hud(
    view: Res<DreamView>,
    preferences: Res<DreamPreferences>,
    mut nodes: Query<&mut Visibility, With<ContextHud>>,
) {
    let visible =
        view.0.phase == RunPhase::Combat && !preferences.paused && !preferences.build_open;
    for mut visibility in &mut nodes {
        *visibility = if visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}

fn route_buttons(
    mut actions: ResMut<UiActions>,
    preferences: Res<DreamPreferences>,
    mut buttons: Query<
        (
            &Interaction,
            &ActionButton,
            &mut BackgroundColor,
            &mut BorderColor,
        ),
        Changed<Interaction>,
    >,
) {
    for (interaction, action, mut background, mut border) in &mut buttons {
        match interaction {
            Interaction::Pressed => {
                *background = BackgroundColor(Color::srgb(0.20, 0.41, 0.40));
                *border = BorderColor::all(TEAL);
                let action = match action.0 {
                    UiAction::Choose { choice, .. } => UiAction::Choose {
                        choice,
                        slot: preferences.selected_slot.min(3),
                    },
                    action => action,
                };
                actions.0.push(action);
            }
            Interaction::Hovered => {
                *background = BackgroundColor(Color::srgb(0.09, 0.18, 0.23));
                *border = BorderColor::all(TEAL);
            }
            Interaction::None => {
                *background = BackgroundColor(INK);
                *border = BorderColor::all(BORDER);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn sync_hud(
    view: Res<DreamView>,
    connection: Res<DreamConnection>,
    preferences: Res<DreamPreferences>,
    mut roots: Query<&mut Visibility, With<HudRoot>>,
    mut texts: Query<(&HudText, &mut Text, &mut TextColor)>,
    mut fills: Query<(&Fill, &mut Node, &mut BackgroundColor), Without<MemoryFrame>>,
    mut bosses: Query<&mut Node, (With<BossPanel>, Without<Fill>)>,
    mut memory_frames: Query<(
        &MemoryFrame,
        &Interaction,
        &mut BorderColor,
        &mut BackgroundColor,
        &mut ReadyPulse,
    )>,
) {
    let snap = &view.0;
    let hero = &snap.hero;
    let companions: Vec<_> = snap
        .heroes
        .iter()
        .filter(|companion| companion.id != hero.id)
        .collect();
    let boss = snap.enemies.iter().find(|e| e.kind == EnemyKind::Boss);
    for mut visibility in &mut roots {
        *visibility = if preferences.paused
            || preferences.build_open
            || matches!(
                snap.phase,
                RunPhase::Intro | RunPhase::Victory | RunPhase::Defeat
            ) {
            Visibility::Hidden
        } else {
            Visibility::Visible
        };
    }
    for mut node in &mut bosses {
        node.display = if boss.is_some() {
            Display::Flex
        } else {
            Display::None
        };
    }
    for (kind, mut text, mut color) in &mut texts {
        let value = match *kind {
            HudText::Health => {
                if hero.shield > 0.5 {
                    format!(
                        "{:.0} / {:.0}   +{:.0}",
                        hero.hp.max(0.0),
                        hero.max_hp,
                        hero.shield
                    )
                } else {
                    format!("{:.0} / {:.0}", hero.hp.max(0.0), hero.max_hp)
                }
            }
            HudText::Level => format!("LEVEL {}", hero.level),
            HudText::Shards => format!("{}  DREAM SHARDS", hero.shards),
            HudText::Objective => {
                if boss.is_some() {
                    "THE UNWAKING THRONE".into()
                } else {
                    snap.encounter_name.to_uppercase()
                }
            }
            HudText::Progress if hero.hp <= 0.0 && snap.phase == RunPhase::Combat => {
                "DOWNED  /  YOUR COMPANIONS FIGHT ON".into()
            }
            HudText::Progress => format!(
                "DREAM {:02} / {:02}  ·  {}",
                snap.room + 1,
                TOTAL_ROOMS,
                match snap.phase {
                    RunPhase::Combat => format!("{} FOES REMAIN", snap.enemies_remaining),
                    RunPhase::Reward => "CHOOSE YOUR REWARD".into(),
                    RunPhase::Rest => "A MOMENT OF STILLNESS".into(),
                    RunPhase::Transition => "ENCOUNTER COMPLETE".into(),
                    _ => String::new(),
                }
            ),
            HudText::Boss => boss
                .map(|b| format!("THE SOMNARCH  /  PHASE {}", b.phase.max(1)))
                .unwrap_or_default(),
            HudText::Dash => {
                if hero.dash_cooldown > 0.0 {
                    format!("SPACE   Dash {:.1}s", hero.dash_cooldown)
                } else {
                    "SPACE   Dash ready".into()
                }
            }
            HudText::Combo => format!(
                "MOONBOUND  ·  {} / 3  ·  THIRD STRIKE RESTORES",
                hero.combo % 3
            ),
            HudText::MemoryName(i) => {
                let rgb = hero.memories[i].kind.color();
                color.0 = Color::srgb(rgb[0], rgb[1], rgb[2]);
                hero.memories[i].kind.name().into()
            }
            HudText::MemoryEssence(i) => format!(
                "RANK {}  /  {}",
                hero.memories[i].level,
                hero.memories[i]
                    .essence
                    .map(|e| e.name())
                    .unwrap_or("No Essence")
            ),
            HudText::MemoryCooldown(i) => {
                let slot = hero.memories[i];
                color.0 = if slot.ready() { TEAL } else { MUTED };
                if slot.ready() {
                    "READY".into()
                } else {
                    format!("{:.1}s", slot.cooldown)
                }
            }
            HudText::Connection => {
                if connection.connected {
                    format!(
                        "ONLINE  /  {} TRAVELERS  /  {:.0} ms",
                        connection.party_size.max(1),
                        connection.rtt_ms
                    )
                } else {
                    "RECONNECTING TO THE DREAM".into()
                }
            }
            HudText::Notice => connection.status.clone(),
            HudText::PartyOverflow => {
                if companions.len() > 7 {
                    format!("+ {} MORE COMPANIONS", companions.len() - 7)
                } else {
                    String::new()
                }
            }
            HudText::Companion(index) => companions
                .get(index)
                .map(|hero| {
                    color.0 = if hero.hp <= 0.0 {
                        MUTED
                    } else {
                        crate::plugins::graphics::scene::companion_color(hero.id)
                    };
                    if hero.hp <= 0.0 {
                        format!("VESPER {:08X}  /  DOWNED", hero.id & 0xffff_ffff)
                    } else {
                        format!("VESPER {:08X}  /  {:.0} HP", hero.id & 0xffff_ffff, hero.hp)
                    }
                })
                .unwrap_or_default(),
            HudText::Audio => {
                if preferences.muted {
                    "M  Sound muted".into()
                } else {
                    "M  Sound on".into()
                }
            }
        };
        if text.0 != value {
            text.0 = value;
        }
    }
    for (kind, mut node, mut background) in &mut fills {
        let fraction = match *kind {
            Fill::Health => {
                background.0 = if hero.hp < hero.max_hp * 0.30 {
                    Color::srgb(0.97, 0.38, 0.40)
                } else {
                    TEAL
                };
                hero.hp / hero.max_hp.max(1.0)
            }
            Fill::Experience => hero.xp / hero.xp_next.max(1.0),
            Fill::Boss => boss.map(|b| b.hp / b.max_hp.max(1.0)).unwrap_or(0.0),
            Fill::Companion(index) => companions
                .get(index)
                .map(|h| {
                    background.0 = crate::plugins::graphics::scene::companion_color(h.id);
                    h.hp / h.max_hp.max(1.0)
                })
                .unwrap_or(0.0),
            Fill::Memory(i) => {
                let rgb = hero.memories[i].kind.color();
                background.0 = Color::srgb(rgb[0], rgb[1], rgb[2]);
                1.0 - hero.memories[i].cooldown / hero.memories[i].max_cooldown.max(0.01)
            }
        };
        node.width = percent(fraction.clamp(0.0, 1.0) * 100.0);
    }
    for (frame, interaction, mut border, mut background, mut pulse) in &mut memory_frames {
        let ready = hero.memories[frame.0].ready();
        if ready && !pulse.was_ready {
            pulse.until_tick = snap.tick + 22;
        }
        pulse.was_ready = ready;
        if *interaction == Interaction::None {
            let remaining = pulse.until_tick.saturating_sub(snap.tick) as f32 / 22.0;
            background.0 = if remaining > 0.0 {
                Color::srgba(
                    0.045 + remaining * 0.11,
                    0.085 + remaining * 0.18,
                    0.115 + remaining * 0.15,
                    0.96,
                )
            } else {
                PANEL
            };
        }
        let selecting =
            matches!(snap.phase, RunPhase::Reward | RunPhase::Rest) || preferences.build_open;
        *border = BorderColor::all(
            if *interaction != Interaction::None
                || (selecting && frame.0 == preferences.selected_slot)
            {
                TEAL
            } else {
                BORDER
            },
        );
    }
}

fn sync_party_hud(view: Res<DreamView>, mut panels: Query<(&PartyPanel, &mut Node)>) {
    let count = view
        .0
        .heroes
        .iter()
        .filter(|hero| hero.id != view.0.hero.id)
        .count();
    for (panel, mut node) in &mut panels {
        node.display = if panel.0 < count {
            Display::Flex
        } else {
            Display::None
        };
    }
}

/// Tiny geometric sigils retain a recognizable silhouette without font glyph assets.
fn sigil_part(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    angle: f32,
    stroke: f32,
    fill: Color,
    accent: Color,
) -> impl Scene {
    bsn! {
        Node {
            position_type: PositionType::Absolute, left: px(x), top: px(y), width: px(w), height: px(h),
            border: px(stroke), border_radius: BorderRadius::all(px(radius)),
        }
        UiTransform::from_rotation(Rot2::degrees(angle))
        BackgroundColor(fill)
        BorderColor::all(accent)
        template_value(FocusPolicy::Pass)
    }
}

fn sync_memory_emblems(
    mut commands: Commands,
    view: Res<DreamView>,
    mut emblems: Query<(Entity, &mut MemoryEmblem, Option<&Children>)>,
) {
    for (entity, mut emblem, children) in &mut emblems {
        let kind = view.0.hero.memories[emblem.0].kind;
        if emblem.1 == Some(kind) {
            continue;
        }
        emblem.1 = Some(kind);
        if let Some(children) = children {
            for child in children.iter() {
                commands.entity(child).despawn();
            }
        }
        let rgb = kind.color();
        let accent = Color::srgb(rgb[0], rgb[1], rgb[2]);
        // x, y, width, height, corner radius, rotation, stroke, filled.
        let pieces = match kind {
            MemoryKind::Crescent => [
                (2., 2., 19., 19., 12., 0., 2., false),
                (8., -1., 16., 16., 12., 0., 0., false),
                (1., 19., 3., 3., 3., 0., 0., true),
            ],
            MemoryKind::Starfall => [
                (7., 3., 12., 12., 0., 45., 2., false),
                (2., 15., 12., 2., 0., -45., 0., true),
                (18., 0., 3., 3., 0., 45., 0., true),
            ],
            MemoryKind::Nova => [
                (2., 2., 20., 20., 12., 0., 1., false),
                (7., 7., 10., 10., 0., 45., 1., false),
                (10., 10., 4., 4., 4., 0., 0., true),
            ],
            MemoryKind::Blink => [
                (2., 6., 12., 12., 1., 45., 2., false),
                (11., 6., 12., 12., 1., 45., 2., false),
                (0., 0., 2., 2., 0., 0., 0., true),
            ],
            MemoryKind::Aegis => [
                (4., 2., 17., 19., 5., 0., 2., false),
                (10., 7., 4., 8., 2., 0., 0., true),
                (8., 9., 8., 3., 1., 0., 0., true),
            ],
            MemoryKind::Wisp => [
                (5., 1., 15., 15., 10., 0., 2., false),
                (10., 14., 4., 8., 2., -18., 0., true),
                (0., 3., 3., 3., 3., 0., 0., true),
            ],
        };
        for (i, (x, y, w, h, r, a, stroke, filled)) in pieces.into_iter().enumerate() {
            let fill = if filled {
                accent
            } else if kind == MemoryKind::Crescent && i == 1 {
                PANEL
            } else {
                Color::NONE
            };
            commands
                .spawn_scene(sigil_part(x, y, w, h, r, a, stroke, fill, accent))
                .insert(ChildOf(entity));
        }
    }
}

fn panel_node(width: f32) -> Node {
    Node {
        width: px(width),
        padding: UiRect::all(px(32)),
        border: UiRect::all(px(1)),
        border_radius: BorderRadius::all(px(12)),
        row_gap: px(18),
        flex_direction: FlexDirection::Column,
        ..default()
    }
}

fn add_label(
    commands: &mut Commands,
    parent: Entity,
    text: impl Into<String>,
    size: f32,
    color: Color,
) {
    commands
        .spawn_scene(label(text, size, color))
        .insert(ChildOf(parent));
}
fn add_button(
    commands: &mut Commands,
    parent: Entity,
    action: UiAction,
    text: impl Into<String>,
    emphasis: bool,
) {
    commands
        .spawn_scene(button(action, text, emphasis))
        .insert(ChildOf(parent));
}
fn row(commands: &mut Commands, parent: Entity, gap: f32) -> Entity {
    commands
        .spawn((
            Node {
                column_gap: px(gap),
                align_items: AlignItems::Stretch,
                ..default()
            },
            ChildOf(parent),
        ))
        .id()
}
fn panel(commands: &mut Commands, parent: Entity, width: f32) -> Entity {
    commands
        .spawn((
            panel_node(width),
            BackgroundColor(PANEL),
            BorderColor::all(BORDER),
            ChildOf(parent),
        ))
        .id()
}

fn sync_overlay(
    mut commands: Commands,
    view: Res<DreamView>,
    preferences: Res<DreamPreferences>,
    connection: Res<DreamConnection>,
    roots: Query<Entity, With<OverlayRoot>>,
    mut previous: Local<String>,
) {
    let snap = &view.0;
    // Only rebuild dynamic card hierarchies when their contents or screen changes.
    let key = format!(
        "{:?}:{}:{}:{}:{}:{}:{}:{}:{:?}:{:?}",
        snap.phase,
        snap.room,
        preferences.paused,
        preferences.build_open,
        preferences.muted,
        preferences.selected_slot,
        snap.hero.shards,
        snap.message,
        snap.rewards,
        snap.hero.memories.map(|s| (s.kind, s.level, s.essence))
    );
    let key = format!(
        "{key}:{}:{}:{}:{}:{}:{}:{}:{}",
        connection.connected,
        connection.status,
        connection.client_id,
        connection.party_size,
        snap.ready,
        snap.awaiting_party,
        connection.http_base,
        connection.share_url
    );
    if *previous == key {
        return;
    }
    *previous = key;
    for entity in &roots {
        commands.entity(entity).despawn();
    }
    if connection.connected
        && snap.phase == RunPhase::Combat
        && !preferences.paused
        && !preferences.build_open
    {
        return;
    }
    let intro = snap.phase == RunPhase::Intro;
    let root = commands
        .spawn((
            OverlayRoot,
            Node {
                width: percent(100),
                height: percent(100),
                position_type: PositionType::Absolute,
                left: px(0),
                top: px(0),
                justify_content: if intro {
                    JustifyContent::Start
                } else {
                    JustifyContent::Center
                },
                align_items: AlignItems::Center,
                padding: if intro {
                    UiRect::left(px(70))
                } else {
                    UiRect::ZERO
                },
                ..default()
            },
            BackgroundColor(if intro {
                Color::srgba(0.015, 0.025, 0.06, 0.20)
            } else {
                Color::srgba(0.012, 0.022, 0.046, 0.64)
            }),
            GlobalZIndex(20),
        ))
        .id();
    if intro {
        intro_panel(&mut commands, root, &connection, snap);
    } else if !connection.connected {
        connection_panel(&mut commands, root, &connection);
    } else if matches!(snap.phase, RunPhase::Victory | RunPhase::Defeat) {
        end_panel(&mut commands, root, snap, &connection);
    } else if preferences.build_open {
        build_panel(
            &mut commands,
            root,
            snap,
            preferences.selected_slot.min(3),
            &connection,
        );
    } else if preferences.paused {
        pause_panel(&mut commands, root, preferences.muted, &connection);
    } else if snap.awaiting_party && matches!(snap.phase, RunPhase::Reward | RunPhase::Rest) {
        waiting_panel(&mut commands, root);
    } else {
        match snap.phase {
            RunPhase::Reward | RunPhase::Rest => {
                reward_panel(&mut commands, root, snap, preferences.selected_slot.min(3))
            }
            RunPhase::Transition => transition_panel(&mut commands, root, snap),
            _ => {}
        }
    }
}

fn intro_panel(
    commands: &mut Commands,
    root: Entity,
    connection: &DreamConnection,
    _snap: &dreamwake_sim::DreamSnapshot,
) {
    let card = panel(commands, root, 560.0);
    add_label(
        commands,
        card,
        "A PLAYABLE DREAM  /  ACTION ROGUELITE",
        11.0,
        GOLD,
    );
    add_label(commands, card, "DREAMWAKE", 61.0, WHITE);
    add_label(commands, card, "The place between waking.", 23.0, TEAL);
    add_label(
        commands,
        card,
        "Cross ten fractured dreams. Collect impossible powers. Wake the world before the Somnarch consumes it.",
        17.0,
        MUTED,
    );
    let traveler = commands
        .spawn((
            Node {
                padding: UiRect::axes(px(18), px(17)),
                flex_direction: FlexDirection::Column,
                row_gap: px(8),
                border: UiRect::left(px(2)),
                ..default()
            },
            BorderColor::all(TEAL),
            BackgroundColor(Color::srgba(0.12, 0.27, 0.29, 0.28)),
            ChildOf(card),
        ))
        .id();
    add_label(
        commands,
        traveler,
        "VESPER  /  MOONBOUND DUELIST",
        17.0,
        WHITE,
    );
    add_label(
        commands,
        traveler,
        "Every third strike that hits restores 5 health and cuts all Memory cooldowns by 0.5 seconds. Stay close; keep the rhythm.",
        15.0,
        MUTED,
    );
    add_label(
        commands,
        card,
        "WASD to move  ·  Mouse to aim  ·  LMB to strike\nSpace to dash  ·  Q / E / R / F to cast Memories",
        14.0,
        WHITE,
    );
    add_label(
        commands,
        card,
        &connection.status,
        13.0,
        if connection.connected { TEAL } else { GOLD },
    );
    add_label(
        commands,
        card,
        format!("SERVER  /  {}", connection.http_base),
        11.0,
        MUTED,
    );
    if connection.connected && !connection.share_url.is_empty() {
        add_label(
            commands,
            card,
            format!("INVITE COMPANIONS  /  {}", connection.share_url),
            12.0,
            TEAL,
        );
    }
    if !connection.connected {
        #[cfg(not(target_arch = "wasm32"))]
        add_button(
            commands,
            card,
            UiAction::Host,
            "HOST A DREAM     /     Create a party",
            true,
        );
        add_button(
            commands,
            card,
            UiAction::Connect,
            "JOIN SERVER     /     Connect to the dream",
            cfg!(target_arch = "wasm32"),
        );
        add_label(
            commands,
            card,
            "Join a server and enter the dream. Friends can drop in at any time. Eight Travelers by default.",
            12.0,
            MUTED,
        );
    } else {
        add_button(
            commands,
            card,
            UiAction::Start { lucid: false },
            format!(
                "ENTER THE DREAM     /     {} Traveler{}",
                connection.party_size.max(1),
                if connection.party_size == 1 { "" } else { "s" }
            ),
            true,
        );
        add_button(
            commands,
            card,
            UiAction::Start { lucid: true },
            "LUCID DREAM     /     Greater danger, richer rewards",
            false,
        );
        add_label(
            commands,
            card,
            "Start now. Friends can join this dream at any time.",
            11.0,
            MUTED,
        );
    }
}

fn pause_panel(commands: &mut Commands, root: Entity, muted: bool, connection: &DreamConnection) {
    let card = panel(commands, root, 470.0);
    add_label(commands, card, "THE DREAM CAN WAIT", 12.0, GOLD);
    add_label(commands, card, "A moment between.", 33.0, WHITE);
    add_label(
        commands,
        card,
        if connection.party_size > 1 {
            "Your online party continues while this menu is open."
        } else {
            "Paused"
        },
        16.0,
        if connection.party_size > 1 {
            GOLD
        } else {
            MUTED
        },
    );
    add_button(
        commands,
        card,
        UiAction::TogglePause,
        "RESUME     /     Esc",
        true,
    );
    add_button(
        commands,
        card,
        UiAction::ToggleBuild,
        "VIEW MEMORIES     /     Tab",
        false,
    );
    add_button(
        commands,
        card,
        UiAction::ToggleMute,
        if muted {
            "ENABLE SOUND     /     M"
        } else {
            "MUTE SOUND     /     M"
        },
        false,
    );
    if connection.party_size <= 1 {
        add_button(
            commands,
            card,
            UiAction::Restart,
            "BEGIN A NEW DREAM",
            false,
        );
    }
    add_button(commands, card, UiAction::Disconnect, "LEAVE PARTY", false);
    add_label(
        commands,
        card,
        "WASD Move  ·  LMB Strike  ·  Space Dash\nQ E R F Memories  ·  + / - Zoom\n1 2 3 Choose reward  ·  Z X C V Target slot\nCONTROLLER\nLeft / right stick Move / aim · RT Slash · LB Dash\nA / X / Y / B Memories · Start Pause · Back Build\nD-pad left / right Target · A / X / Y Reward choices",
        13.0,
        MUTED,
    );
}

fn connection_panel(commands: &mut Commands, root: Entity, connection: &DreamConnection) {
    let card = panel(commands, root, 510.0);
    add_label(commands, card, "A THREAD BETWEEN WORLDS", 12.0, GOLD);
    add_label(commands, card, "Rejoin the dream.", 37.0, WHITE);
    add_label(commands, card, &connection.status, 16.0, MUTED);
    add_label(commands, card, &connection.http_base, 13.0, TEAL);
    add_button(commands, card, UiAction::Connect, "RETRY CONNECTION", true);
    add_button(
        commands,
        card,
        UiAction::Disconnect,
        "RETURN TO THE THRESHOLD",
        false,
    );
}

fn waiting_panel(commands: &mut Commands, root: Entity) {
    let card = panel(commands, root, 520.0);
    add_label(commands, card, "YOUR REWARD IS WOVEN", 12.0, GOLD);
    add_label(commands, card, "A shared dream.", 37.0, WHITE);
    add_label(
        commands,
        card,
        "Your companions are choosing their rewards. The way opens when everyone is ready.",
        17.0,
        MUTED,
    );
    add_label(commands, card, "WAITING FOR COMPANIONS", 14.0, TEAL);
    add_button(
        commands,
        card,
        UiAction::ToggleBuild,
        "VIEW YOUR MEMORIES     /     Tab",
        false,
    );
}

fn reward_panel(
    commands: &mut Commands,
    root: Entity,
    snap: &dreamwake_sim::DreamSnapshot,
    selected: usize,
) {
    // Leave the always-visible Memory tray unobscured and interactive beneath rewards.
    let rest = snap.phase == RunPhase::Rest;
    commands.entity(root).insert((
        FocusPolicy::Pass,
        Node {
            width: percent(100),
            height: percent(100),
            position_type: PositionType::Absolute,
            left: px(0),
            top: px(0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            padding: UiRect::bottom(px(175)),
            ..default()
        },
    ));
    let area = commands
        .spawn((
            Node {
                width: px(1010),
                // A definite vertical budget avoids intrinsic text measurement
                // retaining an oversized column after a browser scale change.
                height: px(if rest { 620 } else { 550 }),
                flex_shrink: 0.0,
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: px(17),
                ..default()
            },
            ChildOf(root),
        ))
        .id();
    add_label(
        commands,
        area,
        if rest {
            "SANCTUARY  /  THE QUIET MARKET"
        } else {
            "THE DREAM REMEMBERS YOU"
        },
        12.0,
        GOLD,
    );
    add_label(
        commands,
        area,
        if rest {
            "Rest. Reforge. Return."
        } else {
            "Choose what you become."
        },
        36.0,
        WHITE,
    );
    add_label(
        commands,
        area,
        if rest {
            "50% health restored. Refine a Memory with shards, then choose one free blessing."
                .into()
        } else {
            format!(
                "Choose one reward. Memory and Essence choices apply to {} [{}].",
                snap.hero.memories[selected].kind.name(),
                KEYS[selected]
            )
        },
        15.0,
        MUTED,
    );
    let cards = row(commands, area, 16.0);
    for (index, reward) in snap.rewards.iter().take(3).enumerate() {
        let accent = match reward.rarity {
            Rarity::Common => TEAL,
            Rarity::Rare => Color::srgb(0.47, 0.69, 1.0),
            Rarity::Epic => Color::srgb(0.81, 0.55, 1.0),
        };
        let card = commands
            .spawn((
                Button,
                ActionButton(UiAction::Choose {
                    choice: index,
                    slot: selected,
                }),
                Node {
                    width: px(326),
                    min_height: px(355),
                    padding: UiRect::all(px(23)),
                    border: UiRect::all(px(1)),
                    border_radius: BorderRadius::all(px(9)),
                    flex_direction: FlexDirection::Column,
                    row_gap: px(15),
                    ..default()
                },
                BackgroundColor(PANEL),
                BorderColor::all(accent),
                ChildOf(cards),
            ))
            .id();
        let at_rank_cap = matches!(reward.kind, RewardKind::Memory(kind) if kind == snap.hero.memories[selected].kind && snap.hero.memories[selected].level >= 8);
        if at_rank_cap {
            commands.entity(card).remove::<(Button, ActionButton)>();
        }
        let category = match reward.kind {
            RewardKind::Memory(_) => "MEMORY",
            RewardKind::Essence(_) => "ESSENCE",
            RewardKind::Upgrade(_) => "TRAVELER UPGRADE",
        };
        add_label(
            commands,
            card,
            format!(
                "0{}  /  {} {}",
                index + 1,
                reward.rarity.name().to_uppercase(),
                category
            ),
            11.0,
            accent,
        );
        add_label(commands, card, reward.name(), 26.0, WHITE);
        add_label(commands, card, reward.description(), 15.0, WHITE);
        let comparison = commands
            .spawn((
                Node {
                    margin: UiRect::top(px(2)),
                    padding: UiRect::all(px(12)),
                    flex_grow: 1.0,
                    flex_direction: FlexDirection::Column,
                    row_gap: px(7),
                    border_radius: BorderRadius::all(px(5)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.10, 0.18, 0.24, 0.65)),
                ChildOf(card),
            ))
            .id();
        add_label(commands, comparison, "YOUR BUILD", 10.0, accent);
        add_label(
            commands,
            comparison,
            reward_comparison(reward, snap, selected),
            13.0,
            MUTED,
        );
        add_label(
            commands,
            card,
            if at_rank_cap {
                "MAX RANK  /  TARGET ANOTHER SLOT".into()
            } else {
                format!("CHOOSE     /     {}", index + 1)
            },
            12.0,
            accent,
        );
    }
    add_label(
        commands,
        area,
        format!(
            "TARGET SLOT:  {}  /  {}    ·    Click a Memory below or press Z / X / C / V",
            KEYS[selected],
            snap.hero.memories[selected].kind.name()
        ),
        13.0,
        TEAL,
    );
    if rest {
        let shop = row(commands, area, 14.0);
        if snap.hero.shards >= 45 && snap.hero.memories[selected].level < 8 {
            add_button(
                commands,
                shop,
                UiAction::BuyMemoryUpgrade(selected),
                format!(
                    "REFINE {} +1 RANK   /   45 SHARDS",
                    snap.hero.memories[selected].kind.name().to_uppercase()
                ),
                true,
            );
        } else {
            add_label(
                commands,
                shop,
                if snap.hero.memories[selected].level >= 8 {
                    "Selected Memory is at maximum rank"
                } else {
                    "Memory refinement costs 45 shards"
                },
                13.0,
                MUTED,
            );
        }
        add_button(
            commands,
            shop,
            UiAction::Continue,
            "CONTINUE     /     Enter",
            false,
        );
    }
}

fn reward_comparison(reward: &Reward, snap: &dreamwake_sim::DreamSnapshot, slot: usize) -> String {
    let hero = &snap.hero;
    match reward.kind {
        RewardKind::Upgrade(kind) => match kind {
            UpgradeKind::Attack => format!(
                "Basic attack power: {:.0}% → {:.0}%.",
                hero.attack_power * 100.0,
                (hero.attack_power + 0.30) * 100.0
            ),
            UpgradeKind::Ability => format!(
                "Memory power: {:.0}% → {:.0}%. Improves damage and shielding.",
                hero.ability_power * 100.0,
                (hero.ability_power + 0.28) * 100.0
            ),
            UpgradeKind::Movement => format!(
                "Movement: {:.1} → {:.1}. Dash becomes ready immediately.",
                hero.movement_speed,
                (hero.movement_speed * 1.14).min(13.0)
            ),
            UpgradeKind::Critical => format!(
                "Critical chance: {:.0}% → {:.0}%. Critical hits deal 190% damage.",
                hero.critical_chance * 100.0,
                (hero.critical_chance + 0.15).min(0.85) * 100.0
            ),
            UpgradeKind::Recovery => format!(
                "Cooldown duration: {:.0}% → {:.0}% of base. Current cooldowns also shrink.",
                hero.recovery * 100.0,
                (hero.recovery * 0.82).max(0.28) * 100.0
            ),
            UpgradeKind::Health => format!(
                "Maximum health: {:.0} → {:.0}. Health now: {:.0} → {:.0}.",
                hero.max_hp,
                hero.max_hp + 50.0,
                hero.hp,
                (hero.hp + 75.0).min(hero.max_hp + 50.0)
            ),
            UpgradeKind::Defense => format!(
                "Damage reduction: {:.0}% → {:.0}%. Gain 35 shield immediately.",
                hero.defense * 100.0,
                (hero.defense + 0.10).min(0.60) * 100.0
            ),
        },
        _ => reward.comparison(&hero.memories[slot]),
    }
}

fn build_panel(
    commands: &mut Commands,
    root: Entity,
    snap: &dreamwake_sim::DreamSnapshot,
    selected: usize,
    connection: &DreamConnection,
) {
    let card = panel(commands, root, 1070.0);
    commands.entity(card).insert(Node {
        height: px(740),
        ..panel_node(1070.0)
    });
    add_label(
        commands,
        card,
        "VESPER  /  THE MEMORIES YOU CARRY",
        12.0,
        GOLD,
    );
    add_label(commands, card, "Every dream leaves a mark.", 34.0, WHITE);
    add_label(
        commands,
        card,
        format!(
            "ATK {:.0}%   ·   MEMORY {:.0}%   ·   SPEED {:.1}   ·   CRIT {:.0}%   ·   COOLDOWN {:.0}%   ·   DEFENSE {:.0}%",
            snap.hero.attack_power * 100.0,
            snap.hero.ability_power * 100.0,
            snap.hero.movement_speed,
            snap.hero.critical_chance * 100.0,
            snap.hero.recovery * 100.0,
            snap.hero.defense * 100.0
        ),
        12.0,
        TEAL,
    );
    let slots = row(commands, card, 12.0);
    for (i, memory) in snap.hero.memories.iter().enumerate() {
        let column = panel(commands, slots, 242.0);
        commands.entity(column).insert(Node {
            width: px(242),
            padding: UiRect::all(px(16)),
            row_gap: px(13),
            flex_direction: FlexDirection::Column,
            border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(px(8)),
            ..default()
        });
        let rgb = memory.kind.color();
        let color = Color::srgb(rgb[0], rgb[1], rgb[2]);
        add_label(
            commands,
            column,
            format!("{}  /  RANK {}", KEYS[i], memory.level),
            12.0,
            color,
        );
        add_label(commands, column, memory.kind.name(), 24.0, WHITE);
        add_label(commands, column, memory.kind.description(), 14.0, MUTED);
        add_label(
            commands,
            column,
            format!(
                "+{}% rank power\n{:.1}s recovery",
                (memory.level.saturating_sub(1)) as u32 * 25,
                memory.max_cooldown
            ),
            13.0,
            WHITE,
        );
        add_label(
            commands,
            column,
            memory
                .essence
                .map(|e| format!("{} ESSENCE", e.name().to_uppercase()))
                .unwrap_or("EMPTY ESSENCE SOCKET".into()),
            11.0,
            color,
        );
        add_label(
            commands,
            column,
            memory
                .essence
                .map(|e| e.effect_for(memory.kind))
                .unwrap_or("Find an Essence after an encounter to transform this Memory."),
            13.0,
            MUTED,
        );
        add_button(
            commands,
            column,
            UiAction::SelectSlot(i),
            if i == selected {
                "SELECTED"
            } else {
                "SELECT SLOT"
            },
            i == selected,
        );
        if i != selected && (snap.phase != RunPhase::Combat || snap.paused) && !snap.ready {
            add_button(
                commands,
                column,
                UiAction::Swap(selected, i),
                format!("SWAP WITH {}", KEYS[selected]),
                false,
            );
        }
    }
    add_label(
        commands,
        card,
        if connection.party_size > 1 && snap.phase == RunPhase::Combat {
            "Your online party keeps moving. Rearrange Memories between encounters; one Essence attaches to each Memory."
        } else if snap.ready {
            "Your loadout is ready. Waiting for companions before crossing the veil."
        } else {
            "Select a slot, then choose Swap on another Memory to rearrange. One Essence per Memory; replacement keeps the Essence."
        },
        13.0,
        MUTED,
    );
    add_button(
        commands,
        card,
        UiAction::ToggleBuild,
        "RETURN TO THE DREAM     /     Tab",
        true,
    );
}

fn transition_panel(commands: &mut Commands, root: Entity, snap: &dreamwake_sim::DreamSnapshot) {
    let card = panel(commands, root, 530.0);
    add_label(commands, card, "A FRAGMENT RECLAIMED", 12.0, GOLD);
    add_label(commands, card, "The way opens.", 38.0, WHITE);
    add_label(
        commands,
        card,
        if snap.message.is_empty() {
            "Your new power settles into place. Another dream waits beyond the veil."
        } else {
            &snap.message
        },
        17.0,
        MUTED,
    );
    add_label(
        commands,
        card,
        format!(
            "{} dreams cleared  ·  {} foes defeated\n{} shards  ·  Level {}",
            snap.cleared, snap.kills, snap.hero.shards, snap.hero.level
        ),
        15.0,
        WHITE,
    );
    if snap.ready {
        add_label(
            commands,
            card,
            "READY  /  WAITING FOR COMPANIONS",
            17.0,
            TEAL,
        );
    } else {
        add_button(
            commands,
            card,
            UiAction::Continue,
            "READY TO CROSS     /     Enter",
            true,
        );
    }
    add_button(
        commands,
        card,
        UiAction::ToggleBuild,
        "REFORGE YOUR MEMORIES     /     Tab",
        false,
    );
}

fn end_panel(
    commands: &mut Commands,
    root: Entity,
    snap: &dreamwake_sim::DreamSnapshot,
    _connection: &DreamConnection,
) {
    let victory = snap.phase == RunPhase::Victory;
    let card = panel(commands, root, 580.0);
    add_label(
        commands,
        card,
        if victory {
            "THE SOMNARCH HAS FALLEN"
        } else {
            "NO DREAM IS EVER LOST"
        },
        12.0,
        GOLD,
    );
    add_label(
        commands,
        card,
        if victory {
            "Morning finds you."
        } else {
            "Until the next dream."
        },
        41.0,
        WHITE,
    );
    add_label(
        commands,
        card,
        if victory {
            "You gathered the broken pieces of a sleeping world and taught them how to wake."
        } else {
            "Vesper fades into the stars. The paths will shift, the Memories will change, and you will return."
        },
        18.0,
        MUTED,
    );
    add_label(
        commands,
        card,
        format!(
            "{} BATTLES WON     ·     {} FOES\nLEVEL {}     ·     {} SHARDS     ·     {}:{:02}",
            snap.cleared,
            snap.kills,
            snap.hero.level,
            snap.hero.shards,
            snap.elapsed as u32 / 60,
            snap.elapsed as u32 % 60
        ),
        16.0,
        TEAL,
    );
    let memories = snap
        .hero
        .memories
        .iter()
        .map(|s| {
            format!(
                "{} {}{}",
                s.kind.name(),
                s.level,
                s.essence
                    .map(|e| format!(" + {}", e.name()))
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    add_label(commands, card, memories, 15.0, WHITE);
    {
        add_button(
            commands,
            card,
            UiAction::Restart,
            "DREAM AGAIN     /     Enter",
            true,
        );
        if victory && !snap.lucid {
            add_button(
                commands,
                card,
                UiAction::Start { lucid: true },
                "ENTER A LUCID DREAM",
                false,
            );
        }
    }
    add_button(commands, card, UiAction::Disconnect, "LEAVE PARTY", false);
}

#[cfg(test)]
mod tests {
    use super::*;
    use dreamwake_sim::DreamSimulation;

    #[test]
    fn current_frame_ui_focus_routes_click_and_blocks_attack_before_prediction() {
        use crate::plugins::diagnostics::Playtest;
        use crate::plugins::input::{
            CapturedInput, DreamInputSystems,
            capture::{apply_ui_actions, capture_input},
            configure_input_schedule,
        };
        let mut snapshot = DreamSimulation::new(42, false).snapshot();
        snapshot.phase = RunPhase::Combat;
        let mut app = App::new();
        configure_input_schedule(&mut app);
        app.insert_resource(DreamView(snapshot))
            .init_resource::<DreamPreferences>()
            .init_resource::<UiActions>()
            .init_resource::<CapturedInput>()
            .init_resource::<Playtest>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_systems(
                PreUpdate,
                (
                    // Stand in for Bevy's focus calculation at its actual set boundary.
                    (|mut buttons: Query<&mut Interaction>| {
                        for mut interaction in &mut buttons {
                            *interaction = Interaction::Pressed;
                        }
                    })
                    .in_set(bevy::ui::UiSystems::Focus),
                    route_buttons.in_set(DreamInputSystems::Buttons),
                    apply_ui_actions.in_set(DreamInputSystems::Apply),
                    capture_input.in_set(DreamInputSystems::Capture),
                ),
            );
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.world_mut().spawn((
            Node::default(),
            Interaction::None,
            ActionButton(UiAction::ToggleMute),
            BackgroundColor::default(),
            BorderColor::default(),
        ));
        app.world_mut().run_schedule(PreUpdate);
        assert!(app.world().resource::<DreamPreferences>().muted);
        assert!(app.world().resource::<UiActions>().0.is_empty());
        assert!(!app.world().resource::<CapturedInput>().0.attack);
    }

    fn ui_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
        ))
        .insert_resource(DreamView(DreamSimulation::new(42, false).snapshot()))
        .insert_resource(DreamConnection {
            status: "Connected".into(),
            connected: true,
            client_id: 1,
            http_base: "http://127.0.0.1:8080".into(),
            share_url: String::new(),
            rtt_ms: 0.0,
            party_size: 1,
        })
        .insert_resource(UiScale(1.0))
        .init_resource::<DreamPreferences>()
        .add_plugins(DreamUiPlugin);
        app
    }

    #[test]
    fn screens_replace_cleanly_and_rewards_target_the_current_slot() {
        let mut app = ui_app();
        app.update();
        assert_eq!(
            app.world_mut()
                .query::<&OverlayRoot>()
                .iter(app.world())
                .count(),
            1
        );
        {
            let mut view = app.world_mut().resource_mut::<DreamView>();
            view.0.phase = RunPhase::Reward;
            view.0.rewards = vec![Reward {
                kind: RewardKind::Memory(MemoryKind::Nova),
                rarity: Rarity::Rare,
                title: "Nova".into(),
                description: "A dream nova".into(),
            }];
        }
        app.update();
        app.world_mut()
            .resource_mut::<DreamPreferences>()
            .selected_slot = 3;
        {
            let world = app.world_mut();
            for (action, mut interaction) in world
                .query::<(&ActionButton, &mut Interaction)>()
                .iter_mut(world)
            {
                if matches!(action.0, UiAction::Choose { .. }) {
                    *interaction = Interaction::Pressed;
                }
                assert!(!matches!(action.0, UiAction::Start { .. }));
            }
        }
        app.update();
        assert!(
            app.world()
                .resource::<UiActions>()
                .0
                .iter()
                .any(|action| matches!(action, UiAction::Choose { choice: 0, slot: 3 }))
        );
        app.world_mut().resource_mut::<DreamView>().0.phase = RunPhase::Combat;
        app.update();
        assert_eq!(
            app.world_mut()
                .query::<&OverlayRoot>()
                .iter(app.world())
                .count(),
            0
        );
    }

    #[test]
    fn any_player_can_start_and_consumed_rewards_stay_hidden() {
        let mut app = ui_app();
        {
            let mut connection = app.world_mut().resource_mut::<DreamConnection>();
            connection.client_id = 2;
            connection.party_size = 2;
        }
        app.update();
        {
            let world = app.world_mut();
            assert!(
                world
                    .query::<&ActionButton>()
                    .iter(world)
                    .any(|button| matches!(button.0, UiAction::Start { .. }))
            );
        }
        {
            let mut view = app.world_mut().resource_mut::<DreamView>();
            view.0.phase = RunPhase::Reward;
            view.0.awaiting_party = true;
            view.0.rewards.clear();
        }
        app.update();
        {
            let world = app.world_mut();
            assert!(
                !world
                    .query::<&ActionButton>()
                    .iter(world)
                    .any(|button| matches!(button.0, UiAction::Choose { .. }))
            );
            assert!(
                world
                    .query::<&Text>()
                    .iter(world)
                    .any(|text| text.0 == "WAITING FOR COMPANIONS")
            );
        }
        {
            let mut view = app.world_mut().resource_mut::<DreamView>();
            view.0.phase = RunPhase::Transition;
            view.0.ready = true;
            view.0.awaiting_party = false;
        }
        app.update();
        let world = app.world_mut();
        assert!(
            !world
                .query::<&ActionButton>()
                .iter(world)
                .any(|button| matches!(button.0, UiAction::Continue))
        );
    }

    #[test]
    fn upgrade_comparisons_show_caps_and_actual_result() {
        let mut snapshot = DreamSimulation::new(42, false).snapshot();
        snapshot.hero.critical_chance = 0.8;
        let reward = Reward {
            kind: RewardKind::Upgrade(UpgradeKind::Critical),
            rarity: Rarity::Rare,
            title: String::new(),
            description: String::new(),
        };
        assert!(reward_comparison(&reward, &snapshot, 0).contains("80% → 85%"));
    }
}
