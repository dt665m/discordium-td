use super::owner::RayKey;
use super::*;
use engine_net::codec::BoundedVec;
engine_net::schema! {pub(super) struct Flight(SchemaId(0x1220),1,Representation::OwnerCheckpoint) {
1=>key:RayKey=RayKey::defaults()=>o(Units::Unitless,Range::None),
2=>ordinal:u8=0=>o(Units::Unitless,U8),
3=>origin_tick:u32=0=>o(Units::Ticks,U32),
4=>position:[f32;2]=[0.0;2]=>o(Units::Metres,F),
6=>direction:[f32;2]=[1.0,0.0]=>o(Units::Unitless,F),
7=>radius:f32=0.34=>o(Units::Metres,NONNEG),
8=>remaining:f32=2.0=>o(Units::Seconds,NONNEG),
9=>essence:Option<Essence>=None=>o(Units::Unitless,Range::None),
10=>authority_id:Option<u64>=None=>o(Units::Unitless,U64),
}}
impl From<&crate::starfall::StarfallFlight> for Flight {
    fn from(v: &crate::starfall::StarfallFlight) -> Self {
        Self {
            key: v.key.action.into(),
            ordinal: v.key.ordinal,
            origin_tick: v.origin_tick,
            position: v.position,
            direction: v.direction,
            radius: v.radius,
            remaining: v.remaining,
            essence: v.essence.map(Into::into),
            authority_id: v.authority_id,
        }
    }
}
impl From<Flight> for crate::starfall::StarfallFlight {
    fn from(v: Flight) -> Self {
        Self {
            key: crate::starfall::SpawnKey {
                action: v.key.into(),
                ordinal: v.ordinal,
            },
            origin_tick: v.origin_tick,
            position: v.position,
            direction: v.direction,
            radius: v.radius,
            remaining: v.remaining,
            essence: v.essence.map(Into::into),
            authority_id: v.authority_id,
        }
    }
}
engine_net::schema! {pub(super) struct Repeat(SchemaId(0x1221),1,Representation::OwnerCheckpoint) {
1=>key:RayKey=RayKey::defaults()=>o(Units::Unitless,Range::None),
2=>ordinal:u8=3=>o(Units::Unitless,U8),
3=>origin_tick:u32=0=>o(Units::Ticks,U32),
4=>remaining:f32=0.55=>o(Units::Seconds,NONNEG),
5=>origin:[f32;2]=[0.0;2]=>o(Units::Metres,F),
6=>direction:[f32;2]=[1.0,0.0]=>o(Units::Unitless,F),
7=>authority_id:Option<u64>=None=>o(Units::Unitless,U64),
}}
impl From<&crate::starfall::StarfallRepeat> for Repeat {
    fn from(v: &crate::starfall::StarfallRepeat) -> Self {
        Self {
            authority_id: v.authority_id,
            key: v.key.action.into(),
            ordinal: v.key.ordinal,
            origin_tick: v.origin_tick,
            remaining: v.remaining,
            origin: v.origin,
            direction: v.direction,
        }
    }
}
impl From<Repeat> for crate::starfall::StarfallRepeat {
    fn from(v: Repeat) -> Self {
        Self {
            authority_id: v.authority_id,
            key: crate::starfall::SpawnKey {
                action: v.key.into(),
                ordinal: v.ordinal,
            },
            origin_tick: v.origin_tick,
            remaining: v.remaining,
            origin: v.origin,
            direction: v.direction,
        }
    }
}
engine_net::schema_enum! {pub(super) enum Entry {1=>Flight(Flight),2=>Repeat(Repeat)}}
engine_net::schema! {pub(super) struct Domain(SchemaId(0x1222),1,Representation::OwnerCheckpoint) {
1=>entries:BoundedVec<Entry,8>=BoundedVec::default()=>o(Units::Unitless,Range::None),
}}
impl TryFrom<&crate::starfall::StarfallDomain> for Domain {
    type Error = SchemaError;
    fn try_from(v: &crate::starfall::StarfallDomain) -> Result<Self, Self::Error> {
        Ok(Self {
            entries: BoundedVec::new(
                v.flights
                    .iter()
                    .map(|f| Entry::Flight(f.into()))
                    .chain(v.repeats.iter().map(|r| Entry::Repeat(r.into())))
                    .collect(),
            )
            .map_err(invalid)?,
        })
    }
}
impl From<Domain> for crate::starfall::StarfallDomain {
    fn from(v: Domain) -> Self {
        let mut result = Self::default();
        for entry in v.entries.into_vec() {
            match entry {
                Entry::Flight(f) => result.flights.push(f.into()),
                Entry::Repeat(r) => result.repeats.push(r.into()),
            }
        }
        result
    }
}
