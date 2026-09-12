//! Dreamwake's deterministic, server-authoritative cooperative roguelite world.
//! Clients submit authenticated player inputs and render per-player snapshots.
mod source_identity {
    include!(concat!(env!("OUT_DIR"), "/source_identity.rs"));
}
mod catalog;
mod checkpoint;
pub mod collision;
pub mod combat;
mod combat_history;
pub mod combat_trace;
pub mod platform;
pub mod starfall;
pub use checkpoint::{CHECKPOINT_SCHEMA, CheckpointError, CheckpointIdentity, DreamCheckpoint};
pub use engine_core::ChargeCommand;
mod encounters;
mod plugin;
pub use plugin::{DreamInputs, DreamStep, DreamSystems, DreamwakePlugin};
pub mod replication;
pub use replication::DreamPresentation;
mod simulation;
pub use simulation::DreamSimulation;
mod snapshot_access;
mod snapshot_capture;
mod state;
mod systems;
#[cfg(test)]
mod tests;
mod types;
use bevy::prelude::*;
use engine_core::Health;
use state::*;
pub use types::*;
