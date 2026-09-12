//! Traveler selection uses Bevy's editable-text widget and the saved profile API.
use super::{BORDER, GOLD, INK, MUTED, PANEL, TEAL, WHITE, label};
use crate::{DreamConnection, identity::TravelerProfile};
use bevy::{
    input_focus::{
        InputFocus, InputFocusVisible,
        tab_navigation::{TabGroup, TabIndex, TabNavigationPlugin},
    },
    prelude::*,
    text::{EditableText, EditableTextFilter, TextCursorStyle},
    ui::{FocusPolicy, InteractionDisabled},
    ui_widgets::{Activate, Button as WidgetButton, SelectAllOnFocus},
};
use dreamwake_protocol::player::PlayerId;

#[derive(Resource, Default)]
pub(crate) struct TravelerIdentityUi {
    pub(crate) open: bool,
    feedback: Option<(String, bool)>,
    pending_save: Option<(PlayerId, bool)>,
}
#[derive(Resource, Default)]
struct RequestedAction(Option<PanelAction>);
#[derive(Component, Clone, Copy, Default)]
enum PanelAction {
    #[default]
    Open,
    Close,
    Disconnect,
    Save,
    SaveAndJoin,
}
#[derive(Component, Default, Clone)]
struct IdentityRoot;
#[derive(Component, Default, Clone)]
struct IdentityInput;
#[derive(Component, Default, Clone)]
struct IdentityFeedback;
#[derive(Component, Default, Clone)]
struct IdentityConnectionStatus;
#[derive(Component, Default, Clone)]
struct SaveButton;
#[derive(Component, Clone, Copy, Default)]
enum CurrentIdText {
    #[default]
    Launcher,
    Panel,
}

pub(super) struct TravelerIdentityPlugin;
impl Plugin for TravelerIdentityPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<TabNavigationPlugin>() {
            app.add_plugins(TabNavigationPlugin);
        }
        // DefaultPlugins owns input dispatch and editable-text behavior. These
        // resources also let the lightweight headless UI tests compose this UI.
        app.init_resource::<InputFocus>()
            .init_resource::<InputFocusVisible>()
            .init_resource::<TravelerIdentityUi>()
            .init_resource::<RequestedAction>()
            .add_observer(request_action)
            .add_systems(
                PreUpdate,
                (apply_request, finish_pending_save)
                    .chain()
                    .in_set(crate::plugins::input::DreamInputSystems::Buttons),
            )
            .add_systems(Update, (sync_panel, sync_current_id, sync_feedback).chain())
            .add_systems(
                PostUpdate,
                apply_save_request.after(bevy::text::EditableTextSystems),
            );
    }
}

fn identity_button(
    action: PanelAction,
    title: impl Into<String>,
    emphasis: bool,
    tab: i32,
) -> impl Scene {
    let background = if emphasis {
        Color::srgb(0.15, 0.31, 0.31)
    } else {
        INK
    };
    let border = if emphasis { TEAL } else { BORDER };
    bsn! {
        Button
        WidgetButton
        template_value(action)
        TabIndex(tab)
        Node {
            min_height: px(42), padding: UiRect::axes(px(16), px(10)),
            border: px(1), border_radius: BorderRadius::all(px(6)),
            justify_content: JustifyContent::Center, align_items: AlignItems::Center,
        }
        BackgroundColor(background)
        BorderColor::all(border)
        Children [label(title, 14.0, WHITE)]
    }
}

/// A compact discoverable entry point; its text updates without rebuilding the
/// surrounding lobby or pause card when the selected profile changes.
pub(super) fn add_launcher(commands: &mut Commands, parent: Entity) {
    commands.spawn_scene(bsn! {
        Button
        WidgetButton
        template_value(PanelAction::Open)
        TabIndex(0)
        Node {
            padding: UiRect::axes(px(12), px(9)), min_height: px(38),
            border: px(1), border_radius: BorderRadius::all(px(6)),
            justify_content: JustifyContent::Center, align_items: AlignItems::Center,
        }
        BackgroundColor(INK)
        BorderColor::all(BORDER)
        Children [(label("TRAVELER ID  /  Settings", 13.0, TEAL) template_value(CurrentIdText::Launcher))]
    }).insert((ChildOf(parent), TabGroup::new(0)));
}

fn request_action(
    event: On<Activate>,
    actions: Query<&PanelAction>,
    mut request: ResMut<RequestedAction>,
) {
    if let Ok(action) = actions.get(event.entity) {
        request.0 = Some(*action);
    }
}

fn apply_request(world: &mut World) {
    let Some(action) = world.resource_mut::<RequestedAction>().0.take() else {
        return;
    };
    match action {
        PanelAction::Open => {
            let warning = world
                .get_resource::<TravelerProfile>()
                .and_then(|profile| profile.storage_error().map(str::to_owned));
            let mut ui = world.resource_mut::<TravelerIdentityUi>();
            ui.open = true;
            ui.feedback = warning.map(|warning| (warning, false));
        }
        PanelAction::Close => {
            let mut ui = world.resource_mut::<TravelerIdentityUi>();
            ui.open = false;
            ui.pending_save = None;
            world.resource_mut::<InputFocus>().clear();
        }
        PanelAction::Disconnect => {
            crate::plugins::network::disconnect(world);
            world.resource_mut::<TravelerIdentityUi>().feedback = None;
        }
        PanelAction::Save | PanelAction::SaveAndJoin => {
            // Bevy applies text edits in PostUpdate. Saving before then can
            // omit the last keystroke or an asynchronous clipboard paste.
            world.resource_mut::<RequestedAction>().0 = Some(action);
        }
    }
}

fn apply_save_request(world: &mut World) {
    let Some(action @ (PanelAction::Save | PanelAction::SaveAndJoin)) =
        world.resource::<RequestedAction>().0
    else {
        return;
    };
    let input = world
        .query_filtered::<&EditableText, With<IdentityInput>>()
        .single(world)
        .ok()
        .map(|input| {
            (
                input.value().to_string(),
                input.pending_edits.is_empty() && input.pending_paste.is_none(),
            )
        });
    let Some((text, ready)) = input else {
        world.resource_mut::<RequestedAction>().0 = None;
        return;
    };
    if !ready {
        return;
    }
    world.resource_mut::<RequestedAction>().0 = None;
    // Shared parser and profile selection both validate. The second check
    // also enforces the live/pending connection lock at activation time.
    let result = text
        .parse::<PlayerId>()
        .map_err(|error| error.to_string())
        .and_then(|_| crate::identity::select(world, &text));
    match result {
        Ok(crate::identity::SelectionStatus::Ready) => {
            finish_selection(world, matches!(action, PanelAction::SaveAndJoin))
        }
        Ok(crate::identity::SelectionStatus::Pending) => {
            let mut ui = world.resource_mut::<TravelerIdentityUi>();
            ui.pending_save = Some((
                text.parse().expect("validated Traveler ID"),
                matches!(action, PanelAction::SaveAndJoin),
            ));
            ui.feedback = Some(("Saving Traveler ID…".into(), true));
        }
        Err(error) => world.resource_mut::<TravelerIdentityUi>().feedback = Some((error, false)),
    }
}

fn finish_selection(world: &mut World, join: bool) {
    let mut ui = world.resource_mut::<TravelerIdentityUi>();
    ui.feedback = Some(("Traveler ID saved on this device.".into(), true));
    if join && ui.open {
        ui.open = false;
        world.resource_mut::<InputFocus>().clear();
        world
            .resource_mut::<super::UiActions>()
            .0
            .push(super::UiAction::Connect);
    }
}

fn finish_pending_save(world: &mut World) {
    let Some((selected, join)) = world.resource::<TravelerIdentityUi>().pending_save else {
        return;
    };
    let profile = world.resource::<TravelerProfile>();
    if profile.is_pending() {
        return;
    }
    let error = profile.storage_error().map(str::to_owned);
    let ready = profile.is_ready() && profile.selected_id() == selected;
    world.resource_mut::<TravelerIdentityUi>().pending_save = None;
    if let Some(error) = error {
        world.resource_mut::<TravelerIdentityUi>().feedback = Some((error, false));
    } else if ready {
        let unchanged = world
            .query_filtered::<&EditableText, With<IdentityInput>>()
            .single(world)
            .ok()
            .is_some_and(|input| input.value().to_string() == selected.to_string());
        if unchanged {
            finish_selection(world, join);
        } else {
            world.resource_mut::<TravelerIdentityUi>().feedback = Some((
                format!("ID {selected} was saved. Save your edited ID before joining."),
                true,
            ));
        }
    } else {
        world.resource_mut::<TravelerIdentityUi>().feedback = Some((
            "Traveler ID save did not complete. Try again.".into(),
            false,
        ));
    }
}

fn sync_panel(
    mut commands: Commands,
    ui: Res<TravelerIdentityUi>,
    profile: Option<Res<TravelerProfile>>,
    roots: Query<Entity, With<IdentityRoot>>,
    mut focus: ResMut<InputFocus>,
    mut previous: Local<Option<(bool, bool)>>,
) {
    let locked = profile.as_ref().is_none_or(|profile| profile.is_locked());
    let initializing = profile
        .as_ref()
        .is_some_and(|profile| profile.is_initializing());
    let desired = ui.open.then_some((locked, initializing));
    if *previous == desired {
        return;
    }
    *previous = desired;
    focus.clear();
    for entity in &roots {
        commands.entity(entity).despawn();
    }
    if !ui.open {
        return;
    }
    let selected = profile
        .as_ref()
        .map(|profile| profile.selected_id().to_string())
        .unwrap_or_default();
    let root = commands
        .spawn_scene(bsn! {
            IdentityRoot
            TabGroup::modal()
            Node {
                position_type: PositionType::Absolute, left: px(0), top: px(0),
                width: percent(100), height: percent(100),
                justify_content: JustifyContent::Center, align_items: AlignItems::Center,
                padding: px(20),
            }
            BackgroundColor(Color::srgba(0.012, 0.022, 0.046, 0.94))
            GlobalZIndex(50)
            template_value(FocusPolicy::Block)
        })
        .id();
    let card = commands.spawn_scene(bsn! {
        Node {
            width: px(570), max_width: percent(100), padding: px(26), row_gap: px(16),
            flex_direction: FlexDirection::Column,
            border: px(1), border_radius: BorderRadius::all(px(9)),
        }
        BackgroundColor(PANEL)
        BorderColor::all(BORDER)
        Children [
            label("YOUR TRAVELER", 12.0, GOLD),
            label("Traveler ID", 32.0, WHITE),
            (label("", 19.0, TEAL) template_value(CurrentIdText::Panel)),
            label("Keep the same ID to return as the same Traveler on this device.", 15.0, MUTED),
            (label("", 13.0, GOLD) template_value(IdentityConnectionStatus)),
        ]
    }).insert(ChildOf(root)).id();
    if initializing {
        super::add_label(
            &mut commands,
            card,
            "Preparing your saved Traveler ID…",
            16.0,
            TEAL,
        );
    } else if locked {
        super::add_label(
            &mut commands,
            card,
            "Leave the party or cancel the connection before changing your ID.",
            15.0,
            MUTED,
        );
        let disconnect = commands
            .spawn_scene(identity_button(
                PanelAction::Disconnect,
                "DISCONNECT TO CHANGE ID",
                true,
                0,
            ))
            .insert(ChildOf(card))
            .id();
        focus.set(disconnect, bevy::input_focus::FocusCause::Navigated);
    } else {
        super::add_label(
            &mut commands,
            card,
            "ID  /  Whole number, no leading zeros",
            12.0,
            MUTED,
        );
        let editable = EditableText {
            max_characters: Some(19),
            visible_width: Some(21.0),
            allow_newlines: false,
            ..EditableText::new(selected)
        };
        let input = commands.spawn_scene(bsn! {
            IdentityInput
            template_value(editable)
            template_value(EditableTextFilter::new(|character| character.is_ascii_digit()))
            SelectAllOnFocus
            TextCursorStyle
            TextLayout::no_wrap()
            TextFont { font_size: FontSize::Px(24.0) }
            TextColor(WHITE)
            TabIndex(0)
            Node {
                width: percent(100), min_height: px(52), padding: px(12),
                border: px(1), border_radius: BorderRadius::all(px(5)), overflow: Overflow::clip_x(),
            }
            BackgroundColor(INK)
            BorderColor::all(TEAL)
        }).insert(ChildOf(card)).id();
        focus.set(input, bevy::input_focus::FocusCause::Navigated);
        super::add_label(
            &mut commands,
            card,
            "Allowed IDs: 1 to 9223372036854775807",
            12.0,
            MUTED,
        );
        let row = super::row(&mut commands, card, 10.0);
        commands
            .spawn_scene(identity_button(PanelAction::Save, "SAVE ID", false, 1))
            .insert((ChildOf(row), SaveButton));
        commands
            .spawn_scene(identity_button(
                PanelAction::SaveAndJoin,
                "SAVE & JOIN",
                true,
                2,
            ))
            .insert((ChildOf(row), SaveButton));
    }
    commands
        .spawn_scene(bsn! {
            IdentityFeedback
            label("", 14.0, GOLD)
            Node { min_height: px(40) }
        })
        .insert(ChildOf(card));
    commands
        .spawn_scene(identity_button(PanelAction::Close, "BACK", false, 3))
        .insert(ChildOf(card));
}

fn sync_current_id(
    profile: Option<Res<TravelerProfile>>,
    connection: Res<DreamConnection>,
    mut labels: Query<(&CurrentIdText, &mut Text)>,
    mut status: Query<&mut Text, (With<IdentityConnectionStatus>, Without<CurrentIdText>)>,
) {
    let selected = profile
        .as_ref()
        .map(|profile| profile.selected_id().to_string())
        .unwrap_or_else(|| "Unavailable".into());
    for (kind, mut text) in &mut labels {
        let value = match kind {
            CurrentIdText::Launcher => format!("TRAVELER ID  {selected}   /   Settings"),
            CurrentIdText::Panel => format!("Current ID: {selected}"),
        };
        if text.0 != value {
            text.0 = value;
        }
    }
    for mut text in &mut status {
        if text.0 != connection.status {
            text.0.clone_from(&connection.status);
        }
    }
}

fn sync_feedback(
    mut commands: Commands,
    profile: Option<Res<TravelerProfile>>,
    mut ui: ResMut<TravelerIdentityUi>,
    inputs: Query<&EditableText, With<IdentityInput>>,
    mut feedback: Query<(&mut Text, &mut TextColor), With<IdentityFeedback>>,
    mut buttons: Query<
        (
            Entity,
            &mut BackgroundColor,
            &mut BorderColor,
            Has<InteractionDisabled>,
        ),
        With<SaveButton>,
    >,
    mut previous: Local<Option<String>>,
) {
    if !ui.open {
        *previous = None;
        return;
    }
    let text = inputs.single().ok().map(|input| input.value().to_string());
    if let Some(text) = &text {
        if previous.as_ref().is_some_and(|old| old != text) {
            ui.feedback = None;
        }
        *previous = Some(text.clone());
    } else {
        *previous = None;
    }
    let validation = text.as_deref().map(str::parse::<PlayerId>);
    let pending = profile.as_ref().is_some_and(|profile| profile.is_pending());
    let enabled = profile
        .as_ref()
        .is_some_and(|profile| !profile.is_locked() && !profile.is_pending())
        && matches!(validation, Some(Ok(_)));
    for (entity, mut background, mut border, disabled) in &mut buttons {
        if enabled && disabled {
            commands.entity(entity).remove::<InteractionDisabled>();
        } else if !enabled && !disabled {
            commands.entity(entity).insert(InteractionDisabled);
        }
        let color = if enabled { TEAL } else { MUTED };
        if border.top != color {
            *border = BorderColor::all(color);
        }
        let color = if enabled {
            Color::srgb(0.15, 0.31, 0.31)
        } else {
            INK
        };
        if background.0 != color {
            background.0 = color;
        }
    }
    let (message, success) = if pending {
        ("Saving Traveler ID…".into(), true)
    } else if let Some(Err(error)) = validation {
        (error.to_string(), false)
    } else if let Some(feedback) = &ui.feedback {
        feedback.clone()
    } else if let Some(error) = profile.as_ref().and_then(|profile| profile.storage_error()) {
        (error.into(), false)
    } else {
        (String::new(), true)
    };
    for (mut text, mut color) in &mut feedback {
        if text.0 != message {
            text.0.clone_from(&message);
        }
        let desired = if success {
            TEAL
        } else {
            Color::srgb(1.0, 0.65, 0.55)
        };
        if color.0 != desired {
            color.0 = desired;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
            bevy::text::TextPlugin,
        ))
        .init_resource::<TravelerProfile>()
        .init_resource::<DreamConnection>()
        .init_resource::<super::super::UiActions>()
        .add_plugins(TravelerIdentityPlugin);
        crate::identity::select(app.world_mut(), "101").unwrap();
        app
    }
    fn activate(app: &mut App, action: PanelAction) {
        let entity = app.world_mut().spawn(action).id();
        app.world_mut().trigger(Activate { entity });
        app.update();
        app.world_mut().despawn(entity);
    }
    fn input_entity(app: &mut App) -> Entity {
        app.world_mut()
            .query_filtered::<Entity, With<IdentityInput>>()
            .single(app.world())
            .unwrap()
    }
    fn set_input(app: &mut App, value: &str) {
        let entity = input_entity(app);
        app.world_mut()
            .get_mut::<EditableText>(entity)
            .unwrap()
            .editor_mut()
            .set_text(value);
        app.update();
    }

    #[test]
    fn native_widget_draft_survives_connection_status_changes() {
        let mut app = app();
        activate(&mut app, PanelAction::Open);
        let entity = input_entity(&mut app);
        assert_eq!(
            app.world()
                .get::<EditableText>(entity)
                .unwrap()
                .value()
                .to_string(),
            "101"
        );
        assert!(app.world().get::<EditableTextFilter>(entity).is_some());
        assert!(app.world().get::<SelectAllOnFocus>(entity).is_some());
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(entity));
        set_input(&mut app, "9223372036854775807");
        app.world_mut().resource_mut::<DreamConnection>().status =
            "This Traveler is already connected.".into();
        app.update();
        assert_eq!(input_entity(&mut app), entity);
        assert_eq!(
            app.world()
                .get::<EditableText>(entity)
                .unwrap()
                .value()
                .to_string(),
            "9223372036854775807"
        );
        assert!(
            app.world_mut()
                .query_filtered::<&Text, With<IdentityConnectionStatus>>()
                .iter(app.world())
                .any(|text| text.0.contains("already connected"))
        );
    }

    #[test]
    fn validation_disables_save_and_save_join_uses_the_selected_id() {
        let mut app = app();
        activate(&mut app, PanelAction::Open);
        for invalid in ["", "0", "001", "9223372036854775808"] {
            set_input(&mut app, invalid);
            assert_eq!(
                app.world_mut()
                    .query_filtered::<Entity, (With<SaveButton>, With<InteractionDisabled>)>()
                    .iter(app.world())
                    .count(),
                2
            );
            activate(&mut app, PanelAction::Save);
            assert_eq!(
                app.world()
                    .resource::<TravelerProfile>()
                    .selected_id()
                    .get(),
                101
            );
            assert!(
                app.world_mut()
                    .query_filtered::<&Text, With<IdentityFeedback>>()
                    .iter(app.world())
                    .any(|text| text.0.contains("whole-number"))
            );
        }
        set_input(&mut app, "9223372036854775807");
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, (With<SaveButton>, With<InteractionDisabled>)>()
                .iter(app.world())
                .count(),
            0
        );
        activate(&mut app, PanelAction::SaveAndJoin);
        assert_eq!(
            app.world()
                .resource::<TravelerProfile>()
                .selected_id()
                .get(),
            PlayerId::MAX
        );
        assert!(!app.world().resource::<TravelerIdentityUi>().open);
        assert!(
            app.world()
                .resource::<super::super::UiActions>()
                .0
                .iter()
                .any(|action| matches!(action, super::super::UiAction::Connect))
        );
        assert!(app.world().resource::<InputFocus>().get().is_none());
    }

    #[test]
    fn locked_profile_is_visible_but_has_no_editable_field() {
        let mut app = app();
        app.world_mut()
            .resource_mut::<TravelerProfile>()
            .lock()
            .unwrap();
        activate(&mut app, PanelAction::Open);
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<IdentityInput>>()
                .iter(app.world())
                .count(),
            0
        );
        assert!(
            app.world_mut()
                .query::<&PanelAction>()
                .iter(app.world())
                .any(|action| matches!(action, PanelAction::Disconnect))
        );
        assert!(
            app.world_mut()
                .query::<(&CurrentIdText, &Text)>()
                .iter(app.world())
                .any(|(_, text)| text.0.contains("101"))
        );
        app.world_mut().resource_mut::<TravelerProfile>().unlock();
        app.update();
        let entity = input_entity(&mut app);
        assert_eq!(
            app.world()
                .get::<EditableText>(entity)
                .unwrap()
                .value()
                .to_string(),
            "101"
        );
        activate(&mut app, PanelAction::Close);
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<IdentityRoot>>()
                .iter(app.world())
                .count(),
            0
        );
    }
    #[test]
    fn save_waits_for_bevy_to_apply_the_current_frame_last_digit() {
        let mut app = app();
        activate(&mut app, PanelAction::Open);
        set_input(&mut app, "20");
        let entity = input_entity(&mut app);
        {
            let mut editable = app.world_mut().get_mut::<EditableText>(entity).unwrap();
            editable.queue_edit(bevy::text::TextEdit::TextEnd(false));
            editable.queue_edit(bevy::text::TextEdit::Insert("2".into()));
        }
        activate(&mut app, PanelAction::SaveAndJoin);
        assert_eq!(
            app.world()
                .resource::<TravelerProfile>()
                .selected_id()
                .get(),
            202
        );
        assert!(!app.world().resource::<TravelerIdentityUi>().open);
    }
}
