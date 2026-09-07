mod debug_panel;
mod debug_recorder;
pub mod game;
pub mod lock_on;

pub mod prelude {
    pub use super::game::{ClientArgs, GameClientPlugin};
}
