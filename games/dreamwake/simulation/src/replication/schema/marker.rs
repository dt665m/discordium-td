use super::*;
use crate::replication::PublicCoverMarkerView;
use engine_net::types::*;

engine_net::schema! {pub(super) struct CoverMarker(SchemaId(0x110c),2,Representation::PublicPresentation) {
    1=>id:u64=1=>p(Units::Unitless,U64),
    3=>parent_index:u64=1=>p(Units::Unitless,U64),
    4=>parent_generation:u32=1=>p(Units::Unitless,U32),
    7=>revision:u64=1=>p(Units::Unitless,U64),
    8=>local_offset:[f32;3]=[0.0,1.35,0.0]=>p(Units::Metres,F),
    9=>open:bool=false=>p(Units::Unitless,Range::None),
}}
impl Root for CoverMarker {
    type Model = PublicCoverMarkerView;
    fn capture(v: &Self::Model) -> Result<Self, SchemaError> {
        if !v.valid() {
            return Err(SchemaError::InvalidEncoding);
        }
        Ok(Self {
            id: v.id,
            parent_index: v.parent.index,
            parent_generation: v.parent.generation,
            revision: v.revision,
            local_offset: v.local_offset,
            open: v.open,
        })
    }
    fn restore(&self) -> Result<Self::Model, SchemaError> {
        let value = PublicCoverMarkerView {
            id: self.id,
            parent: EntityId {
                index: self.parent_index,
                generation: self.parent_generation,
            },
            revision: self.revision,
            local_offset: self.local_offset,
            open: self.open,
        };
        if !value.valid() {
            return Err(SchemaError::InvalidEncoding);
        }
        Ok(value)
    }
}
