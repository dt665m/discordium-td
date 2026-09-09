use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Progression {
    pub level: u32,
    pub xp: f32,
    pub xp_next: f32,
    pub pending_levels: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProgressionError {
    InvalidExperience,
    InvalidThreshold,
    LevelOverflow,
}

/// Actor-local pending grant. Invalid grants are consumed and their error retained.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PendingExperience(pub f32);
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ExperienceResult(pub Result<u32, ProgressionError>);
