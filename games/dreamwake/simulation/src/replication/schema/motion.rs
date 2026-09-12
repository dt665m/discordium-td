use super::*;
engine_net::schema! {pub(super) struct ColliderKey(SchemaId(0x1002),1,Representation::PublicPresentation) {
    1=>index:u64=1=>d(Units::Unitless,U64),
    2=>generation:u32=1=>d(Units::Unitless,U32),
}}
impl From<engine_core::ColliderKey> for ColliderKey {
    fn from(v: engine_core::ColliderKey) -> Self {
        Self {
            index: v.index,
            generation: v.generation,
        }
    }
}
impl From<ColliderKey> for engine_core::ColliderKey {
    fn from(v: ColliderKey) -> Self {
        Self {
            index: v.index,
            generation: v.generation,
        }
    }
}
engine_net::schema! {pub(super) struct Ground(SchemaId(0x1207),1,Representation::OwnerCheckpoint) {
    1=>collider:ColliderKey=ColliderKey::defaults()=>o(Units::Unitless,Range::None),
    2=>scene_revision:u64=1=>o(Units::Unitless,U64),
    3=>normal:[f32;3]=[0.0,1.0,0.0]=>o(Units::Unitless,F),
    4=>local_position:[f32;3]=[0.0;3]=>o(Units::Metres,F),
}}
impl From<engine_core::GroundContact> for Ground {
    fn from(v: engine_core::GroundContact) -> Self {
        Self {
            collider: v.collider.into(),
            scene_revision: v.scene_revision,
            normal: v.normal,
            local_position: v.local_position,
        }
    }
}
impl From<Ground> for engine_core::GroundContact {
    fn from(v: Ground) -> Self {
        Self {
            collider: v.collider.into(),
            scene_revision: v.scene_revision,
            normal: v.normal,
            local_position: v.local_position,
        }
    }
}
engine_net::schema! {pub(super) struct Attachment(SchemaId(0x1208),1,Representation::OwnerCheckpoint) {
    1=>collider:ColliderKey=ColliderKey::defaults()=>o(Units::Unitless,Range::None),
    2=>scene_revision:u64=1=>o(Units::Unitless,U64),
    3=>pose_tick:u64=0=>o(Units::Ticks,U64),
    4=>local_anchor:[f32;3]=[0.0;3]=>o(Units::Metres,F),
}}
impl From<engine_core::BaseAttachment> for Attachment {
    fn from(v: engine_core::BaseAttachment) -> Self {
        Self {
            collider: v.collider.into(),
            scene_revision: v.scene_revision,
            pose_tick: v.pose_tick,
            local_anchor: v.local_anchor,
        }
    }
}
impl From<Attachment> for engine_core::BaseAttachment {
    fn from(v: Attachment) -> Self {
        Self {
            collider: v.collider.into(),
            scene_revision: v.scene_revision,
            pose_tick: v.pose_tick,
            local_anchor: v.local_anchor,
        }
    }
}
engine_net::schema! {pub(super) struct Base(SchemaId(0x1209),1,Representation::OwnerCheckpoint) {
    1=>revision:u64=0=>o(Units::Unitless,U64),
    2=>attachment:Option<Attachment>=None=>o(Units::Unitless,Range::None),
}}
impl From<engine_core::BaseState> for Base {
    fn from(v: engine_core::BaseState) -> Self {
        Self {
            revision: v.revision,
            attachment: v.attachment.map(Into::into),
        }
    }
}
impl From<Base> for engine_core::BaseState {
    fn from(v: Base) -> Self {
        Self {
            revision: v.revision,
            attachment: v.attachment.map(Into::into),
        }
    }
}
engine_net::schema! {pub(super) struct Motion(SchemaId(0x120a),1,Representation::OwnerCheckpoint) {
    1=>position:[f32;3]=[0.0,0.0,2.0]=>o(Units::Metres,F),
    2=>velocity:[f32;3]=[0.0;3]=>o(Units::Custom("metres/second"),F),
    3=>facing:[f32;2]=[0.0,-1.0]=>o(Units::Unitless,F),
    4=>stance:Stance=Stance::Standing=>o(Units::Unitless,Range::None),
    5=>grounded:bool=false=>o(Units::Unitless,Range::None),
    6=>ground:Option<Ground>=None=>o(Units::Unitless,Range::None),
    7=>scene_revision:u64=1=>o(Units::Unitless,U64),
    8=>jump_buffer_ticks:u16=0=>o(Units::Ticks,U16),
    9=>coyote_ticks:u16=0=>o(Units::Ticks,U16),
    10=>dash_ticks:u16=0=>o(Units::Ticks,U16),
    11=>dash_cooldown_ticks:u16=0=>o(Units::Ticks,U16),
    12=>dash_direction:[f32;2]=[0.0,-1.0]=>o(Units::Unitless,F),
    13=>movement_lock_ticks:u16=0=>o(Units::Ticks,U16),
    14=>suppress_snap_ticks:u16=0=>o(Units::Ticks,U16),
    15=>base:Base=Base::defaults()=>o(Units::Unitless,Range::None),
}}
impl From<&engine_core::KinematicState> for Motion {
    fn from(v: &engine_core::KinematicState) -> Self {
        Self {
            position: v.position,
            velocity: v.velocity,
            facing: v.facing,
            stance: v.stance.into(),
            grounded: v.grounded,
            ground: v.ground.map(Into::into),
            scene_revision: v.scene_revision,
            jump_buffer_ticks: v.jump_buffer_ticks,
            coyote_ticks: v.coyote_ticks,
            dash_ticks: v.dash_ticks,
            dash_cooldown_ticks: v.dash_cooldown_ticks,
            dash_direction: v.dash_direction,
            movement_lock_ticks: v.movement_lock_ticks,
            suppress_snap_ticks: v.suppress_snap_ticks,
            base: v.base.into(),
        }
    }
}
impl From<Motion> for engine_core::KinematicState {
    fn from(v: Motion) -> Self {
        Self {
            position: v.position,
            velocity: v.velocity,
            facing: v.facing,
            stance: v.stance.into(),
            grounded: v.grounded,
            ground: v.ground.map(Into::into),
            scene_revision: v.scene_revision,
            jump_buffer_ticks: v.jump_buffer_ticks,
            coyote_ticks: v.coyote_ticks,
            dash_ticks: v.dash_ticks,
            dash_cooldown_ticks: v.dash_cooldown_ticks,
            dash_direction: v.dash_direction,
            movement_lock_ticks: v.movement_lock_ticks,
            suppress_snap_ticks: v.suppress_snap_ticks,
            base: v.base.into(),
        }
    }
}
