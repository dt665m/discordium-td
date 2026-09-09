pub mod timing;
use bevy::prelude::*;
pub use timing::*;
/// Supplies the game-selected replay delta; does not advance a clock.
pub struct SystemPlugin {
    pub dt: f32,
}
impl SystemPlugin {
    pub fn new(dt: f32) -> Self {
        assert!(dt.is_finite() && dt >= 0.0);
        Self { dt }
    }
}
impl Plugin for SystemPlugin {
    fn build(&self, app: &mut App) {
        assert!(self.dt.is_finite() && self.dt >= 0.0);
        app.insert_resource(SimulationStep(self.dt));
    }
}
