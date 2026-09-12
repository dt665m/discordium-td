//! Private authoritative enemy activation and authored population roles.
use super::*;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum EnemyRole {
    Encounter,
    Ambient,
}
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnemyAi {
    pub role: EnemyRole,
    pub home: [f32; 2],
    pub awake: bool,
    /// Stable Traveler identity retained until that actor is unavailable or defeated.
    pub target: Option<u64>,
    /// Initial encounter pacing is consumed at first activation, never while idle.
    pub wake_delay: f32,
}
impl EnemyAi {
    pub fn valid(&self) -> bool {
        self.awake == self.target.is_some()
            && self.target != Some(0)
            && self.home.iter().all(|v| v.is_finite())
            && self.home[0] * self.home[0] + self.home[1] * self.home[1]
                <= ARENA_RADIUS * ARENA_RADIUS
            && self.wake_delay.is_finite()
            && (0.0..=1.8).contains(&self.wake_delay)
            && (self.role != EnemyRole::Ambient || AMBIENT_ENEMY_POSITIONS.contains(&self.home))
    }
}
