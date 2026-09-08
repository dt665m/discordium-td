//! Simulation-owned roles and reusable actor mechanics.
use super::*;
#[derive(Component, Clone, Copy, Debug)]
pub struct Position {
    pub pos: [f32; 2],
}
#[derive(Component, Clone, Copy, Debug)]
pub struct Health {
    pub hp: f32,
    pub max_hp: f32,
}
#[derive(Component, Clone, Copy, Debug)]
pub struct Facing {
    pub direction: FacingComponent,
}
#[derive(Component, Clone, Copy, Debug)]
pub struct Attack {
    pub state: DirectionalAttackStateComponent,
    pub profile: DirectionalAttackComponent,
}
#[derive(Component, Clone, Debug)]
pub struct HeroState {
    pub(crate) respawn_generation: u32,
    pub(crate) move_dir: [f32; 2],
    pub(crate) lock_mode_active: bool,
    pub(crate) lock_target_id: Option<u64>,
    pub(crate) charge_profile: ChargeProfileComponent,
    pub(crate) power_decay_profile: PowerDecayProfileComponent,
    pub(crate) powered_modifiers: PoweredUpModifiersComponent,
    pub(crate) charge_state: ChargeStateComponent,
    pub(crate) mana: f32,
    pub(crate) gold: u32,
    pub(crate) ability_cooldown_ticks: u32,
    pub(crate) last_processed_seq: Option<u32>,
    pub(crate) last_action_seq: Option<u32>,
    pub(crate) last_attack_seq: Option<u32>,
    pub(crate) pending_actions: VecDeque<game_shared::ClientAction>,
    pub(crate) pending_moves: VecDeque<(u32, [f32; 2])>,
}
#[derive(Bundle, Clone, Debug)]
pub(crate) struct HeroBundle {
    pub role: HeroState,
    pub position: Position,
    pub health: Health,
    pub facing: Facing,
    pub attack: Attack,
}
pub(crate) struct HeroRef<'w> {
    pub role: &'w HeroState,
    pub position: &'w Position,
    #[cfg(test)]
    pub health: &'w Health,
    pub facing: &'w Facing,
    pub attack: &'w Attack,
}
pub(crate) struct HeroMut<'w> {
    pub role: &'w mut HeroState,
    pub position: &'w mut Position,
    pub health: &'w mut Health,
    pub facing: &'w mut Facing,
    pub attack: &'w mut Attack,
}
#[cfg(test)]
impl HeroRef<'_> {
    pub fn snapshot(&self) -> HeroBundle {
        HeroBundle {
            role: self.role.clone(),
            position: self.position.clone(),
            health: self.health.clone(),
            facing: self.facing.clone(),
            attack: self.attack.clone(),
        }
    }
}
#[derive(Component, Clone, Debug)]
pub struct EnemyState {
    pub(crate) spawn: game_shared::EnemySpawnIdentity,
    pub(crate) id: u64,
    pub(crate) lane: u8,
    pub(crate) vel: [f32; 2],
    pub(crate) speed: f32,
    pub(crate) reward: u32,
    pub(crate) enemy_type: EnemyType,
    pub(crate) radius: f32,
    pub(crate) lock_target: EnemyLockTarget,
    pub(crate) target_pos: [f32; 2],
    pub(crate) waypoint: [f32; 2],
    pub(crate) repath_cooldown: u32,
}
#[derive(Bundle, Clone, Debug)]
pub(crate) struct EnemyBundle {
    pub role: EnemyState,
    pub position: Position,
    pub health: Health,
    pub facing: Facing,
    pub attack: Attack,
}
pub(crate) struct EnemyRef<'w> {
    pub role: &'w EnemyState,
    pub position: &'w Position,
    #[cfg(test)]
    pub health: &'w Health,
    #[cfg(test)]
    pub facing: &'w Facing,
}
pub(crate) struct EnemyMut<'w> {
    pub role: &'w mut EnemyState,
    pub position: &'w mut Position,
    pub health: &'w mut Health,
    pub facing: &'w mut Facing,
    pub attack: &'w mut Attack,
}
#[derive(Component, Clone, Debug)]
pub struct TowerState {
    pub(crate) id: u64,
    pub(crate) owner: u64,
    pub(crate) lane: u8,
    pub(crate) node_id: u32,
    pub(crate) reload_ticks: u32,
}
#[derive(Bundle, Clone, Debug)]
pub(crate) struct TowerBundle {
    pub role: TowerState,
    pub position: Position,
}
pub(crate) struct TowerRef<'w> {
    pub role: &'w TowerState,
    pub position: &'w Position,
}

#[derive(Component, Default)]
pub(crate) struct EnemyStep {
    pub hero_hit: Option<(u64, f32)>,
    pub objective_hit: bool,
}
#[derive(Component, Default)]
pub(crate) struct HeroStep {
    pub attack_triggered: bool,
}
#[derive(Component, Default)]
pub(crate) struct TowerStep {
    pub shot: Option<(u64, u64)>,
}

#[derive(Component, Clone, Copy, Debug)]
pub struct Presentation {
    pub instance: game_shared::PresentationInstance,
    pub order: u64,
}
