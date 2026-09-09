pub(crate) mod capture;
use bevy::{input::InputSystems, prelude::*};
use dreamwake_sim::DreamInput;
#[derive(Resource, Default)]
pub(crate) struct CapturedInput(pub(crate) DreamInput);

/// Input stages all run in PreUpdate, before the fixed prediction loop.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DreamInputSystems {
    Receive,
    Keyboard,
    Buttons,
    Apply,
    Capture,
}

pub(crate) fn configure_input_schedule(app: &mut App) {
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
pub struct DreamInputPlugin;
impl Plugin for DreamInputPlugin {
    fn build(&self, app: &mut App) {
        configure_input_schedule(app);
        app.init_resource::<CapturedInput>().add_systems(
            PreUpdate,
            (
                capture::keyboard_actions.in_set(DreamInputSystems::Keyboard),
                capture::apply_ui_actions.in_set(DreamInputSystems::Apply),
                capture::capture_input.in_set(DreamInputSystems::Capture),
            ),
        );
    }
}
