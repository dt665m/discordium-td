//! Dreamwake's deterministic, server-authoritative cooperative roguelite world.
//! Clients submit authenticated player inputs and render per-player snapshots.
mod catalog;
mod encounters;
mod plugin;
pub use plugin::{DreamInputs, DreamStep, DreamSystems, DreamwakePlugin};
mod simulation;
pub use simulation::DreamSimulation;
mod snapshot_access;
mod state;
mod systems;
#[cfg(test)]
mod tests;
mod types;
use bevy::prelude::*;
use engine_core::Health;
use state::*;
pub use types::*;
