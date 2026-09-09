use super::GraphicsId;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum GraphicsKind {
    RadialPulse,
}
#[derive(Component, Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct GraphicsInstance {
    pub id: GraphicsId,
    pub kind: GraphicsKind,
    pub pos: [f32; 2],
    pub radius: f32,
    pub age_ticks: u32,
    pub duration_ticks: u32,
}
