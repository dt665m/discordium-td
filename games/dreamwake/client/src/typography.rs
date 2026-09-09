//! Embedded, redistributable Dreamwake typography, shared by native and web builds.
use bevy::{prelude::*, text::Text2dUpdateSystems, ui::UiSystems};

const REGULAR_FONT: &[u8] = include_bytes!("../assets/dreamwake/fonts/FiraSans-Regular.ttf");
const DISPLAY_FONT: &[u8] = include_bytes!("../assets/dreamwake/fonts/FiraSans-Bold.ttf");

/// Apply after DefaultPlugins so the text asset collection is available.
/// Embedded bytes avoid a loading flash and work without an external asset server.
pub struct DreamTypographyPlugin;
impl Plugin for DreamTypographyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Assets<Font>>()
            .init_resource::<DreamFonts>()
            .add_systems(
                PostUpdate,
                (assign_embedded_fonts, replace_unsupported_symbols)
                    .chain()
                    .before(bevy::text::load_font_assets_into_font_collection)
                    .before(bevy::text::detect_text_needs_rerender)
                    .before(UiSystems::Content)
                    .before(Text2dUpdateSystems),
            );
    }
}

#[derive(Resource)]
struct DreamFonts {
    regular: Handle<Font>,
    display: Handle<Font>,
}
impl FromWorld for DreamFonts {
    fn from_world(world: &mut World) -> Self {
        let mut fonts = world.resource_mut::<Assets<Font>>();
        Self {
            regular: fonts.add(Font::from_bytes(REGULAR_FONT.to_vec())),
            display: fonts.add(Font::from_bytes(DISPLAY_FONT.to_vec())),
        }
    }
}

fn assign_embedded_fonts(
    fonts: Res<DreamFonts>,
    mut labels: Query<&mut TextFont, Added<TextFont>>,
) {
    for mut label in &mut labels {
        // Preserve any scene that deliberately chooses its own font.
        if label.font != FontSource::default() {
            continue;
        }
        let display = matches!(label.font_size, FontSize::Px(size) if size >= 18.0);
        label.font = FontSource::Handle(if display {
            fonts.display.clone()
        } else {
            fonts.regular.clone()
        });
    }
}

fn replace_unsupported_symbols(mut labels: Query<&mut Text, Changed<Text>>) {
    for mut label in &mut labels {
        // Fira Sans includes arrows, middle dots, bullets, multiplication and minus
        // signs. It does not include U+2726; keep the critical-hit star as ASCII.
        if label.0.contains('✦') {
            label.0 = label.0.replace('✦', "*");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_labels_receive_embedded_faces_and_explicit_fonts_are_preserved() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, DreamTypographyPlugin));
        let body = app
            .world_mut()
            .spawn((
                Text::new("Crescent · ready → cast"),
                TextFont {
                    font_size: FontSize::Px(15.0),
                    ..default()
                },
            ))
            .id();
        let title = app
            .world_mut()
            .spawn((
                Text::new("DREAMWAKE"),
                TextFont {
                    font_size: FontSize::Px(61.0),
                    ..default()
                },
            ))
            .id();
        let custom = app
            .world_mut()
            .spawn((
                Text::new("Custom face"),
                TextFont {
                    font: FontSource::Family("Custom".into()),
                    ..default()
                },
            ))
            .id();
        let critical = app.world_mut().spawn(Text::new("✦ 123")).id();
        app.update();
        let fonts = app.world().resource::<DreamFonts>();
        assert_eq!(
            app.world().get::<TextFont>(body).unwrap().font,
            FontSource::Handle(fonts.regular.clone())
        );
        assert_eq!(
            app.world().get::<TextFont>(title).unwrap().font,
            FontSource::Handle(fonts.display.clone())
        );
        assert_eq!(
            app.world().get::<TextFont>(custom).unwrap().font,
            FontSource::Family("Custom".into())
        );
        assert_eq!(app.world().get::<Text>(critical).unwrap().0, "* 123");
        assert_eq!(
            app.world().get::<Text>(body).unwrap().0,
            "Crescent · ready → cast"
        );
        let assets = app.world().resource::<Assets<Font>>();
        assert_eq!(
            assets.get(&fonts.regular).unwrap().data.as_ref(),
            REGULAR_FONT
        );
        assert_eq!(
            assets.get(&fonts.display).unwrap().data.as_ref(),
            DISPLAY_FONT
        );
    }
}
