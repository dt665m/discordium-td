//! Exact generated field schemas for every positive Dreamwake representation.
//! Stable IDs and explicit conversions keep wire state separate from gameplay
//! ownership; existing game validators remain mandatory after staged decoding.
use engine_net::{schema::*, types::SchemaId};
mod collision;
mod components;
mod enums;
mod marker;
mod motion;
mod owner;
mod platform;
mod public;
mod starfall;
use collision::Collision;
use components::*;
use enums::*;
use marker::CoverMarker;
use motion::*;
use owner::Owner;
use platform::Platform;
use public::*;

const F: Range = Range::Float32 {
    minimum: -f32::MAX,
    maximum: f32::MAX,
};
const NONNEG: Range = Range::Float32 {
    minimum: 0.0,
    maximum: f32::MAX,
};
const U8: Range = Range::Unsigned {
    minimum: 0,
    maximum: u8::MAX as u64,
};
const U16: Range = Range::Unsigned {
    minimum: 0,
    maximum: u16::MAX as u64,
};
const U32: Range = Range::Unsigned {
    minimum: 0,
    maximum: u32::MAX as u64,
};
const U64: Range = Range::Unsigned {
    minimum: 0,
    maximum: u64::MAX,
};
const fn p(units: Units, range: Range) -> FieldSpec {
    FieldSpec::exact(units, range, Visibility::Public, Prediction::None)
}
const fn linear(units: Units, range: Range) -> FieldSpec {
    FieldSpec {
        interpolation: Interpolation::Linear,
        ..p(units, range)
    }
}
const fn o(units: Units, range: Range) -> FieldSpec {
    FieldSpec::exact(units, range, Visibility::Owner, Prediction::Replayed)
}
const fn d(units: Units, range: Range) -> FieldSpec {
    FieldSpec::exact(units, range, Visibility::Public, Prediction::Dependency)
}
fn invalid(_: impl std::fmt::Display) -> SchemaError {
    SchemaError::InvalidEncoding
}

trait Root: Schema + CompactValue {
    type Model;
    fn capture(value: &Self::Model) -> Result<Self, SchemaError>;
    fn restore(&self) -> Result<Self::Model, SchemaError>;
}
fn registration<S: Root>(lifecycle: LifecycleDeclaration) -> Registration<S, S::Model> {
    Registration {
        capture: S::capture,
        restore: Some(|_, value| value.restore()),
        dependencies: DependencyDeclaration::NoneRequired,
        lifecycle,
    }
}

struct Registry {
    identities: Vec<SchemaIdentity>,
    global: RegisteredSchema<Global, crate::replication::DreamGlobalView>,
    owner: RegisteredSchema<Owner, crate::replication::OwnerCheckpoint>,
    collision: RegisteredSchema<Collision, crate::collision::CollisionManifest>,
    hero: RegisteredSchema<Hero, crate::replication::PublicHeroView>,
    enemy: RegisteredSchema<Enemy, crate::replication::PublicEnemyView>,
    projectile: RegisteredSchema<Projectile, crate::replication::PublicProjectileView>,
    wisp: RegisteredSchema<Wisp, crate::replication::PublicWispView>,
    effect: RegisteredSchema<Effect, crate::replication::PublicEffectView>,
    damage: RegisteredSchema<Damage, crate::replication::PublicDamageView>,
    cover: RegisteredSchema<Cover, crate::replication::PublicCoverView>,
    cover_marker: RegisteredSchema<CoverMarker, crate::replication::PublicCoverMarkerView>,
    platform: RegisteredSchema<Platform, crate::replication::PublicPlatformView>,
}
fn registry() -> &'static Registry {
    static REGISTRY: std::sync::OnceLock<Registry> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| Registry::new().expect("complete bounded Dreamwake schemas"))
}
impl Registry {
    fn new() -> Result<Self, SchemaError> {
        let mut schemas = SchemaRegistry::new(SchemaLimits {
            wire_bytes: 16_374,
            nesting: 16,
            ..SchemaLimits::default()
        })?;
        let global = schemas.register(registration::<Global>(LifecycleDeclaration::External {
            policy: 1,
        }))?;
        let mut owner_hooks = registration::<Owner>(LifecycleDeclaration::External { policy: 1 });
        owner_hooks.dependencies = DependencyDeclaration::External { policy: 1 };
        let owner = schemas.register(owner_hooks)?;
        let collision =
            schemas.register(registration::<Collision>(LifecycleDeclaration::StaticOnly))?;
        let hero = schemas.register(registration::<Hero>(LifecycleDeclaration::External {
            policy: 1,
        }))?;
        let enemy = schemas.register(registration::<Enemy>(LifecycleDeclaration::External {
            policy: 1,
        }))?;
        let projectile =
            schemas.register(registration::<Projectile>(LifecycleDeclaration::External {
                policy: 1,
            }))?;
        let wisp = schemas.register(registration::<Wisp>(LifecycleDeclaration::External {
            policy: 1,
        }))?;
        let effect = schemas.register(registration::<Effect>(LifecycleDeclaration::External {
            policy: 1,
        }))?;
        let damage = schemas.register(registration::<Damage>(LifecycleDeclaration::External {
            policy: 1,
        }))?;
        let cover = schemas.register(registration::<Cover>(LifecycleDeclaration::External {
            policy: 1,
        }))?;
        let mut marker_hooks =
            registration::<CoverMarker>(LifecycleDeclaration::External { policy: 1 });
        marker_hooks.dependencies = DependencyDeclaration::External { policy: 2 };
        let cover_marker = schemas.register(marker_hooks)?;
        let platform =
            schemas.register(registration::<Platform>(LifecycleDeclaration::External {
                policy: 1,
            }))?;
        Ok(Self {
            identities: schemas.identities().to_vec(),
            global,
            owner,
            collision,
            hero,
            enemy,
            projectile,
            wisp,
            effect,
            damage,
            cover,
            cover_marker,
            platform,
        })
    }
}
/// Sorted identities for every wire root and its nested field records.
pub fn schema_identities() -> &'static [SchemaIdentity] {
    &registry().identities
}
/// Content handshake digest binds the exact generated field contract.
pub fn schema_identity() -> [u8; 32] {
    let mut hash = blake3::Hasher::new();
    hash.update(b"dreamwake/replication-schema/v1");
    for identity in schema_identities() {
        hash.update(&identity.schema.0.to_le_bytes());
        hash.update(&identity.version.to_le_bytes());
        hash.update(&identity.fingerprint);
    }
    *hash.finalize().as_bytes()
}
fn error(error: SchemaError) -> crate::replication::ReplicationError {
    crate::replication::ReplicationError::Codec(error.to_string())
}
fn encode<S: Root>(
    registered: &RegisteredSchema<S, S::Model>,
    v: &S::Model,
) -> Result<Vec<u8>, crate::replication::ReplicationError> {
    registered
        .codec()
        .encode(&registered.capture(v).map_err(error)?)
        .map_err(error)
}
fn decode<S: Root>(
    registered: &RegisteredSchema<S, S::Model>,
    bytes: &[u8],
) -> Result<S::Model, crate::replication::ReplicationError> {
    let state = registered.codec().decode(bytes).map_err(error)?;
    let base = state.restore().map_err(error)?;
    registered.restore_replacement(&base, &state).map_err(error)
}
fn encode_compact<S: Root>(
    registered: &RegisteredSchema<S, S::Model>,
    v: &S::Model,
) -> Result<Vec<u8>, crate::replication::ReplicationError> {
    registered
        .codec()
        .encode_compact(&registered.capture(v).map_err(error)?)
        .map_err(error)
}
fn decode_compact<S: Root>(
    registered: &RegisteredSchema<S, S::Model>,
    bytes: &[u8],
) -> Result<S::Model, crate::replication::ReplicationError> {
    let state = registered.codec().decode_compact(bytes).map_err(error)?;
    let base = state.restore().map_err(error)?;
    registered.restore_replacement(&base, &state).map_err(error)
}
pub(super) fn encode_global(
    v: &crate::replication::DreamGlobalView,
) -> Result<Vec<u8>, crate::replication::ReplicationError> {
    encode(&registry().global, v)
}
pub(super) fn decode_global(
    bytes: &[u8],
) -> Result<crate::replication::DreamGlobalView, crate::replication::ReplicationError> {
    decode(&registry().global, bytes)
}
pub(super) fn encode_owner(
    v: &crate::replication::OwnerCheckpoint,
) -> Result<Vec<u8>, crate::replication::ReplicationError> {
    encode(&registry().owner, v)
}
pub(super) fn decode_owner(
    bytes: &[u8],
) -> Result<crate::replication::OwnerCheckpoint, crate::replication::ReplicationError> {
    decode(&registry().owner, bytes)
}
pub(crate) fn encode_collision(
    v: &crate::collision::CollisionManifest,
) -> Result<Vec<u8>, crate::replication::ReplicationError> {
    encode(&registry().collision, v)
}
pub(crate) fn decode_collision(
    bytes: &[u8],
) -> Result<crate::collision::CollisionManifest, crate::replication::ReplicationError> {
    decode(&registry().collision, bytes)
}
pub(super) fn encode_public(
    v: &crate::replication::PublicReplica,
) -> Result<Vec<u8>, crate::replication::ReplicationError> {
    use crate::replication::PublicReplica as P;
    let (tag, bytes) = match v {
        P::Hero(v) => (1, encode(&registry().hero, v)?),
        P::Enemy(v) => (2, encode(&registry().enemy, v)?),
        P::Projectile(v) => (3, encode(&registry().projectile, v)?),
        P::Wisp(v) => (4, encode(&registry().wisp, v)?),
        P::Effect(v) => (5, encode(&registry().effect, v)?),
        P::Damage(v) => (6, encode(&registry().damage, v)?),
        P::Cover(v) => (7, encode(&registry().cover, v)?),
        P::Platform(v) => (8, encode(&registry().platform, v)?),
        P::CoverMarker(v) => (9, encode(&registry().cover_marker, v)?),
    };
    let mut out = Vec::with_capacity(bytes.len() + 1);
    out.push(tag);
    out.extend(bytes);
    Ok(out)
}
pub(super) fn decode_public(
    bytes: &[u8],
) -> Result<crate::replication::PublicReplica, crate::replication::ReplicationError> {
    use crate::replication::{PublicReplica as P, ReplicationError};
    let (tag, body) = bytes.split_first().ok_or(ReplicationError::InvalidState)?;
    match tag {
        1 => decode(&registry().hero, body).map(P::Hero),
        2 => decode(&registry().enemy, body).map(P::Enemy),
        3 => decode(&registry().projectile, body).map(P::Projectile),
        4 => decode(&registry().wisp, body).map(P::Wisp),
        5 => decode(&registry().effect, body).map(P::Effect),
        6 => decode(&registry().damage, body).map(P::Damage),
        7 => decode(&registry().cover, body).map(P::Cover),
        8 => decode(&registry().platform, body).map(P::Platform),
        9 => decode(&registry().cover_marker, body).map(P::CoverMarker),
        _ => Err(ReplicationError::InvalidState),
    }
}
pub(super) fn encode_public_compact(
    v: &crate::replication::PublicReplica,
) -> Result<Vec<u8>, crate::replication::ReplicationError> {
    use crate::replication::PublicReplica as P;
    let (tag, bytes) = match v {
        P::Hero(v) => (1, encode_compact(&registry().hero, v)?),
        P::Enemy(v) => (2, encode_compact(&registry().enemy, v)?),
        P::Projectile(v) => (3, encode_compact(&registry().projectile, v)?),
        P::Wisp(v) => (4, encode_compact(&registry().wisp, v)?),
        P::Effect(v) => (5, encode_compact(&registry().effect, v)?),
        P::Damage(v) => (6, encode_compact(&registry().damage, v)?),
        P::Cover(v) => (7, encode_compact(&registry().cover, v)?),
        P::Platform(v) => (8, encode_compact(&registry().platform, v)?),
        P::CoverMarker(v) => (9, encode_compact(&registry().cover_marker, v)?),
    };
    let mut out = Vec::with_capacity(bytes.len() + 1);
    out.push(tag);
    out.extend(bytes);
    Ok(out)
}
pub(super) fn decode_public_compact(
    bytes: &[u8],
) -> Result<crate::replication::PublicReplica, crate::replication::ReplicationError> {
    use crate::replication::{PublicReplica as P, ReplicationError};
    let (tag, body) = bytes.split_first().ok_or(ReplicationError::InvalidState)?;
    match tag {
        1 => decode_compact(&registry().hero, body).map(P::Hero),
        2 => decode_compact(&registry().enemy, body).map(P::Enemy),
        3 => decode_compact(&registry().projectile, body).map(P::Projectile),
        4 => decode_compact(&registry().wisp, body).map(P::Wisp),
        5 => decode_compact(&registry().effect, body).map(P::Effect),
        6 => decode_compact(&registry().damage, body).map(P::Damage),
        7 => decode_compact(&registry().cover, body).map(P::Cover),
        8 => decode_compact(&registry().platform, body).map(P::Platform),
        9 => decode_compact(&registry().cover_marker, body).map(P::CoverMarker),
        _ => Err(ReplicationError::InvalidState),
    }
}
#[cfg(test)]
mod tests;

/// Maximum encoded schema payloads in owner-group order: global, owner,
/// immutable collision, rotating support. Outer replica/group framing is extra.
pub fn owner_group_schema_maxima() -> Result<[usize; 4], crate::replication::ReplicationError> {
    let limits = SchemaLimits {
        wire_bytes: usize::MAX,
        nesting: 16,
        ..SchemaLimits::default()
    };
    let maximum = || Work::new(limits);
    Ok([
        record_maximum::<Global>(&mut maximum(), 0)
            .map_err(|e| crate::replication::ReplicationError::Codec(e.to_string()))?
            + 36,
        record_maximum::<Owner>(&mut maximum(), 0)
            .map_err(|e| crate::replication::ReplicationError::Codec(e.to_string()))?
            + 36,
        record_maximum::<Collision>(&mut maximum(), 0)
            .map_err(|e| crate::replication::ReplicationError::Codec(e.to_string()))?
            + 36,
        record_maximum::<Platform>(&mut maximum(), 0)
            .map_err(|e| crate::replication::ReplicationError::Codec(e.to_string()))?
            + 36,
    ])
}
