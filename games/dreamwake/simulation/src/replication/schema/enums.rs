use super::*;
macro_rules! mapped_enum {
    ($wire:ident,$model:path,{$($id:literal=>$variant:ident),+$(,)?}) => {
        engine_net::schema_enum! {pub(super) enum $wire {$($id=>$variant),+}}
        impl From<$model> for $wire {fn from(value:$model)->Self {match value {$(<$model>::$variant=>Self::$variant),+}}}
        impl From<$wire> for $model {fn from(value:$wire)->Self {match value {$($wire::$variant=>Self::$variant),+}}}
    }
}
mapped_enum!(Phase,crate::RunPhase,{1=>Intro,2=>Combat,3=>Reward,4=>Rest,5=>Transition,6=>Victory,7=>Defeat});
mapped_enum!(EnemyKind,crate::EnemyKind,{1=>Melee,2=>Ranged,3=>Ambusher,4=>Support,5=>Elite,6=>Boss});
mapped_enum!(MemoryKind,crate::MemoryKind,{1=>Crescent,2=>Starfall,3=>Nova,4=>Blink,5=>Aegis,6=>Wisp});
mapped_enum!(Essence,crate::EssenceKind,{1=>Twin,2=>Echo,3=>Vast,4=>Frost,5=>Leech,6=>Haste});
mapped_enum!(Upgrade,crate::UpgradeKind,{1=>Attack,2=>Ability,3=>Movement,4=>Critical,5=>Recovery,6=>Health,7=>Defense});
mapped_enum!(Rarity,crate::Rarity,{1=>Common,2=>Rare,3=>Epic});
mapped_enum!(Stance,engine_core::Stance,{1=>Standing,2=>Crouched});
engine_net::schema! {pub(super) struct BeamGraphic(SchemaId(0x1003),1,Representation::PublicPresentation) {
    1=>direction:[f32;3]=[1.0,0.0,0.0]=>p(Units::Unitless,Range::Float32{minimum:-1.0,maximum:1.0}),
    2=>elevation:f32=0.0=>p(Units::Metres,Range::Float32{minimum:-100000.0,maximum:100000.0}),
    3=>length:f32=1.0=>p(Units::Metres,Range::Float32{minimum:0.0,maximum:crate::state::DREAMLANCE_RANGE}),
}}
engine_net::schema! {pub(super) struct OrbGraphic(SchemaId(0x1006),1,Representation::PublicPresentation) {
    1=>elevation:f32=0.0=>p(Units::Metres,Range::Float32{minimum:-100000.0,maximum:100000.0}),
}}
engine_net::schema_enum! {pub(super) enum GraphicKind {1=>RadialPulse(()),2=>Beam(BeamGraphic),3=>Orb(OrbGraphic)}}
impl From<engine_core::GraphicsKind> for GraphicKind {
    fn from(v: engine_core::GraphicsKind) -> Self {
        match v {
            engine_core::GraphicsKind::RadialPulse => Self::RadialPulse(()),
            engine_core::GraphicsKind::Orb { elevation } => Self::Orb(OrbGraphic { elevation }),
            engine_core::GraphicsKind::Beam {
                direction,
                elevation,
                length,
            } => Self::Beam(BeamGraphic {
                direction,
                elevation,
                length,
            }),
        }
    }
}
impl From<GraphicKind> for engine_core::GraphicsKind {
    fn from(v: GraphicKind) -> Self {
        match v {
            GraphicKind::RadialPulse(()) => Self::RadialPulse,
            GraphicKind::Orb(v) => Self::Orb {
                elevation: v.elevation,
            },
            GraphicKind::Beam(v) => Self::Beam {
                direction: v.direction,
                elevation: v.elevation,
                length: v.length,
            },
        }
    }
}
