//! Typed owner component snapshots, including latent motion/action/random state.
use super::*;
use engine_net::codec::BoundedVec;
macro_rules! component {
    ($wire:ident,$model:path,$id:literal,{$($fid:literal=>$field:ident:$ty:ty=$default:expr=>$spec:expr),+$(,)?}) => {
        engine_net::schema! {pub(super) struct $wire(SchemaId($id),1,Representation::OwnerCheckpoint) {$($fid=>$field:$ty=$default=>$spec),+}}
        impl From<&$model> for $wire {fn from(v:&$model)->Self {Self{$($field:v.$field),+}}}
        impl From<$wire> for $model {fn from(v:$wire)->Self {Self{$($field:v.$field),+}}}
    }
}
component!(Health,engine_core::Health,0x1200,{
    1=>hp:f32=220.0=>o(Units::Unitless,NONNEG),
    2=>max_hp:f32=220.0=>o(Units::Unitless,NONNEG),
});
component!(Action,engine_core::ActionState,0x1201,{
    1=>target:[f32;2]=[0.0;2]=>o(Units::Metres,F),
    2=>windup:f32=0.0=>o(Units::Seconds,NONNEG),
    3=>recovery:f32=0.0=>o(Units::Seconds,NONNEG),
    4=>sequence:u32=0=>o(Units::Unitless,U32),
    5=>due:bool=false=>o(Units::Unitless,Range::None),
});
component!(Progression,engine_core::Progression,0x1202,{
    1=>level:u32=1=>o(Units::Unitless,U32),
    2=>xp:f32=0.0=>o(Units::Unitless,NONNEG),
    3=>xp_next:f32=50.0=>o(Units::Unitless,NONNEG),
    4=>pending_levels:u32=0=>o(Units::Unitless,U32),
});
engine_net::schema! {pub(super) struct Random(SchemaId(0x1203),1,Representation::OwnerCheckpoint) {
    1=>key:[u8;32]=engine_core::system::RandomStream::new(0,1,1,crate::state::CRITICAL_RANDOM_DOMAIN).unwrap().snapshot().key=>o(Units::Unitless,U8),
    2=>counter:u64=0=>o(Units::Unitless,U64),
}}
impl From<&engine_core::system::RandomStream> for Random {
    fn from(v: &engine_core::system::RandomStream) -> Self {
        let state = v.snapshot();
        Self {
            key: state.key,
            counter: state.counter,
        }
    }
}
impl From<Random> for engine_core::system::RandomStream {
    fn from(v: Random) -> Self {
        Self::from_state(engine_core::system::RandomStreamState {
            key: v.key,
            counter: v.counter,
        })
    }
}
engine_net::schema! {pub(super) struct Combat(SchemaId(0x1205),2,Representation::OwnerCheckpoint) {
    1=>shield:f32=0.0=>o(Units::Unitless,NONNEG),
    2=>shield_remaining:f32=0.0=>o(Units::Seconds,NONNEG),
    3=>invulnerability_remaining:f32=0.0=>o(Units::Seconds,NONNEG),
    5=>status_ids:BoundedVec<u16,64>=BoundedVec::default()=>o(Units::Unitless,U16),
    6=>status_magnitudes:BoundedVec<f32,64>=BoundedVec::default()=>o(Units::Unitless,F),
    7=>status_remaining:BoundedVec<f32,64>=BoundedVec::default()=>o(Units::Seconds,NONNEG),
}}
impl TryFrom<&engine_core::CombatState> for Combat {
    type Error = SchemaError;
    fn try_from(v: &engine_core::CombatState) -> Result<Self, SchemaError> {
        Ok(Self {
            shield: v.shield,
            shield_remaining: v.shield_remaining,
            invulnerability_remaining: v.invulnerability_remaining,
            status_ids: BoundedVec::new(v.statuses.iter().map(|s| s.id.0).collect())
                .map_err(invalid)?,
            status_magnitudes: BoundedVec::new(v.statuses.iter().map(|s| s.magnitude).collect())
                .map_err(invalid)?,
            status_remaining: BoundedVec::new(v.statuses.iter().map(|s| s.remaining).collect())
                .map_err(invalid)?,
        })
    }
}
impl TryFrom<Combat> for engine_core::CombatState {
    type Error = SchemaError;
    fn try_from(v: Combat) -> Result<Self, SchemaError> {
        if v.status_ids.len() != v.status_magnitudes.len()
            || v.status_ids.len() != v.status_remaining.len()
        {
            return Err(SchemaError::InvalidEncoding);
        }
        Ok(Self {
            shield: v.shield,
            shield_remaining: v.shield_remaining,
            invulnerability_remaining: v.invulnerability_remaining,
            statuses: v
                .status_ids
                .into_vec()
                .into_iter()
                .zip(v.status_magnitudes.into_vec())
                .zip(v.status_remaining.into_vec())
                .map(|((id, magnitude), remaining)| engine_core::TimedStatus {
                    id: engine_core::StatusId(id),
                    magnitude,
                    remaining,
                })
                .collect(),
        })
    }
}
engine_net::schema! {pub(super) struct Memory(SchemaId(0x1206),1,Representation::OwnerCheckpoint) {
    1=>kind:MemoryKind=MemoryKind::Crescent=>o(Units::Unitless,Range::None),
    2=>level:u8=1=>o(Units::Unitless,U8),
    3=>modifier:Option<Essence>=None=>o(Units::Unitless,Range::None),
    4=>cooldown:f32=0.0=>o(Units::Seconds,NONNEG),
    5=>max_cooldown:f32=1.0=>o(Units::Seconds,NONNEG),
}}
impl From<&engine_core::AbilitySlot<crate::MemoryKind, crate::EssenceKind>> for Memory {
    fn from(v: &engine_core::AbilitySlot<crate::MemoryKind, crate::EssenceKind>) -> Self {
        Self {
            kind: v.kind.into(),
            level: v.level,
            modifier: v.modifier.map(Into::into),
            cooldown: v.cooldown,
            max_cooldown: v.max_cooldown,
        }
    }
}
impl From<Memory> for engine_core::AbilitySlot<crate::MemoryKind, crate::EssenceKind> {
    fn from(v: Memory) -> Self {
        Self {
            kind: v.kind.into(),
            level: v.level,
            modifier: v.modifier.map(Into::into),
            cooldown: v.cooldown,
            max_cooldown: v.max_cooldown,
        }
    }
}
