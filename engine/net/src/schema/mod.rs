//! Typed exact canonical schemas with bounded generated field codecs.
//!
//! `schema!` declares stable IDs, typed defaults and field policies. Registration
//! validates the entire nested declaration and requires restoration for replayed
//! fields. Strict fingerprints reject unknown/evolved layouts; lossy and optional
//! field evolution remain unsupported. Capture, dependency and lifecycle hooks
//! are game declarations, not proof of their runtime completeness or authority.
//! Generated decode stages owned values and never mutates a gameplay world.
mod compact;
#[cfg(test)]
mod compact_tests;
mod composites;
mod inspection;
mod macros;
mod model;
mod registry;
#[cfg(test)]
mod tests;
mod value;
mod wire;
use crate::types::SchemaId;
pub use compact::CompactValue;
pub use composites::BoundedString;
#[doc(hidden)]
pub use composites::enum_ids;
#[doc(hidden)]
pub use inspection::Inspection;
pub use model::*;
pub use registry::*;
pub use value::FieldValue;
pub use wire::SchemaCodec;
#[doc(hidden)]
pub use wire::{
    Reader, Work, diagnostic_field, ordered, raw_encode, read_record, record_maximum, record_size,
    write_field, write_record,
};

/// Implemented by `schema!`. Custom implementations are trusted codecs and must
/// honor the same bounds; ordinary serde reflection is not registration.
pub trait Schema: Clone + PartialEq + Sized + 'static {
    const ID: SchemaId;
    const VERSION: u32;
    const REPRESENTATION: Representation;
    const FIELDS: &'static [FieldDescriptor];
    fn defaults() -> Self;
    #[doc(hidden)]
    fn inspect_fields(
        i: &mut Inspection,
        depth: usize,
        visibility: Visibility,
        prediction: Prediction,
    ) -> Result<(), SchemaError>;
    #[doc(hidden)]
    fn maximum_fields(work: &mut Work, depth: usize) -> Result<usize, SchemaError>;
    #[doc(hidden)]
    fn fields_size(&self, work: &mut Work, depth: usize) -> Result<usize, SchemaError>;
    #[doc(hidden)]
    fn write_fields(
        &self,
        out: &mut Vec<u8>,
        work: &mut Work,
        depth: usize,
    ) -> Result<(), SchemaError>;
    #[doc(hidden)]
    fn read_fields(
        reader: &mut Reader<'_>,
        work: &mut Work,
        depth: usize,
        count: usize,
    ) -> Result<Self, SchemaError>;
    #[doc(hidden)]
    fn diagnose(
        before: &Self,
        after: &Self,
        access: &DiagnosticAccess<'_>,
        out: &mut Vec<FieldChange>,
        work: &mut Work,
    ) -> Result<(), SchemaError>;
}
