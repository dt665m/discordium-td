use super::*;
#[doc(hidden)]
pub struct Inspection {
    pub limits: SchemaLimits,
    hash: blake3::Hasher,
    nodes: usize,
    pub(crate) restore_required: bool,
    pub(crate) known: Vec<SchemaIdentity>,
    root: [u8; 32],
}
impl Inspection {
    pub(crate) fn new(limits: SchemaLimits) -> Self {
        let mut hash = blake3::Hasher::new();
        hash.update(b"engine_net/schema/exact-canonical/v1");
        Self {
            limits,
            hash,
            nodes: 0,
            restore_required: false,
            known: Vec::new(),
            root: [0; 32],
        }
    }
    pub fn tag(&mut self, tag: u8) {
        self.hash.update(&[tag]);
    }
    pub fn number(&mut self, value: u64) {
        self.hash.update(&value.to_le_bytes());
    }
    pub fn count(&self, value: usize) -> Result<(), SchemaError> {
        if value > self.limits.collection {
            Err(SchemaError::CountBudget)
        } else {
            Ok(())
        }
    }
    fn text(&mut self, value: &str) -> Result<(), SchemaError> {
        if value.len() > 128 {
            return Err(SchemaError::InvalidSchema);
        }
        self.number(value.len() as u64);
        self.hash.update(value.as_bytes());
        Ok(())
    }
    pub fn schema<S: Schema>(
        &mut self,
        depth: usize,
        visibility: Visibility,
        prediction: Prediction,
    ) -> Result<(), SchemaError> {
        if depth > self.limits.nesting {
            return Err(SchemaError::NestingBudget);
        }
        if S::ID.0 == 0 || S::VERSION == 0 {
            return Err(SchemaError::InvalidSchema);
        }
        if S::REPRESENTATION.visibility() > visibility {
            return Err(SchemaError::VisibilityDowngrade);
        }
        let maximum = record_maximum::<S>(&mut Work::new(self.limits), depth)?;
        if maximum
            .checked_add(36)
            .is_none_or(|n| n > self.limits.wire_bytes)
        {
            return Err(SchemaError::WireBudget);
        }
        let parent = std::mem::replace(&mut self.hash, blake3::Hasher::new());
        self.hash.update(b"engine_net/schema/exact-canonical/v1");
        self.tag(20);
        self.number(u64::from(S::ID.0));
        self.number(u64::from(S::VERSION));
        self.tag(S::REPRESENTATION as u8);
        let (_, count) = ordered(S::FIELDS, self.limits.fields)?;
        self.number(count as u64);
        S::inspect_fields(self, depth, S::REPRESENTATION.visibility(), prediction)?;
        // Defaults are typed, validated and encoded without reflection. Integer
        // defaults/ranges/identities never pass through floating point.
        let defaults =
            raw_encode(&S::defaults(), self.limits).map_err(|_| SchemaError::InvalidDefault)?;
        self.number(defaults.len() as u64);
        self.hash.update(&defaults);
        let identity = SchemaIdentity {
            schema: S::ID,
            version: S::VERSION,
            fingerprint: *self.hash.finalize().as_bytes(),
        };
        if let Some(existing) = self.known.iter().find(|entry| entry.schema == S::ID) {
            if *existing != identity {
                return Err(SchemaError::DuplicateSchema);
            }
        } else {
            if self.known.len() >= self.limits.schemas {
                return Err(SchemaError::CountBudget);
            }
            self.known.push(identity);
        }
        self.hash = parent;
        self.hash.update(&identity.fingerprint);
        if depth == 0 {
            self.root = identity.fingerprint;
        }
        Ok(())
    }
    pub fn field<T: FieldValue>(
        &mut self,
        field: &FieldDescriptor,
        depth: usize,
        visibility: Visibility,
        prediction: Prediction,
        version: u32,
    ) -> Result<(), SchemaError> {
        self.nodes = self.nodes.checked_add(1).ok_or(SchemaError::ValueBudget)?;
        if self.nodes > self.limits.value_nodes {
            return Err(SchemaError::ValueBudget);
        }
        let s = &field.spec;
        if field.id.0 == 0 || field.name.is_empty() || field.name.len() > 128 {
            return Err(SchemaError::InvalidField(field.id));
        }
        if s.quantizer != Quantizer::ExactCanonical {
            return Err(SchemaError::UnsupportedQuantizer);
        }
        if !s.compatibility.required {
            return Err(SchemaError::UnsupportedCompatibility);
        }
        if s.compatibility.introduced == 0 || s.compatibility.introduced > version {
            return Err(SchemaError::InvalidField(field.id));
        }
        if s.visibility > visibility {
            return Err(SchemaError::VisibilityDowngrade);
        }
        if s.prediction > prediction {
            return Err(SchemaError::PredictionDowngrade);
        }
        if s.prediction == Prediction::Replayed {
            if s.authority != Authority::Server || s.interpolation != Interpolation::None {
                return Err(SchemaError::InvalidInterpolation);
            }
            if visibility == Visibility::Public {
                return Err(SchemaError::PredictionDowngrade);
            }
            self.restore_required = true;
        }
        self.number(u64::from(field.id.0));
        match s.units {
            Units::Custom(text) => {
                self.tag(5);
                self.text(text)?;
            }
            unit => self.tag(match unit {
                Units::Unitless => 0,
                Units::Metres => 1,
                Units::Seconds => 2,
                Units::Ticks => 3,
                Units::Radians => 4,
                _ => unreachable!(),
            }),
        }
        match s.range {
            Range::None => self.tag(0),
            Range::Unsigned { minimum, maximum } => {
                self.tag(1);
                self.number(minimum);
                self.number(maximum);
            }
            Range::Signed { minimum, maximum } => {
                self.tag(2);
                self.hash.update(&minimum.to_le_bytes());
                self.hash.update(&maximum.to_le_bytes());
            }
            Range::Float32 { minimum, maximum } => {
                self.tag(3);
                self.hash.update(
                    &(if minimum == 0.0 { 0.0 } else { minimum })
                        .to_bits()
                        .to_le_bytes(),
                );
                self.hash.update(
                    &(if maximum == 0.0 { 0.0 } else { maximum })
                        .to_bits()
                        .to_le_bytes(),
                );
            }
            Range::Float64 { minimum, maximum } => {
                self.tag(4);
                self.hash.update(
                    &(if minimum == 0.0 { 0.0 } else { minimum })
                        .to_bits()
                        .to_le_bytes(),
                );
                self.hash.update(
                    &(if maximum == 0.0 { 0.0 } else { maximum })
                        .to_bits()
                        .to_le_bytes(),
                );
            }
        }
        self.tag(0); // ExactCanonical policy, including finite values and +0.
        self.tag(s.authority as u8);
        self.tag(s.visibility as u8);
        self.tag(s.prediction as u8);
        self.tag(s.interpolation as u8);
        self.tag(s.change as u8);
        self.number(u64::from(s.compatibility.introduced));
        self.tag(u8::from(s.compatibility.required));
        T::inspect(s, self, depth + 1)
    }
    pub(crate) fn fingerprint(&self) -> [u8; 32] {
        self.root
    }
}
