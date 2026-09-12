use super::ReplicationError;
use crate::{EnemyKind, EssenceKind, RunPhase};
use serde::{Deserialize, Serialize};

/// End-of-authoritative-step time is distinct from the pausable gameplay clock.
/// These identities are supplied by the committed server driver, not inferred
/// from a render frame, global RNG seed, or a packet receive sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplicationStamp {
    pub match_epoch: u32,
    pub server_tick: u64,
    pub gameplay_tick: u32,
    pub scene_revision: u64,
    pub revision: u64,
}
impl ReplicationStamp {
    pub fn validate(&self) -> Result<(), ReplicationError> {
        if self.match_epoch == 0 || self.scene_revision == 0 || self.revision == 0 {
            return Err(ReplicationError::InvalidIdentity);
        }
        Ok(())
    }
}
/// Approved match summary only. Enemy population/future waves, private messages,
/// seeds, RNG state and global allocation counters have no representation here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DreamGlobalView {
    pub stamp: ReplicationStamp,
    pub phase: RunPhase,
    /// Public party count, independent of this connection's spatial actor scopes.
    pub active_travelers: u16,
    pub paused: bool,
    pub room: usize,
    pub realm: usize,
    pub encounter_name: String,
    pub kills: u32,
    pub elapsed: f32,
    pub lucid: bool,
    pub cleared: u32,
    pub message: String,
}
impl DreamGlobalView {
    pub fn validate(&self) -> Result<(), ReplicationError> {
        self.stamp.validate()?;
        if self.room > crate::TOTAL_ROOMS
            || self.realm != (self.room / 3).min(2)
            || !self.elapsed.is_finite()
            || self.elapsed < 0.0
            || self.encounter_name.len() > 2048
            || self.message.len() > 2048
        {
            return Err(ReplicationError::InvalidState);
        }
        Ok(())
    }
}
/// Public pose/health and visible action appearance. No loadout, XP, shards,
/// damage/stat multipliers, reward choices, cooldown schedule or random stream.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicHeroView {
    pub id: u64,
    pub position: [f32; 2],
    pub elevation: f32,
    pub crouched: bool,
    pub facing: [f32; 2],
    pub velocity: [f32; 2],
    pub hp: f32,
    pub max_hp: f32,
    pub shield: f32,
    pub level: u32,
    pub invulnerable: bool,
    pub dashing: bool,
    pub combo: u32,
    pub hit_flash: f32,
    pub attack_flash: f32,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicEnemyView {
    pub id: u64,
    pub position: [f32; 2],
    pub facing: [f32; 2],
    pub hp: f32,
    pub max_hp: f32,
    pub kind: EnemyKind,
    pub windup: f32,
    pub target: [f32; 2],
    pub warn_radius: f32,
    pub phase: u8,
    pub slowed: bool,
    pub hit_flash: f32,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicProjectileView {
    pub id: u64,
    /// Omitted when the source is not separately disclosed to this connection.
    pub owner: Option<u64>,
    pub position: [f32; 2],
    pub direction: [f32; 2],
    pub radius: f32,
    pub friendly: bool,
    pub essence: Option<EssenceKind>,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicWispView {
    pub id: u64,
    pub owner: Option<u64>,
    pub position: [f32; 2],
    pub remaining: f32,
    pub essence: Option<EssenceKind>,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicEffectView {
    /// Source-owned IDs are emitted only for authorized sources. An anonymous
    /// proxy reserves owner=0 and carries no hidden source or input identity.
    pub id: engine_core::GraphicsId,
    pub kind: engine_core::GraphicsKind,
    pub position: [f32; 2],
    pub radius: f32,
    pub age_ticks: u32,
    pub duration_ticks: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicDamageView {
    pub id: u64,
    pub position: [f32; 2],
    pub amount: f32,
    pub critical: bool,
    pub friendly: bool,
    pub age: f32,
}
/// Public current cover geometry; transition schedule remains server-owned.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicCoverView {
    pub id: u64,
    pub revision: u64,
    /// Current map-region state, retained even after authority removes the cover.
    pub present: bool,
    pub position: [f32; 3],
    pub half_extents: [f32; 3],
    /// Resting endpoint of this phase; current collision follows `position`.
    pub open: bool,
}
impl PublicCoverView {
    /// Dreamwake's veil region has one declared map slot. Its scoped cover root
    /// is the complete region manifest, including current absence after removal.
    pub fn region_manifest(
        &self,
        stamp: ReplicationStamp,
    ) -> Result<engine_net::replication::regions::RegionManifest<1>, ReplicationError> {
        use engine_net::{
            codec::BoundedVec,
            replication::regions::{MapObjectState, RegionManifest},
            types::SceneRevision,
        };
        stamp.validate()?;
        if !self.valid() {
            return Err(ReplicationError::InvalidState);
        }
        let manifest = RegionManifest {
            scene: SceneRevision(
                stamp
                    .scene_revision
                    .try_into()
                    .map_err(|_| ReplicationError::WrongScene)?,
            ),
            region: 1,
            revision: self.revision,
            objects: BoundedVec::new(vec![MapObjectState {
                slot: 1,
                generation: 1,
                present: self.present,
            }])
            .map_err(|_| ReplicationError::BudgetExceeded)?,
        };
        manifest
            .validate()
            .map_err(|_| ReplicationError::InvalidState)?;
        Ok(manifest)
    }

    pub fn valid(&self) -> bool {
        self.id > 0
            && self.revision > 0
            && crate::DEMO_COVER_POSITIONS.contains(&[self.position[0], self.position[2]])
            && (crate::COVER_CLOSED_HEIGHT..=crate::COVER_OPEN_HEIGHT).contains(&self.position[1])
            && self.half_extents == crate::COVER_HALF_EXTENTS
    }
}

/// Public adornment of a cover. Parent linkage conveys no inventory entitlement.
/// The server fills the exact connection scope after independently admitting both
/// roots. Clients stage this state until that exact parent scope is published.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicCoverMarkerView {
    pub id: u64,
    pub parent: engine_net::types::ScopeIdentity,
    pub revision: u64,
    pub local_offset: [f32; 3],
    pub open: bool,
}
impl PublicCoverMarkerView {
    pub fn valid(&self) -> bool {
        self.id > 0
            && self.revision > 0
            && self.parent.connection.0 > 0
            && self.parent.entity.index > 0
            && self.parent.entity.generation > 0
            && self.parent.scope.0 > 0
            && self.parent.representation.0 > 0
            && self.local_offset == [0.0, 1.35, 0.0]
    }
    pub fn attachment(&self) -> engine_net::replication::attachments::Attachment<[f32; 3]> {
        engine_net::replication::attachments::Attachment {
            parent: self.parent,
            revision: self.revision,
            local: self.local_offset,
            independent: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoverMarkerPresentation {
    pub id: u64,
    pub position: [f32; 3],
    pub open: bool,
}
/// Positive support pose and deterministic motion anchor for presentation and replay.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicPlatformView {
    pub id: u64,
    pub collider: engine_core::ColliderKey,
    pub scene_revision: u64,
    pub motion_revision: u32,
    pub origin_gameplay_tick: u32,
    pub gameplay_tick: u32,
    pub phase: u16,
    pub pose: engine_core::BasePose,
    pub half_extents: [f32; 3],
}
/// This is the only actor payload union intended for peer replication. It cannot
/// carry SavedState, DreamSnapshot, an arbitrary component or a raw source event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", deny_unknown_fields)]
pub enum PublicReplica {
    Hero(PublicHeroView),
    Enemy(PublicEnemyView),
    Projectile(PublicProjectileView),
    Wisp(PublicWispView),
    Effect(PublicEffectView),
    Damage(PublicDamageView),
    Cover(PublicCoverView),
    CoverMarker(PublicCoverMarkerView),
    Platform(PublicPlatformView),
}

/// A game/graph-approved anonymous audience representation. Coarse cells are
/// supplied after event policy, without a hidden actor ID or exact position.
#[derive(Debug, Clone, Copy)]
pub struct CoarseEffectProxy {
    pub proxy_id: u32,
    pub cell: [i32; 2],
    pub cell_size: u16,
    pub age_ticks: u32,
    pub duration_ticks: u32,
}
impl CoarseEffectProxy {
    pub fn project(self, stamp: ReplicationStamp) -> Result<PublicEffectView, ReplicationError> {
        stamp.validate()?;
        if self.proxy_id == 0
            || !(1..=1024).contains(&self.cell_size)
            || self.duration_ticks == 0
            || self.age_ticks > self.duration_ticks
            || self.cell.iter().any(|v| v.unsigned_abs() > 1_000_000)
        {
            return Err(ReplicationError::InvalidState);
        }
        let size = f32::from(self.cell_size);
        Ok(PublicEffectView {
            id: engine_core::GraphicsId {
                scope: None,
                match_epoch: stamp.match_epoch,
                owner: 0,
                action_seq: self.proxy_id,
                slot: u16::MAX,
            },
            kind: engine_core::GraphicsKind::RadialPulse,
            position: self.cell.map(|v| (v as f32 + 0.5) * size),
            radius: size,
            age_ticks: self.age_ticks,
            duration_ticks: self.duration_ticks,
        })
    }
}
