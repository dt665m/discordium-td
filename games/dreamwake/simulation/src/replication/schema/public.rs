use super::*;
use crate::replication as game;
macro_rules! copy_schema {
    ($wire:ident,$model:path,$id:literal,{$($fid:literal=>$field:ident:$ty:ty=$default:expr=>$spec:expr),+$(,)?}) => {
        copy_schema!($wire,$model,$id,1,{$($fid=>$field:$ty=$default=>$spec),+});
    };
    ($wire:ident,$model:path,$id:literal,$version:literal,{$($fid:literal=>$field:ident:$ty:ty=$default:expr=>$spec:expr),+$(,)?}) => {
        engine_net::schema! {pub(super) struct $wire(SchemaId($id),$version,Representation::PublicPresentation) {$($fid=>$field:$ty=$default=>$spec),+}}
        impl From<&$model> for $wire {fn from(value:&$model)->Self {Self{$($field:value.$field),+}}}
        impl From<$wire> for $model {fn from(value:$wire)->Self {Self{$($field:value.$field),+}}}
    }
}
copy_schema!(Stamp,game::ReplicationStamp,0x1000,{
    1=>match_epoch:u32=1=>p(Units::Unitless,U32),
    2=>server_tick:u64=0=>p(Units::Ticks,U64),
    3=>gameplay_tick:u32=0=>p(Units::Ticks,U32),
    4=>scene_revision:u64=1=>p(Units::Unitless,U64),
    5=>revision:u64=1=>p(Units::Unitless,U64),
});
engine_net::schema! {pub(super) struct Global(SchemaId(0x1101),1,Representation::PublicPresentation) {
    1=>stamp:Stamp=Stamp::defaults()=>p(Units::Unitless,Range::None),
    2=>phase:Phase=Phase::Intro=>p(Units::Unitless,Range::None),
    3=>active_travelers:u16=0=>p(Units::Unitless,U16),
    4=>paused:bool=false=>p(Units::Unitless,Range::None),
    5=>room:u16=0=>p(Units::Unitless,U16),
    6=>realm:u16=0=>p(Units::Unitless,U16),
    7=>encounter_name:BoundedString<256>=BoundedString::default()=>p(Units::Unitless,Range::None),
    8=>kills:u32=0=>p(Units::Unitless,U32),
    9=>elapsed:f32=0.0=>p(Units::Seconds,NONNEG),
    10=>lucid:bool=false=>p(Units::Unitless,Range::None),
    11=>cleared:u32=0=>p(Units::Unitless,U32),
    12=>message:BoundedString<512>=BoundedString::default()=>p(Units::Unitless,Range::None),
}}
impl Root for Global {
    type Model = game::DreamGlobalView;
    fn capture(v: &Self::Model) -> Result<Self, SchemaError> {
        v.validate().map_err(invalid)?;
        Ok(Self {
            stamp: (&v.stamp).into(),
            phase: v.phase.into(),
            active_travelers: v.active_travelers,
            paused: v.paused,
            room: v.room.try_into().map_err(invalid)?,
            realm: v.realm.try_into().map_err(invalid)?,
            encounter_name: BoundedString::new(&v.encounter_name)?,
            kills: v.kills,
            elapsed: v.elapsed,
            lucid: v.lucid,
            cleared: v.cleared,
            message: BoundedString::new(&v.message)?,
        })
    }
    fn restore(&self) -> Result<Self::Model, SchemaError> {
        let value = Self::Model {
            stamp: self.stamp.clone().into(),
            phase: self.phase.into(),
            active_travelers: self.active_travelers,
            paused: self.paused,
            room: usize::from(self.room),
            realm: usize::from(self.realm),
            encounter_name: self.encounter_name.as_str().into(),
            kills: self.kills,
            elapsed: self.elapsed,
            lucid: self.lucid,
            cleared: self.cleared,
            message: self.message.as_str().into(),
        };
        value.validate().map_err(invalid)?;
        Ok(value)
    }
}
copy_schema!(Hero,game::PublicHeroView,0x1104,{
    1=>id:u64=1=>p(Units::Unitless,U64),
    2=>position:[f32;2]=[0.0;2]=>linear(Units::Metres,F),
    3=>elevation:f32=0.0=>p(Units::Metres,F),
    4=>crouched:bool=false=>p(Units::Unitless,Range::None),
    5=>facing:[f32;2]=[0.0,-1.0]=>p(Units::Unitless,F),
    6=>velocity:[f32;2]=[0.0;2]=>p(Units::Custom("metres/second"),F),
    7=>hp:f32=1.0=>p(Units::Unitless,NONNEG),
    8=>max_hp:f32=1.0=>p(Units::Unitless,NONNEG),
    9=>shield:f32=0.0=>p(Units::Unitless,NONNEG),
    10=>level:u32=1=>p(Units::Unitless,U32),
    11=>invulnerable:bool=false=>p(Units::Unitless,Range::None),
    12=>dashing:bool=false=>p(Units::Unitless,Range::None),
    13=>combo:u32=0=>p(Units::Unitless,U32),
    14=>hit_flash:f32=0.0=>p(Units::Seconds,F),
    15=>attack_flash:f32=0.0=>p(Units::Seconds,F),
});
impl Root for Hero {
    type Model = game::PublicHeroView;
    fn capture(v: &Self::Model) -> Result<Self, SchemaError> {
        game::PublicReplica::Hero(*v).validate().map_err(invalid)?;
        Ok(v.into())
    }
    fn restore(&self) -> Result<Self::Model, SchemaError> {
        let v: Self::Model = self.clone().into();
        game::PublicReplica::Hero(v).validate().map_err(invalid)?;
        Ok(v)
    }
}
engine_net::schema! {pub(super) struct Enemy(SchemaId(0x1105),1,Representation::PublicPresentation) {
    1=>id:u64=1=>p(Units::Unitless,U64),
    2=>position:[f32;2]=[0.0;2]=>linear(Units::Metres,F),
    3=>facing:[f32;2]=[0.0,-1.0]=>p(Units::Unitless,F),
    4=>hp:f32=1.0=>p(Units::Unitless,NONNEG),
    5=>max_hp:f32=1.0=>p(Units::Unitless,NONNEG),
    6=>kind:EnemyKind=EnemyKind::Melee=>p(Units::Unitless,Range::None),
    7=>windup:f32=0.0=>p(Units::Seconds,F),
    8=>target:[f32;2]=[0.0;2]=>p(Units::Metres,F),
    9=>warn_radius:f32=0.0=>p(Units::Metres,NONNEG),
    10=>phase:u8=0=>p(Units::Unitless,U8),
    11=>slowed:bool=false=>p(Units::Unitless,Range::None),
    12=>hit_flash:f32=0.0=>p(Units::Seconds,F),
}}
impl Root for Enemy {
    type Model = game::PublicEnemyView;
    fn capture(v: &Self::Model) -> Result<Self, SchemaError> {
        game::PublicReplica::Enemy(*v).validate().map_err(invalid)?;
        Ok(Self {
            id: v.id,
            position: v.position,
            facing: v.facing,
            hp: v.hp,
            max_hp: v.max_hp,
            kind: v.kind.into(),
            windup: v.windup,
            target: v.target,
            warn_radius: v.warn_radius,
            phase: v.phase,
            slowed: v.slowed,
            hit_flash: v.hit_flash,
        })
    }
    fn restore(&self) -> Result<Self::Model, SchemaError> {
        let v = Self::Model {
            id: self.id,
            position: self.position,
            facing: self.facing,
            hp: self.hp,
            max_hp: self.max_hp,
            kind: self.kind.clone().into(),
            windup: self.windup,
            target: self.target,
            warn_radius: self.warn_radius,
            phase: self.phase,
            slowed: self.slowed,
            hit_flash: self.hit_flash,
        };
        game::PublicReplica::Enemy(v).validate().map_err(invalid)?;
        Ok(v)
    }
}
engine_net::schema! {pub(super) struct Projectile(SchemaId(0x1106),1,Representation::PublicPresentation) {
    1=>id:u64=1=>p(Units::Unitless,U64),
    2=>owner:Option<u64>=None=>p(Units::Unitless,U64),
    3=>position:[f32;2]=[0.0;2]=>linear(Units::Metres,F),
    4=>direction:[f32;2]=[0.0,-1.0]=>p(Units::Unitless,F),
    5=>radius:f32=0.1=>p(Units::Metres,NONNEG),
    6=>friendly:bool=true=>p(Units::Unitless,Range::None),
    7=>essence:Option<Essence>=None=>p(Units::Unitless,Range::None),
}}
impl Root for Projectile {
    type Model = game::PublicProjectileView;
    fn capture(v: &Self::Model) -> Result<Self, SchemaError> {
        game::PublicReplica::Projectile(*v)
            .validate()
            .map_err(invalid)?;
        Ok(Self {
            id: v.id,
            owner: v.owner,
            position: v.position,
            direction: v.direction,
            radius: v.radius,
            friendly: v.friendly,
            essence: v.essence.map(Into::into),
        })
    }
    fn restore(&self) -> Result<Self::Model, SchemaError> {
        let v = Self::Model {
            id: self.id,
            owner: self.owner,
            position: self.position,
            direction: self.direction,
            radius: self.radius,
            friendly: self.friendly,
            essence: self.essence.map(Into::into),
        };
        game::PublicReplica::Projectile(v)
            .validate()
            .map_err(invalid)?;
        Ok(v)
    }
}
engine_net::schema! {pub(super) struct Wisp(SchemaId(0x1107),1,Representation::PublicPresentation) {
    1=>id:u64=1=>p(Units::Unitless,U64),
    2=>owner:Option<u64>=None=>p(Units::Unitless,U64),
    3=>position:[f32;2]=[0.0;2]=>linear(Units::Metres,F),
    4=>remaining:f32=0.0=>p(Units::Seconds,NONNEG),
    5=>essence:Option<Essence>=None=>p(Units::Unitless,Range::None),
}}
impl Root for Wisp {
    type Model = game::PublicWispView;
    fn capture(v: &Self::Model) -> Result<Self, SchemaError> {
        game::PublicReplica::Wisp(*v).validate().map_err(invalid)?;
        Ok(Self {
            id: v.id,
            owner: v.owner,
            position: v.position,
            remaining: v.remaining,
            essence: v.essence.map(Into::into),
        })
    }
    fn restore(&self) -> Result<Self::Model, SchemaError> {
        let v = Self::Model {
            id: self.id,
            owner: self.owner,
            position: self.position,
            remaining: self.remaining,
            essence: self.essence.map(Into::into),
        };
        game::PublicReplica::Wisp(v).validate().map_err(invalid)?;
        Ok(v)
    }
}
copy_schema!(Damage,game::PublicDamageView,0x1109,{
    1=>id:u64=1=>p(Units::Unitless,U64),
    2=>position:[f32;2]=[0.0;2]=>linear(Units::Metres,F),
    3=>amount:f32=0.0=>p(Units::Unitless,NONNEG),
    4=>critical:bool=false=>p(Units::Unitless,Range::None),
    5=>friendly:bool=false=>p(Units::Unitless,Range::None),
    6=>age:f32=0.0=>p(Units::Seconds,NONNEG),
});
impl Root for Damage {
    type Model = game::PublicDamageView;
    fn capture(v: &Self::Model) -> Result<Self, SchemaError> {
        game::PublicReplica::Damage(*v)
            .validate()
            .map_err(invalid)?;
        Ok(v.into())
    }
    fn restore(&self) -> Result<Self::Model, SchemaError> {
        let v: Self::Model = self.clone().into();
        game::PublicReplica::Damage(v).validate().map_err(invalid)?;
        Ok(v)
    }
}
copy_schema!(GraphicScope,engine_core::GraphicsScope,0x1004,{
    1=>source_epoch:u64=0=>p(Units::Unitless,U64),
    2=>stream:u32=0=>p(Units::Unitless,U32),
    3=>control_epoch:u32=0=>p(Units::Unitless,U32),
    4=>generation:u32=0=>p(Units::Unitless,U32),
});
engine_net::schema! {pub(super) struct GraphicId(SchemaId(0x1001),2,Representation::PublicPresentation) {
    1=>match_epoch:u32=1=>p(Units::Unitless,U32),
    2=>owner:u64=1=>p(Units::Unitless,U64),
    3=>action_seq:u32=0=>p(Units::Unitless,U32),
    4=>slot:u16=0=>p(Units::Unitless,U16),
    5=>scope:Option<GraphicScope>=None=>p(Units::Unitless,Range::None),
}}
impl From<&engine_core::GraphicsId> for GraphicId {
    fn from(v: &engine_core::GraphicsId) -> Self {
        Self {
            match_epoch: v.match_epoch,
            owner: v.owner,
            action_seq: v.action_seq,
            slot: v.slot,
            scope: v.scope.map(|scope| (&scope).into()),
        }
    }
}
impl From<GraphicId> for engine_core::GraphicsId {
    fn from(v: GraphicId) -> Self {
        Self {
            match_epoch: v.match_epoch,
            owner: v.owner,
            action_seq: v.action_seq,
            slot: v.slot,
            scope: v.scope.map(Into::into),
        }
    }
}
engine_net::schema! {pub(super) struct Effect(SchemaId(0x1108),1,Representation::PublicPresentation) {
    1=>id:GraphicId=GraphicId::defaults()=>p(Units::Unitless,Range::None),
    2=>kind:GraphicKind=GraphicKind::RadialPulse(())=>p(Units::Unitless,Range::None),
    3=>position:[f32;2]=[0.0;2]=>linear(Units::Metres,F),
    4=>radius:f32=0.0=>p(Units::Metres,NONNEG),
    5=>age_ticks:u32=0=>p(Units::Ticks,U32),
    6=>duration_ticks:u32=1=>p(Units::Ticks,U32),
}}
impl Root for Effect {
    type Model = game::PublicEffectView;
    fn capture(v: &Self::Model) -> Result<Self, SchemaError> {
        game::PublicReplica::Effect(*v)
            .validate()
            .map_err(invalid)?;
        Ok(Self {
            id: (&v.id).into(),
            kind: v.kind.into(),
            position: v.position,
            radius: v.radius,
            age_ticks: v.age_ticks,
            duration_ticks: v.duration_ticks,
        })
    }
    fn restore(&self) -> Result<Self::Model, SchemaError> {
        let v = Self::Model {
            id: self.id.clone().into(),
            kind: self.kind.clone().into(),
            position: self.position,
            radius: self.radius,
            age_ticks: self.age_ticks,
            duration_ticks: self.duration_ticks,
        };
        game::PublicReplica::Effect(v).validate().map_err(invalid)?;
        Ok(v)
    }
}
copy_schema!(Cover,game::PublicCoverView,0x110a,4,{
    1=>id:u64=1=>p(Units::Unitless,U64),
    2=>position:[f32;3]=[0.0,1.0,-6.0]=>linear(Units::Metres,F),
    3=>half_extents:[f32;3]=[1.5,1.0,0.15]=>p(Units::Metres,NONNEG),
    4=>open:bool=false=>p(Units::Unitless,Range::None),
    5=>present:bool=false=>p(Units::Unitless,Range::None),
    6=>revision:u64=1=>p(Units::Unitless,U64),
});
impl Root for Cover {
    type Model = game::PublicCoverView;
    fn capture(v: &Self::Model) -> Result<Self, SchemaError> {
        if !v.valid() {
            return Err(SchemaError::InvalidEncoding);
        }
        Ok(v.into())
    }
    fn restore(&self) -> Result<Self::Model, SchemaError> {
        let value: Self::Model = self.clone().into();
        if !value.valid() {
            return Err(SchemaError::InvalidEncoding);
        }
        Ok(value)
    }
}
