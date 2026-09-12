pub(crate) mod capture;
#[cfg(feature = "debug-tools")]
mod debug;
#[cfg(not(target_arch = "wasm32"))]
mod native_capture;
#[cfg(not(target_arch = "wasm32"))]
mod native_window;
pub(crate) mod overlay;
mod playtest;
mod spatial;
use bevy::prelude::*;
pub(crate) use playtest::Playtest;
pub(crate) use spatial::SpatialPlaytest;
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
        #[cfg(not(target_arch = "wasm32"))]
        app.init_resource::<native_capture::NativeCapture>()
            .init_resource::<native_window::NativeWindowMetrics>()
            .add_systems(
                PreUpdate,
                (native_window::place_window, native_window::sample_window).chain(),
            );
    }
}
