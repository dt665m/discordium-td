//! Repeatable ordinary player input for the two-process spatial qualification scene.
//! This changes captured intent only; the authoritative server admits every command.
use bevy::prelude::*;
use dreamwake_sim::{DreamInput, DreamPresentation, TICK_HZ};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum SpatialPlaytest {
    #[default]
    Off,
    Left,
    Right,
}
impl SpatialPlaytest {
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Left,
            Self::Left => Self::Right,
            Self::Right => Self::Off,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Left => "Left traveler",
            Self::Right => "Right traveler",
        }
    }
    pub fn separated(self, gameplay_tick: u32) -> bool {
        self != Self::Off && gameplay_tick / (TICK_HZ * 12) % 2 == 0
    }
    pub fn input(self, view: &DreamPresentation, mut combat: DreamInput) -> DreamInput {
        let side = match self {
            Self::Off => return combat,
            Self::Left => -1.0,
            Self::Right => 1.0,
        };
        // Admission can take several seconds while the match is still in Intro.
        // Both travelers follow the shared gameplay clock once play begins.
        let target = if self.separated(view.tick) {
            Vec2::new(side * dreamwake_sim::ARENA_RADIUS * 0.625, 0.0)
        } else {
            Vec2::new(side * 1.0, 2.0)
        };
        let offset = target - Vec2::from_array(view.hero.position);
        let position = Vec2::from_array(view.hero.position);
        let mut evade = Vec2::ZERO;
        for enemy in &view.enemies {
            let away = position - Vec2::from_array(enemy.target);
            if enemy.windup > 0.0 && away.length() < enemy.warn_radius + 1.6 {
                evade += away.try_normalize().unwrap_or(Vec2::Y);
            }
        }
        for shot in view.projectiles.iter().filter(|shot| !shot.friendly) {
            let away = position - Vec2::from_array(shot.position);
            if away.length() < 2.0 {
                evade += away.try_normalize().unwrap_or(Vec2::Y);
            }
        }
        let avoiding = evade.length_squared() > 0.0;
        combat.movement = if avoiding {
            evade.normalize().to_array()
        } else if offset.length() > 0.3 {
            offset.normalize().to_array()
        } else {
            [0.0; 2]
        };
        // Attacks intentionally lock movement. Prioritize reaching the fixture
        // waypoint, then fight with the same ordinary autopilot as the default test.
        if avoiding || offset.length() > 1.0 {
            combat.attack = false;
            combat.casts = [false; 4];
        }
        combat
    }
}
