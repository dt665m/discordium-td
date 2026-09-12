use super::*;
use crate::replication::PublicPlatformView;
engine_net::schema! {pub(super) struct Pose(SchemaId(0x1005),1,Representation::PublicPresentation) {
1=>position:[f32;3]=crate::platform::PLATFORM_POSITION=>d(Units::Metres,F),
2=>rotation:[f32;4]=[0.0,0.0,0.0,1.0]=>d(Units::Unitless,F),
3=>linear_velocity:[f32;3]=[0.0;3]=>d(Units::Custom("metres/second"),F),
4=>angular_velocity:[f32;3]=[0.0,0.3681554,0.0]=>d(Units::Custom("radians/second"),F),
}}
impl From<engine_core::BasePose> for Pose {
    fn from(v: engine_core::BasePose) -> Self {
        Self {
            position: v.position,
            rotation: v.rotation,
            linear_velocity: v.linear_velocity,
            angular_velocity: v.angular_velocity,
        }
    }
}
impl From<Pose> for engine_core::BasePose {
    fn from(v: Pose) -> Self {
        Self {
            position: v.position,
            rotation: v.rotation,
            linear_velocity: v.linear_velocity,
            angular_velocity: v.angular_velocity,
        }
    }
}
engine_net::schema! {pub(super) struct Platform(SchemaId(0x110b),1,Representation::PublicPresentation) {
1=>id:u64=1=>d(Units::Unitless,U64),
2=>collider:ColliderKey=ColliderKey::defaults()=>d(Units::Unitless,Range::None),
3=>scene_revision:u64=1=>d(Units::Unitless,U64),
4=>motion_revision:u32=1=>d(Units::Unitless,U32),
5=>origin_gameplay_tick:u32=0=>d(Units::Ticks,U32),
6=>gameplay_tick:u32=0=>d(Units::Ticks,U32),
7=>phase:u16=0=>d(Units::Ticks,U16),
8=>pose:Pose=Pose::defaults()=>d(Units::Unitless,Range::None),
9=>half_extents:[f32;3]=[3.0,0.125,1.5]=>d(Units::Metres,NONNEG),
}}
impl Root for Platform {
    type Model = PublicPlatformView;
    fn capture(v: &Self::Model) -> Result<Self, SchemaError> {
        if !v.valid() {
            return Err(SchemaError::InvalidEncoding);
        }
        Ok(Self {
            id: v.id,
            collider: v.collider.into(),
            scene_revision: v.scene_revision,
            motion_revision: v.motion_revision,
            origin_gameplay_tick: v.origin_gameplay_tick,
            gameplay_tick: v.gameplay_tick,
            phase: v.phase,
            pose: v.pose.into(),
            half_extents: v.half_extents,
        })
    }
    fn restore(&self) -> Result<Self::Model, SchemaError> {
        let v = PublicPlatformView {
            id: self.id,
            collider: self.collider.clone().into(),
            scene_revision: self.scene_revision,
            motion_revision: self.motion_revision,
            origin_gameplay_tick: self.origin_gameplay_tick,
            gameplay_tick: self.gameplay_tick,
            phase: self.phase,
            pose: self.pose.clone().into(),
            half_extents: self.half_extents,
        };
        if !v.valid() {
            return Err(SchemaError::InvalidEncoding);
        }
        Ok(v)
    }
}
