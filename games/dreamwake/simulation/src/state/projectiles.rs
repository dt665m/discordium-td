use super::*;
use engine_core::{ProjectileState, SummonState};

#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Projectile {
    pub damage: f32,
    pub essence: Option<EssenceKind>,
}
impl Projectile {
    pub fn snapshot(&self, state: &ProjectileState) -> ProjectileView {
        ProjectileView {
            id: state.id,
            owner: state.owner,
            position: state.position,
            direction: state.direction,
            radius: state.radius,
            friendly: state.faction == 1,
            essence: self.essence,
        }
    }
}

#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Wisp {
    pub damage: f32,
    pub essence: Option<EssenceKind>,
}
impl Wisp {
    pub fn snapshot(&self, state: &SummonState) -> WispView {
        WispView {
            id: state.id,
            owner: state.owner,
            position: state.position,
            remaining: state.remaining,
            essence: self.essence,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SavedProjectile {
    pub payload: Projectile,
    pub state: ProjectileState,
}
impl SavedProjectile {
    pub fn snapshot(&self) -> ProjectileView {
        self.payload.snapshot(&self.state)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SavedWisp {
    pub payload: Wisp,
    pub state: SummonState,
}
impl SavedWisp {
    pub fn snapshot(&self) -> WispView {
        self.payload.snapshot(&self.state)
    }
}
