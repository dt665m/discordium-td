use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Serializable simulation lifetime in seconds. Non-finite lifetimes expire.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Lifetime(pub f32);
