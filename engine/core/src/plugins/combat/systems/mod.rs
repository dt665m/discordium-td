mod effects;
mod health;
mod regeneration;
pub use effects::tick_combat;
pub use health::update_health;
pub use regeneration::regenerate_meters;
