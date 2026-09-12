use super::*;
use engine_net::codec::BoundedVec;
engine_net::schema! {pub(super) struct Config(SchemaId(0x1300),1,Representation::PublicPresentation) {
    1=>fixed_dt:f32=crate::collision::movement_profile().fixed_dt=>d(Units::Seconds,NONNEG),
    2=>radius:f32=crate::collision::movement_profile().radius=>d(Units::Metres,NONNEG),
    3=>standing_height:f32=crate::collision::movement_profile().standing_height=>d(Units::Metres,NONNEG),
    4=>crouched_height:f32=crate::collision::movement_profile().crouched_height=>d(Units::Metres,NONNEG),
    5=>speed:f32=crate::collision::movement_profile().speed=>d(Units::Custom("metres/second"),NONNEG),
    6=>crouched_speed:f32=crate::collision::movement_profile().crouched_speed=>d(Units::Custom("metres/second"),NONNEG),
    7=>acceleration:f32=crate::collision::movement_profile().acceleration=>d(Units::Custom("metres/second^2"),NONNEG),
    8=>braking:f32=crate::collision::movement_profile().braking=>d(Units::Custom("metres/second^2"),NONNEG),
    9=>air_acceleration:f32=crate::collision::movement_profile().air_acceleration=>d(Units::Custom("metres/second^2"),NONNEG),
    10=>gravity:f32=crate::collision::movement_profile().gravity=>d(Units::Custom("metres/second^2"),NONNEG),
    11=>jump_speed:f32=crate::collision::movement_profile().jump_speed=>d(Units::Custom("metres/second"),NONNEG),
    12=>terminal_speed:f32=crate::collision::movement_profile().terminal_speed=>d(Units::Custom("metres/second"),NONNEG),
    13=>dash_speed:f32=crate::collision::movement_profile().dash_speed=>d(Units::Custom("metres/second"),NONNEG),
    14=>dash_duration_ticks:u16=crate::collision::movement_profile().dash_duration_ticks=>d(Units::Ticks,U16),
    15=>dash_cooldown_ticks:u16=crate::collision::movement_profile().dash_cooldown_ticks=>d(Units::Ticks,U16),
    16=>jump_buffer_ticks:u16=crate::collision::movement_profile().jump_buffer_ticks=>d(Units::Ticks,U16),
    17=>coyote_ticks:u16=crate::collision::movement_profile().coyote_ticks=>d(Units::Ticks,U16),
    18=>skin:f32=crate::collision::movement_profile().skin=>d(Units::Metres,NONNEG),
    19=>ground_snap:f32=crate::collision::movement_profile().ground_snap=>d(Units::Metres,NONNEG),
    20=>max_step_height:f32=crate::collision::movement_profile().max_step_height=>d(Units::Metres,NONNEG),
    21=>walkable_normal_y:f32=crate::collision::movement_profile().walkable_normal_y=>d(Units::Unitless,NONNEG),
    22=>max_slide_iterations:u8=crate::collision::movement_profile().max_slide_iterations=>d(Units::Unitless,U8),
    23=>max_depenetration_iterations:u8=crate::collision::movement_profile().max_depenetration_iterations=>d(Units::Unitless,U8),
    24=>max_queries:u32=crate::collision::movement_profile().max_queries=>d(Units::Unitless,U32),
    25=>max_depenetration_distance:f32=crate::collision::movement_profile().max_depenetration_distance=>d(Units::Metres,NONNEG),
}}
impl From<&engine_core::KinematicConfig> for Config {
    fn from(v: &engine_core::KinematicConfig) -> Self {
        Self {
            fixed_dt: v.fixed_dt,
            radius: v.radius,
            standing_height: v.standing_height,
            crouched_height: v.crouched_height,
            speed: v.speed,
            crouched_speed: v.crouched_speed,
            acceleration: v.acceleration,
            braking: v.braking,
            air_acceleration: v.air_acceleration,
            gravity: v.gravity,
            jump_speed: v.jump_speed,
            terminal_speed: v.terminal_speed,
            dash_speed: v.dash_speed,
            dash_duration_ticks: v.dash_duration_ticks,
            dash_cooldown_ticks: v.dash_cooldown_ticks,
            jump_buffer_ticks: v.jump_buffer_ticks,
            coyote_ticks: v.coyote_ticks,
            skin: v.skin,
            ground_snap: v.ground_snap,
            max_step_height: v.max_step_height,
            walkable_normal_y: v.walkable_normal_y,
            max_slide_iterations: v.max_slide_iterations,
            max_depenetration_iterations: v.max_depenetration_iterations,
            max_queries: v.max_queries,
            max_depenetration_distance: v.max_depenetration_distance,
        }
    }
}
impl From<Config> for engine_core::KinematicConfig {
    fn from(v: Config) -> Self {
        Self {
            fixed_dt: v.fixed_dt,
            radius: v.radius,
            standing_height: v.standing_height,
            crouched_height: v.crouched_height,
            speed: v.speed,
            crouched_speed: v.crouched_speed,
            acceleration: v.acceleration,
            braking: v.braking,
            air_acceleration: v.air_acceleration,
            gravity: v.gravity,
            jump_speed: v.jump_speed,
            terminal_speed: v.terminal_speed,
            dash_speed: v.dash_speed,
            dash_duration_ticks: v.dash_duration_ticks,
            dash_cooldown_ticks: v.dash_cooldown_ticks,
            jump_buffer_ticks: v.jump_buffer_ticks,
            coyote_ticks: v.coyote_ticks,
            skin: v.skin,
            ground_snap: v.ground_snap,
            max_step_height: v.max_step_height,
            walkable_normal_y: v.walkable_normal_y,
            max_slide_iterations: v.max_slide_iterations,
            max_depenetration_iterations: v.max_depenetration_iterations,
            max_queries: v.max_queries,
            max_depenetration_distance: v.max_depenetration_distance,
        }
    }
}
engine_net::schema! {pub(super) struct BoxShape(SchemaId(0x1301),1,Representation::PublicPresentation) {
    1=>half_extents:[f32;3]=[1.0;3]=>d(Units::Metres,NONNEG),
}}
engine_net::schema! {pub(super) struct BallShape(SchemaId(0x1302),1,Representation::PublicPresentation) {
    1=>radius:f32=1.0=>d(Units::Metres,NONNEG),
}}
engine_net::schema! {pub(super) struct CapsuleShape(SchemaId(0x1303),1,Representation::PublicPresentation) {
    1=>half_segment:f32=0.0=>d(Units::Metres,NONNEG),
    2=>radius:f32=1.0=>d(Units::Metres,NONNEG),
}}
engine_net::schema_enum! {pub(super) enum Shape {1=>Box(BoxShape),2=>Ball(BallShape),3=>Capsule(CapsuleShape)}}
impl From<engine_core::CollisionShape> for Shape {
    fn from(v: engine_core::CollisionShape) -> Self {
        match v {
            engine_core::CollisionShape::Box { half_extents } => {
                Self::Box(BoxShape { half_extents })
            }
            engine_core::CollisionShape::Ball { radius } => Self::Ball(BallShape { radius }),
            engine_core::CollisionShape::Capsule {
                half_segment,
                radius,
            } => Self::Capsule(CapsuleShape {
                half_segment,
                radius,
            }),
        }
    }
}
impl From<Shape> for engine_core::CollisionShape {
    fn from(v: Shape) -> Self {
        match v {
            Shape::Box(v) => Self::Box {
                half_extents: v.half_extents,
            },
            Shape::Ball(v) => Self::Ball { radius: v.radius },
            Shape::Capsule(v) => Self::Capsule {
                half_segment: v.half_segment,
                radius: v.radius,
            },
        }
    }
}
engine_net::schema! {pub(super) struct Collider(SchemaId(0x1304),1,Representation::PublicPresentation) {
    1=>key:ColliderKey=ColliderKey::defaults()=>d(Units::Unitless,Range::None),
    2=>position:[f32;3]=[0.0;3]=>d(Units::Metres,F),
    3=>rotation:[f32;4]=[0.0,0.0,0.0,1.0]=>d(Units::Unitless,F),
    4=>shape:Shape=Shape::Box(BoxShape::defaults())=>d(Units::Unitless,Range::None),
}}
impl From<&engine_core::StaticCollider> for Collider {
    fn from(v: &engine_core::StaticCollider) -> Self {
        Self {
            key: v.key.into(),
            position: v.position,
            rotation: v.rotation,
            shape: v.shape.into(),
        }
    }
}
impl From<Collider> for engine_core::StaticCollider {
    fn from(v: Collider) -> Self {
        Self {
            key: v.key.into(),
            position: v.position,
            rotation: v.rotation,
            shape: v.shape.into(),
        }
    }
}
engine_net::schema! {pub(super) struct Collision(SchemaId(0x1103),1,Representation::PublicPresentation) {
    1=>schema_version:u32=1=>d(Units::Unitless,U32),
    2=>scene_revision:u64=1=>d(Units::Unitless,U64),
    3=>config:Config=Config::defaults()=>d(Units::Unitless,Range::None),
    4=>colliders:BoundedVec<Collider,64>=BoundedVec::new(crate::collision::CollisionManifest::current(1).colliders().iter().map(Into::into).collect()).unwrap()=>d(Units::Unitless,Range::None),
}}
impl Root for Collision {
    type Model = crate::collision::CollisionManifest;
    fn capture(v: &Self::Model) -> Result<Self, SchemaError> {
        v.build().map_err(invalid)?;
        Ok(Self {
            schema_version: v.schema_version(),
            scene_revision: v.scene_revision(),
            config: (&v.config()).into(),
            colliders: BoundedVec::new(v.colliders().iter().map(Into::into).collect())
                .map_err(invalid)?,
        })
    }
    fn restore(&self) -> Result<Self::Model, SchemaError> {
        if self.schema_version != 1 {
            return Err(SchemaError::IncompatibleSchema);
        }
        crate::collision::CollisionManifest::from_parts(
            self.scene_revision,
            self.config.clone().into(),
            self.colliders
                .as_slice()
                .iter()
                .cloned()
                .map(Into::into)
                .collect(),
        )
        .map_err(invalid)
    }
}
