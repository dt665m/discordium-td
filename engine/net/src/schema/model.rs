use crate::types::SchemaId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FieldId(pub u16);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Representation {
    PublicPresentation,
    OwnerCheckpoint,
    OfflineCheckpoint,
}
impl Representation {
    pub(crate) fn visibility(self) -> Visibility {
        match self {
            Self::PublicPresentation => Visibility::Public,
            Self::OwnerCheckpoint => Visibility::Owner,
            Self::OfflineCheckpoint => Visibility::ServerOnly,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Visibility {
    Public,
    Owner,
    ServerOnly,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Authority {
    Server,
    ClientIntent,
    Derived,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Prediction {
    None,
    Dependency,
    Replayed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    None,
    Step,
    Linear,
    AngleRadians,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeRule {
    CanonicalEquality,
    Always,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quantizer {
    /// Finite IEEE values, preserving precision and normalizing signed zero.
    ExactCanonical,
    /// Reserved declaration: registration rejects this until its codec exists.
    Lossy { policy: u32, version: u32 },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Units {
    Unitless,
    Metres,
    Seconds,
    Ticks,
    Radians,
    Custom(&'static str),
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Range {
    None,
    Unsigned { minimum: u64, maximum: u64 },
    Signed { minimum: i64, maximum: i64 },
    Float32 { minimum: f32, maximum: f32 },
    Float64 { minimum: f64, maximum: f64 },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Compatibility {
    pub introduced: u32,
    /// The initial strict codec requires all fields. Optional evolution needs a
    /// separately implemented safe-default negotiation policy before use.
    pub required: bool,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldSpec {
    pub units: Units,
    pub range: Range,
    pub quantizer: Quantizer,
    pub authority: Authority,
    pub visibility: Visibility,
    pub prediction: Prediction,
    pub interpolation: Interpolation,
    pub change: ChangeRule,
    pub compatibility: Compatibility,
}
impl FieldSpec {
    pub const fn exact(
        units: Units,
        range: Range,
        visibility: Visibility,
        prediction: Prediction,
    ) -> Self {
        Self {
            units,
            range,
            quantizer: Quantizer::ExactCanonical,
            authority: Authority::Server,
            visibility,
            prediction,
            interpolation: Interpolation::None,
            change: ChangeRule::CanonicalEquality,
            compatibility: Compatibility {
                introduced: 1,
                required: true,
            },
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct FieldDescriptor {
    pub id: FieldId,
    /// Diagnostics only: renaming a Rust field does not alter its wire identity.
    pub name: &'static str,
    pub spec: FieldSpec,
}
#[derive(Clone, Copy, Debug)]
pub struct SchemaLimits {
    pub schemas: usize,
    pub fields: usize,
    pub nesting: usize,
    pub collection: usize,
    /// Aggregate entries across all collections in one value.
    pub collection_items: usize,
    /// Aggregate UTF-8 payload bytes across all strings in one value.
    pub text_bytes: usize,
    pub value_nodes: usize,
    pub wire_bytes: usize,
    /// Cumulative owned sequence allocations in one decode, including staging.
    pub allocation_bytes: usize,
}
impl Default for SchemaLimits {
    fn default() -> Self {
        Self {
            schemas: 128,
            fields: 128,
            nesting: 8,
            collection: 256,
            collection_items: 8192,
            text_bytes: 8192,
            value_nodes: 8192,
            wire_bytes: 65535,
            allocation_bytes: 2 * 1024 * 1024,
        }
    }
}
impl SchemaLimits {
    pub(crate) fn validate(self) -> Result<Self, SchemaError> {
        if !(1..=256).contains(&self.schemas)
            || !(1..=256).contains(&self.fields)
            || !(1..=16).contains(&self.nesting)
            || !(1..=4096).contains(&self.collection)
            || !(1..=65536).contains(&self.collection_items)
            || !(1..=65535).contains(&self.text_bytes)
            || !(1..=65536).contains(&self.value_nodes)
            || !(48..=65535).contains(&self.wire_bytes)
            || self.allocation_bytes < self.wire_bytes
            || self.allocation_bytes > 8 * 1024 * 1024
        {
            return Err(SchemaError::InvalidLimits);
        }
        Ok(self)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchemaIdentity {
    pub schema: SchemaId,
    pub version: u32,
    pub fingerprint: [u8; 32],
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaError {
    InvalidLimits,
    InvalidSchema,
    DuplicateSchema,
    DuplicateField(FieldId),
    InvalidField(FieldId),
    InvalidRange,
    InvalidDefault,
    OutOfRange,
    NonFinite,
    UnsupportedQuantizer,
    UnsupportedCompatibility,
    VisibilityDowngrade,
    PredictionDowngrade,
    InvalidInterpolation,
    MissingRestore,
    MissingDependencyDeclaration,
    DependencyContextRequired,
    InvalidLifecycleDeclaration,
    RestoreMismatch,
    NoRestore,
    InvalidDependency,
    IncompatibleSchema,
    UnknownField(FieldId),
    MissingField(FieldId),
    NonCanonicalOrder,
    Truncated,
    TrailingBytes,
    WireBudget,
    AllocationBudget,
    CountBudget,
    NestingBudget,
    ValueBudget,
    TextBudget,
    InvalidEncoding,
}
impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for SchemaError {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldChange {
    pub field: FieldId,
    pub before: Option<Vec<u8>>,
    pub after: Option<Vec<u8>>,
}
/// The default exposes identities only. The explicit callback must represent
/// the caller's diagnostic disclosure authorization, not an interest shortcut.
pub enum DiagnosticAccess<'a> {
    FieldIdsOnly,
    Authorized(&'a dyn Fn(&FieldDescriptor) -> bool),
}
impl DiagnosticAccess<'_> {
    pub(crate) fn permits(&self, field: &FieldDescriptor) -> bool {
        match self {
            Self::FieldIdsOnly => false,
            Self::Authorized(allow) => allow(field),
        }
    }
}
