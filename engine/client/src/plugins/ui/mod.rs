//! Passive screen UI primitives and world anchors, independent of game rules and art.
mod billboard;
pub mod debug;
mod widgets;

pub use billboard::{WorldUi, WorldUiAnchor};
pub use debug::{DebugUiPlugin, DebugUiRoot, DebugUiState};
pub use engine_core::DespawnWithOwner as WorldUiOwner;
pub use widgets::{MeterFill, MeterFraction, label, meter, meter_with_marker};

use bevy::{camera::CameraUpdateSystems, prelude::*, ui::UiSystems};

/// Public phases for game adapters. Update presentation data before these sets.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiSet {
    Billboards,
    Widgets,
}

/// Adds passive UI synchronization; debug panels are a separate opt-in plugin.
pub struct EngineUiPlugin;

impl Plugin for EngineUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiScale>()
            .configure_sets(
                PostUpdate,
                (UiSet::Billboards, UiSet::Widgets).before(UiSystems::Content),
            )
            .configure_sets(PostUpdate, UiSet::Billboards.after(CameraUpdateSystems))
            .add_systems(
                PostUpdate,
                billboard::project_world_ui.in_set(UiSet::Billboards),
            )
            .add_systems(PostUpdate, widgets::sync_meters.in_set(UiSet::Widgets));
    }
}

#[cfg(test)]
mod tests;
