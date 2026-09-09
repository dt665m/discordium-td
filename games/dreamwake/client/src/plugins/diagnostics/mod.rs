pub(crate) mod capture;
#[cfg(feature = "debug-tools")]
mod debug;
pub(crate) mod overlay;
mod playtest;
use bevy::prelude::*;
pub(crate) use playtest::Playtest;
pub struct DreamDiagnosticsPlugin;
impl Plugin for DreamDiagnosticsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(overlay::DreamDiagnosticsPlugin)
            .init_resource::<capture::Telemetry>()
            .add_systems(
                Update,
                (
                    playtest::playtest_capture,
                    capture::sample.before(engine_client::network_tools::graphs::update_graphs),
                ),
            );
        #[cfg(feature = "debug-tools")]
        app.add_plugins(debug::DreamDebugPlugin);
    }
}
