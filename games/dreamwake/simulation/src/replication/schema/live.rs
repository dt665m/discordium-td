//! The live form of every replicated value. A [`ReplicaState`] is the exact,
//! already quantized wire value, so both peers hold the same thing and either
//! can use it as the base of a delta: the encoding of a state against a base
//! carries only the fields that differ.
use super::*;
use crate::replication::{
    DreamGlobalView, OwnerCheckpoint, OwnerExpectation, PublicReplica, ReplicationError,
};

/// A replicated value in game terms.
#[derive(Debug, Clone, PartialEq)]
pub enum Replica {
    Global(DreamGlobalView),
    /// Visible only to its owner's connection.
    Owner(OwnerCheckpoint),
    Actor(PublicReplica),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplicaKind {
    Global,
    Owner,
    Actor,
}

#[derive(Debug, Clone, PartialEq)]
enum Wire {
    Global(Global),
    Owner(Owner),
    Hero(Hero),
    Enemy(Enemy),
    Projectile(Projectile),
    Wisp(Wisp),
    Effect(Effect),
    Damage(Damage),
    Cover(Cover),
    Platform(Platform),
    CoverMarker(CoverMarker),
}

/// The exact value a stream entry carries and that serves as a delta base.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplicaState(Wire);

/// Set on the tag when the body holds changes against a base.
const DELTA: u8 = 0x80;

fn capture<S: Root>(
    registered: &RegisteredSchema<S, S::Model>,
    value: &S::Model,
) -> Result<S, ReplicationError> {
    registered.capture(value).map_err(error)
}
fn restore<S: Root>(
    registered: &RegisteredSchema<S, S::Model>,
    state: &S,
) -> Result<S::Model, ReplicationError> {
    let model = state.restore().map_err(error)?;
    registered.restore_replacement(&model, state).map_err(error)
}
fn write<S: Root>(
    registered: &RegisteredSchema<S, S::Model>,
    tag: u8,
    value: &S,
    base: Option<&S>,
    out: &mut Vec<u8>,
) -> Result<(), ReplicationError> {
    let codec = registered.codec();
    let (tag, body) = match base {
        Some(base) => (tag | DELTA, codec.encode_compact_delta(value, base)),
        None => (tag, codec.encode_compact(value)),
    };
    out.push(tag);
    out.extend_from_slice(&body.map_err(error)?);
    Ok(())
}
fn read<S: Root>(
    registered: &RegisteredSchema<S, S::Model>,
    body: &[u8],
    base: Option<&S>,
) -> Result<S, ReplicationError> {
    let codec = registered.codec();
    match base {
        Some(base) => codec.decode_compact_delta(base, body),
        None => codec.decode_compact(body),
    }
    .map_err(error)
}

macro_rules! dispatch {
    ($($tag:literal => $variant:ident ($field:ident)),+ $(,)?) => {
        impl Wire {
            fn tag(&self) -> u8 {
                match self {$(Self::$variant(_) => $tag),+}
            }
            fn encode(&self, base: Option<&Self>, out: &mut Vec<u8>) -> Result<(), ReplicationError> {
                match self {
                    $(Self::$variant(value) => {
                        let base = match base {
                            Some(Self::$variant(base)) => Some(base),
                            _ => None,
                        };
                        write(&registry().$field, $tag, value, base, out)
                    })+
                }
            }
            fn decode(bytes: &[u8], base: Option<&Self>) -> Result<Self, ReplicationError> {
                let (tag, body) = bytes.split_first().ok_or(ReplicationError::InvalidState)?;
                let delta = tag & DELTA != 0;
                match tag & !DELTA {
                    $($tag => {
                        let base = match (delta, base) {
                            (false, _) => None,
                            (true, Some(Self::$variant(base))) => Some(base),
                            (true, _) => return Err(ReplicationError::InvalidState),
                        };
                        read(&registry().$field, body, base).map(Self::$variant)
                    })+
                    _ => Err(ReplicationError::InvalidState),
                }
            }
        }
    };
}
dispatch! {
    1 => Global(global),
    2 => Owner(owner),
    3 => Hero(hero),
    4 => Enemy(enemy),
    5 => Projectile(projectile),
    6 => Wisp(wisp),
    7 => Effect(effect),
    8 => Damage(damage),
    9 => Cover(cover),
    10 => Platform(platform),
    11 => CoverMarker(cover_marker),
}

impl ReplicaState {
    /// Validate and quantize a value for the stream.
    pub fn capture(value: &Replica) -> Result<Self, ReplicationError> {
        let r = registry();
        Ok(Self(match value {
            Replica::Global(v) => Wire::Global(capture(&r.global, v)?),
            Replica::Owner(v) => Wire::Owner(capture(&r.owner, v)?),
            Replica::Actor(v) => match v {
                PublicReplica::Hero(v) => Wire::Hero(capture(&r.hero, v)?),
                PublicReplica::Enemy(v) => Wire::Enemy(capture(&r.enemy, v)?),
                PublicReplica::Projectile(v) => Wire::Projectile(capture(&r.projectile, v)?),
                PublicReplica::Wisp(v) => Wire::Wisp(capture(&r.wisp, v)?),
                PublicReplica::Effect(v) => Wire::Effect(capture(&r.effect, v)?),
                PublicReplica::Damage(v) => Wire::Damage(capture(&r.damage, v)?),
                PublicReplica::Cover(v) => Wire::Cover(capture(&r.cover, v)?),
                PublicReplica::Platform(v) => Wire::Platform(capture(&r.platform, v)?),
                PublicReplica::CoverMarker(v) => Wire::CoverMarker(capture(&r.cover_marker, v)?),
            },
        }))
    }
    pub fn kind(&self) -> ReplicaKind {
        match self.0 {
            Wire::Global(_) => ReplicaKind::Global,
            Wire::Owner(_) => ReplicaKind::Owner,
            _ => ReplicaKind::Actor,
        }
    }
    /// The validated game value. Owner state is only restored for the
    /// connection's own authenticated expectation.
    pub fn restore(&self, owner: Option<OwnerExpectation>) -> Result<Replica, ReplicationError> {
        let r = registry();
        Ok(match &self.0 {
            Wire::Global(v) => {
                let value = restore(&r.global, v)?;
                value.validate()?;
                Replica::Global(value)
            }
            Wire::Owner(v) => {
                let expected = owner.ok_or(ReplicationError::WrongOwner)?;
                let value = restore(&r.owner, v)?;
                value.validate_for(expected)?;
                Replica::Owner(value)
            }
            Wire::Hero(v) => Replica::Actor(PublicReplica::Hero(restore(&r.hero, v)?)),
            Wire::Enemy(v) => Replica::Actor(PublicReplica::Enemy(restore(&r.enemy, v)?)),
            Wire::Projectile(v) => {
                Replica::Actor(PublicReplica::Projectile(restore(&r.projectile, v)?))
            }
            Wire::Wisp(v) => Replica::Actor(PublicReplica::Wisp(restore(&r.wisp, v)?)),
            Wire::Effect(v) => Replica::Actor(PublicReplica::Effect(restore(&r.effect, v)?)),
            Wire::Damage(v) => Replica::Actor(PublicReplica::Damage(restore(&r.damage, v)?)),
            Wire::Cover(v) => Replica::Actor(PublicReplica::Cover(restore(&r.cover, v)?)),
            Wire::Platform(v) => Replica::Actor(PublicReplica::Platform(restore(&r.platform, v)?)),
            Wire::CoverMarker(v) => {
                Replica::Actor(PublicReplica::CoverMarker(restore(&r.cover_marker, v)?))
            }
        })
    }
    /// Append this state, whole or as changes against `base` when `base` holds
    /// the same kind of value.
    pub fn encode(&self, base: Option<&Self>, out: &mut Vec<u8>) -> Result<(), ReplicationError> {
        self.0.encode(base.map(|base| &base.0), out)
    }
    /// Read a state written by [`Self::encode`] against the same `base`.
    pub fn decode(bytes: &[u8], base: Option<&Self>) -> Result<Self, ReplicationError> {
        Wire::decode(bytes, base.map(|base| &base.0)).map(Self)
    }
    /// Whether an encoding against `base` would be a delta.
    pub fn deltas_from(&self, base: &Self) -> bool {
        self.0.tag() == base.0.tag()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replication::PublicEnemyView;

    fn enemy(x: f32) -> Replica {
        Replica::Actor(PublicReplica::Enemy(PublicEnemyView {
            id: 7,
            position: [x, 4.0],
            facing: [0.6, 0.8],
            velocity: [1.5, 0.0],
            hp: 10.0,
            max_hp: 12.0,
            kind: crate::EnemyKind::Melee,
            windup: 0.0,
            target: [x, 4.0],
            warn_radius: 0.0,
            phase: 0,
            slowed: false,
            hit_flash: 0.0,
        }))
    }

    #[test]
    fn moving_actors_send_a_few_bytes_and_restore_quantized() {
        let before = ReplicaState::capture(&enemy(1.0)).unwrap();
        let after = ReplicaState::capture(&enemy(1.075)).unwrap();
        let mut whole = Vec::new();
        after.encode(None, &mut whole).unwrap();
        let mut delta = Vec::new();
        after.encode(Some(&before), &mut delta).unwrap();
        assert!(delta.len() <= 10, "{delta:?}");
        assert!(delta.len() * 4 < whole.len(), "{} vs {}", delta.len(), whole.len());
        let decoded = ReplicaState::decode(&delta, Some(&before)).unwrap();
        assert_eq!(decoded, after);
        assert_eq!(ReplicaState::decode(&whole, None).unwrap(), after);
        let Replica::Actor(PublicReplica::Enemy(view)) = decoded.restore(None).unwrap() else {
            panic!("enemy");
        };
        assert_eq!(view.position[0], 1101.0 / 1024.0);
        // A delta needs its base, and a different kind cannot be one.
        assert!(ReplicaState::decode(&delta, None).is_err());
    }
}
