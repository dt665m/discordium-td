use super::*;
use crate::replication::PublicCoverMarkerView;
use engine_net::types::*;

engine_net::schema! {pub(super) struct CoverMarker(SchemaId(0x110c),1,Representation::PublicPresentation) {
    1=>id:u64=1=>p(Units::Unitless,U64),
    2=>connection:u64=1=>p(Units::Unitless,U64),
    3=>parent_index:u64=1=>p(Units::Unitless,U64),
    4=>parent_generation:u32=1=>p(Units::Unitless,U32),
    5=>parent_scope:u64=1=>p(Units::Unitless,U64),
    6=>parent_representation:u32=1=>p(Units::Unitless,U32),
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
            connection: v.parent.connection.0,
            parent_index: v.parent.entity.index,
            parent_generation: v.parent.entity.generation,
            parent_scope: v.parent.scope.0,
            parent_representation: v.parent.representation.0,
            revision: v.revision,
            local_offset: v.local_offset,
            open: v.open,
        })
    }
    fn restore(&self) -> Result<Self::Model, SchemaError> {
        let value = PublicCoverMarkerView {
            id: self.id,
            parent: ScopeIdentity {
                connection: ConnectionEpoch(self.connection),
                entity: EntityId {
                    index: self.parent_index,
                    generation: self.parent_generation,
                },
                scope: ScopeEpoch(self.parent_scope),
                representation: RepresentationRevision(self.parent_representation),
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
