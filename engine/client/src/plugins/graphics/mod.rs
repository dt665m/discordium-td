//! Drawable frame contract, graphics scheduling and reusable renderers.
mod frame;
mod prototype;
use bevy::prelude::*;
pub use frame::{GraphicsFrame, Primitive, Visual, VisualId};
pub use prototype::{PrototypeRendererPlugin, PrototypeSettings, draw_gizmos};

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GraphicsSet {
    Adapt,
    Render,
}
pub struct GraphicsPlugin;
impl Plugin for GraphicsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GraphicsFrame>()
            .configure_sets(Update, (GraphicsSet::Adapt, GraphicsSet::Render).chain());
    }
}
