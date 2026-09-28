use super::*;

pub(crate) fn clamp(position: [f32; 2]) -> [f32; 2] {
    engine_core::CircleBounds {
        center: [0.0; 2],
        radius: ARENA_RADIUS - 0.8,
    }
    .clamp(position)
}
pub(crate) fn rotate(direction: [f32; 2], angle: f32) -> [f32; 2] {
    let (s, c) = angle.sin_cos();
    [
        direction[0] * c - direction[1] * s,
        direction[0] * s + direction[1] * c,
    ]
}
pub(crate) fn decrement(timer: &mut f32) {
    engine_core::advance_cooldown(timer, DT);
}

#[derive(Clone, Copy)]
pub(crate) struct ActionEffect {
    pub(crate) sequence: u32,
    pub(crate) slot: u16,
    pub(crate) scope: Option<engine_core::GraphicsScope>,
    pub(crate) match_epoch: Option<u32>,
}
impl ActionEffect {
    pub(crate) fn input(sequence: u32, tick: u32, slot: u16) -> Self {
        Self {
            sequence: if sequence == 0 { tick } else { sequence },
            slot,
            scope: None,
            match_epoch: None,
        }
    }
    pub(crate) fn network(key: crate::combat::RayActionKey, slot: u16) -> Self {
        Self {
            sequence: u32::try_from(key.command_sequence).unwrap_or(u32::MAX),
            slot,
            scope: Some(key.graphics_scope()),
            match_epoch: Some(key.match_epoch),
        }
    }
    pub(crate) fn from_id(id: engine_core::GraphicsId) -> Self {
        Self {
            sequence: id.action_seq,
            slot: id.slot,
            scope: id.scope,
            match_epoch: Some(id.match_epoch),
        }
    }
    pub(crate) fn repeat(self) -> Self {
        Self {
            slot: self.slot | 0x0800,
            ..self
        }
    }
    pub(crate) fn offset(self, offset: u16) -> Self {
        Self {
            slot: self.slot + offset,
            ..self
        }
    }
    pub(crate) fn id(self, fallback_match_epoch: u32, owner: u64) -> engine_core::GraphicsId {
        engine_core::GraphicsId {
            scope: self.scope,
            match_epoch: self.match_epoch.unwrap_or(fallback_match_epoch),
            owner,
            action_seq: self.sequence,
            slot: self.slot,
        }
    }
}

pub(crate) fn action_effect(
    commands: &mut Commands,
    run: &Run,
    owner: u64,
    source: ActionEffect,
    position: [f32; 2],
    radius: f32,
    duration: u32,
) {
    commands.spawn((
        DreamOwned,
        engine_core::GraphicsInstance {
            id: source.id((run.seed ^ (run.seed >> 32)) as u32, owner),
            kind: engine_core::GraphicsKind::RadialPulse,
            pos: position,
            radius,
            age_ticks: 0,
            duration_ticks: duration,
        },
    ));
}

pub(crate) fn number(
    commands: &mut Commands,
    run: &mut Run,
    position: [f32; 2],
    amount: f32,
    critical: bool,
    friendly: bool,
) {
    commands.spawn((
        DreamOwned,
        Number(DamageNumber {
            id: run.id(),
            position,
            amount,
            critical,
            friendly,
            age: 0.0,
        }),
    ));
}
