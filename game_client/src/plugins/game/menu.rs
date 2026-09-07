//! Connection menu composed with Bevy Scene Notation (BSN).
use super::*;

pub(super) fn main_menu() -> impl Scene {
    bsn! {
        MainMenuRoot
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
        }
        Children [(
            Node {
                width: px(520),
                padding: px(18),
                border_radius: BorderRadius::all(px(10)),
                row_gap: px(10),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Stretch,
            }
            BackgroundColor(MENU_PANEL_COLOR)
            Children [
                (
                    Text("Discordium TD")
                    TextFont { font_size: px(38) }
                    TextColor(Color::srgb(0.93, 0.96, 0.98))
                ),
                (
                    Text("Choose a connection mode")
                    TextFont { font_size: px(17) }
                    TextColor(Color::srgb(0.7, 0.78, 0.84))
                ),
                { single_player_button() },
                menu_button(MenuAction::ConnectDev, "Connect to Dev (localhost)"),
                (
                    MainMenuStatusText
                    Text("Select a mode to start.")
                    TextFont { font_size: px(15) }
                    TextColor(Color::srgb(0.75, 0.83, 0.89))
                ),
            ]
        )]
    }
}

fn single_player_button() -> impl SceneList {
    #[cfg(not(target_arch = "wasm32"))]
    {
        bsn_list![(
            menu_button(MenuAction::SinglePlayer, "Single Player (Host + Join)")
            Node { margin: UiRect::top(px(8)) }
        )]
    }
    #[cfg(target_arch = "wasm32")]
    {
        bsn_list![]
    }
}

fn menu_button(action: MenuAction, label: &str) -> impl Scene {
    bsn! {
        Button
        MainMenuButton(action)
        Node {
            width: percent(100),
            height: px(54),
            border: px(1),
            border_radius: BorderRadius::all(px(8)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
        }
        BorderColor::all(Color::srgb(0.36, 0.44, 0.49))
        BackgroundColor(MENU_BUTTON_NORMAL)
        Children [(
            Text(label)
            TextFont { font_size: px(21) }
            TextColor(Color::srgb(0.95, 0.97, 0.98))
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_scene_spawns_and_routes_connection_action() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
        ))
        .init_resource::<MainMenuState>()
        .add_systems(Startup, main_menu.spawn())
        .add_systems(Update, handle_menu_buttons);
        app.update();

        let world = app.world_mut();
        assert_eq!(world.query::<&MainMenuRoot>().iter(world).count(), 1);
        assert_eq!(world.query::<&MainMenuStatusText>().iter(world).count(), 1);
        let mut buttons = world.query::<(&MainMenuButton, &mut Interaction, &Children)>();
        assert_eq!(
            buttons.iter(world).count(),
            if cfg!(target_arch = "wasm32") { 1 } else { 2 }
        );
        for (button, mut interaction, children) in buttons.iter_mut(world) {
            assert_eq!(children.len(), 1);
            if matches!(button.0, MenuAction::ConnectDev) {
                *interaction = Interaction::Pressed;
            }
        }

        app.update();
        assert!(matches!(
            app.world().resource::<MainMenuState>().pending_action,
            Some(MenuAction::ConnectDev)
        ));
    }
}
