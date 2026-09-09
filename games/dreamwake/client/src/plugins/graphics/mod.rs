mod adapter;
pub mod scene;
use bevy::prelude::*;
use engine_client::graphics::GraphicsSet;
pub struct DreamGraphicsPlugin;
impl Plugin for DreamGraphicsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, adapter::adapt.in_set(GraphicsSet::Adapt));
    }
}
