use super::*;
const MAGIC: &[u8; 4] = b"NSC1";
const ENVELOPE: usize = 36;
/// Generated-code work accounting; every provided value implementation consumes
/// this before allocating sequences or traversing untrusted children.
#[doc(hidden)]
#[derive(Clone)]
pub struct Work {
    pub limits: SchemaLimits,
    nodes: usize,
    allocated: usize,
    items: usize,
    text: usize,
}
impl Work {
    pub fn new(limits: SchemaLimits) -> Self {
        Self {
            limits,
            nodes: 0,
            allocated: 0,
            items: 0,
            text: 0,
        }
    }
    pub fn node(&mut self, depth: usize) -> Result<(), SchemaError> {
        if depth > self.limits.nesting {
            return Err(SchemaError::NestingBudget);
        }
        self.nodes = self.nodes.checked_add(1).ok_or(SchemaError::ValueBudget)?;
        if self.nodes > self.limits.value_nodes {
            return Err(SchemaError::ValueBudget);
        }
        Ok(())
    }
    pub fn count(&mut self, count: usize) -> Result<(), SchemaError> {
        self.items = self
            .items
            .checked_add(count)
            .ok_or(SchemaError::CountBudget)?;
        if count > self.limits.collection || self.items > self.limits.collection_items {
            Err(SchemaError::CountBudget)
        } else {
            Ok(())
        }
    }
    pub fn text(&mut self, bytes: usize) -> Result<(), SchemaError> {
        self.text = self
            .text
            .checked_add(bytes)
            .ok_or(SchemaError::TextBudget)?;
        if self.text > self.limits.text_bytes {
            Err(SchemaError::TextBudget)
        } else {
            Ok(())
        }
    }
    /// Merge mutually exclusive value alternatives by the largest resource
    /// use in each dimension; this does not sum variants that cannot coexist.
    pub fn merge_max(&mut self, other: &Self) {
        self.nodes = self.nodes.max(other.nodes);
        self.allocated = self.allocated.max(other.allocated);
        self.items = self.items.max(other.items);
        self.text = self.text.max(other.text);
    }
    pub fn size(&self, size: usize) -> Result<usize, SchemaError> {
        if size > self.limits.wire_bytes {
            Err(SchemaError::WireBudget)
        } else {
            Ok(size)
        }
    }
    pub fn reserve<T>(&mut self, count: usize) -> Result<(), SchemaError> {
        self.allocate(
            count
                .checked_mul(std::mem::size_of::<T>())
                .ok_or(SchemaError::AllocationBudget)?,
        )
    }
    pub fn allocate(&mut self, bytes: usize) -> Result<(), SchemaError> {
        self.allocated = self
            .allocated
            .checked_add(bytes)
            .ok_or(SchemaError::AllocationBudget)?;
        if self.allocated > self.limits.allocation_bytes {
            return Err(SchemaError::AllocationBudget);
        }
        Ok(())
    }
    pub fn checkpoint(&self) -> (usize, usize, usize, usize) {
        (self.nodes, self.allocated, self.items, self.text)
    }
    pub fn repeat_since(
        &mut self,
        before: (usize, usize, usize, usize),
        extra: usize,
    ) -> Result<(), SchemaError> {
        self.nodes = self
            .nodes
            .checked_add(
                (self.nodes - before.0)
                    .checked_mul(extra)
                    .ok_or(SchemaError::ValueBudget)?,
            )
            .ok_or(SchemaError::ValueBudget)?;
        if self.nodes > self.limits.value_nodes {
            return Err(SchemaError::ValueBudget);
        }
        self.allocate(
            (self.allocated - before.1)
                .checked_mul(extra)
                .ok_or(SchemaError::AllocationBudget)?,
        )?;
        self.items = self
            .items
            .checked_add(
                (self.items - before.2)
                    .checked_mul(extra)
                    .ok_or(SchemaError::CountBudget)?,
            )
            .ok_or(SchemaError::CountBudget)?;
        if self.items > self.limits.collection_items {
            return Err(SchemaError::CountBudget);
        }
        self.text(
            (self.text - before.3)
                .checked_mul(extra)
                .ok_or(SchemaError::TextBudget)?,
        )
    }
    pub fn append(&self, out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), SchemaError> {
        self.size(
            out.len()
                .checked_add(bytes.len())
                .ok_or(SchemaError::WireBudget)?,
        )?;
        out.extend_from_slice(bytes);
        Ok(())
    }
}
#[doc(hidden)]
pub struct Reader<'a> {
    bytes: &'a [u8],
    previous: FieldId,
}
impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            previous: FieldId(0),
        }
    }
    pub fn remaining(&self) -> usize {
        self.bytes.len()
    }
    pub fn take(&mut self, size: usize) -> Result<&'a [u8], SchemaError> {
        if size > self.bytes.len() {
            return Err(SchemaError::Truncated);
        }
        let (value, next) = self.bytes.split_at(size);
        self.bytes = next;
        Ok(value)
    }
    pub fn u16(&mut self) -> Result<u16, SchemaError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32, SchemaError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn field(&mut self) -> Result<(FieldId, &'a [u8]), SchemaError> {
        let id = FieldId(self.u16()?);
        if id.0 == 0 || id <= self.previous {
            return Err(SchemaError::NonCanonicalOrder);
        }
        self.previous = id;
        let size = self.u32()? as usize;
        Ok((id, self.take(size)?))
    }
    pub fn finish(&self) -> Result<(), SchemaError> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err(SchemaError::TrailingBytes)
        }
    }
}
/// A fixed stack array bounds ordering work independently of input lengths.
#[doc(hidden)]
pub fn ordered(
    fields: &[FieldDescriptor],
    limit: usize,
) -> Result<([FieldId; 256], usize), SchemaError> {
    if fields.is_empty() || fields.len() > limit || fields.len() > 256 {
        return Err(SchemaError::CountBudget);
    }
    let mut ids = [FieldId(0); 256];
    for (id, field) in ids.iter_mut().zip(fields) {
        if field.id.0 == 0 {
            return Err(SchemaError::InvalidField(field.id));
        }
        *id = field.id;
    }
    ids[..fields.len()].sort_unstable();
    if let Some(pair) = ids[..fields.len()].windows(2).find(|p| p[0] == p[1]) {
        return Err(SchemaError::DuplicateField(pair[0]));
    }
    Ok((ids, fields.len()))
}
#[doc(hidden)]
pub fn record_maximum<S: Schema>(work: &mut Work, depth: usize) -> Result<usize, SchemaError> {
    work.node(depth)?;
    ordered(S::FIELDS, work.limits.fields)?;
    let fields = S::maximum_fields(work, depth)?;
    work.size(10 + fields)
}
#[doc(hidden)]
pub fn record_size<S: Schema>(
    value: &S,
    work: &mut Work,
    depth: usize,
) -> Result<usize, SchemaError> {
    work.node(depth)?;
    ordered(S::FIELDS, work.limits.fields)?;
    let fields = value.fields_size(work, depth)?;
    work.size(10 + fields)
}
#[doc(hidden)]
pub fn write_record<S: Schema>(
    value: &S,
    out: &mut Vec<u8>,
    work: &mut Work,
    depth: usize,
) -> Result<(), SchemaError> {
    work.node(depth)?;
    work.append(out, &S::ID.0.to_le_bytes())?;
    work.append(out, &S::VERSION.to_le_bytes())?;
    work.append(out, &(S::FIELDS.len() as u16).to_le_bytes())?;
    value.write_fields(out, work, depth)
}
#[doc(hidden)]
pub fn write_field<T: FieldValue>(
    id: FieldId,
    value: &T,
    out: &mut Vec<u8>,
    work: &mut Work,
    depth: usize,
) -> Result<(), SchemaError> {
    work.append(out, &id.0.to_le_bytes())?;
    let start = out.len();
    work.append(out, &[0; 4])?;
    value.write(out, work, depth + 1)?;
    let length = u32::try_from(out.len() - start - 4).map_err(|_| SchemaError::WireBudget)?;
    out[start..start + 4].copy_from_slice(&length.to_le_bytes());
    Ok(())
}
#[doc(hidden)]
pub fn read_record<S: Schema>(
    bytes: &[u8],
    work: &mut Work,
    depth: usize,
) -> Result<S, SchemaError> {
    work.node(depth)?;
    work.size(bytes.len())?;
    let mut reader = Reader::new(bytes);
    if reader.u32()? != S::ID.0 || reader.u32()? != S::VERSION {
        return Err(SchemaError::IncompatibleSchema);
    }
    let count = reader.u16()? as usize;
    if count > work.limits.fields || count > S::FIELDS.len() {
        return Err(SchemaError::CountBudget);
    }
    if count.checked_mul(6).is_none_or(|n| n > reader.remaining()) {
        return Err(SchemaError::Truncated);
    }
    let value = S::read_fields(&mut reader, work, depth, count)?;
    reader.finish()?;
    Ok(value)
}
#[doc(hidden)]
pub fn raw_encode<S: Schema>(value: &S, limits: SchemaLimits) -> Result<Vec<u8>, SchemaError> {
    let size = record_size(value, &mut Work::new(limits), 0)?;
    let mut bytes = Vec::with_capacity(size);
    write_record(value, &mut bytes, &mut Work::new(limits), 0)?;
    Ok(bytes)
}
/// A codec is produced only after registration checks the complete declaration.
/// All methods create staging values; none receives a mutable gameplay world.
pub struct SchemaCodec<S: Schema> {
    pub(crate) identity: SchemaIdentity,
    pub(crate) limits: SchemaLimits,
    pub(crate) marker: std::marker::PhantomData<S>,
}
impl<S: Schema> SchemaCodec<S> {
    pub fn identity(&self) -> SchemaIdentity {
        self.identity
    }
    pub fn encode(&self, value: &S) -> Result<Vec<u8>, SchemaError> {
        let size = record_size(value, &mut Work::new(self.limits), 0)?
            .checked_add(ENVELOPE)
            .ok_or(SchemaError::WireBudget)?;
        if size > self.limits.wire_bytes {
            return Err(SchemaError::WireBudget);
        }
        let mut bytes = Vec::with_capacity(size);
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&self.identity.fingerprint);
        write_record(value, &mut bytes, &mut Work::new(self.limits), 0)?;
        Ok(bytes)
    }
    pub fn decode(&self, bytes: &[u8]) -> Result<S, SchemaError> {
        if bytes.len() > self.limits.wire_bytes {
            return Err(SchemaError::WireBudget);
        }
        let mut reader = Reader::new(bytes);
        if reader.take(4)? != MAGIC || reader.take(32)? != self.identity.fingerprint {
            return Err(SchemaError::IncompatibleSchema);
        }
        let value = read_record::<S>(
            reader.take(reader.remaining())?,
            &mut Work::new(self.limits),
            0,
        )?;
        record_size(&value, &mut Work::new(self.limits), 0)?;
        Ok(value)
    }
    pub fn default_value(&self) -> Result<S, SchemaError> {
        self.decode(&self.encode(&S::defaults())?)
    }
    pub fn changes(
        &self,
        before: &S,
        after: &S,
        access: DiagnosticAccess<'_>,
    ) -> Result<Vec<FieldChange>, SchemaError> {
        record_size(before, &mut Work::new(self.limits), 0)?;
        record_size(after, &mut Work::new(self.limits), 0)?;
        let mut work = Work::new(self.limits);
        work.reserve::<FieldChange>(S::FIELDS.len())?;
        let mut changes = Vec::with_capacity(S::FIELDS.len());
        S::diagnose(before, after, &access, &mut changes, &mut work)?;
        Ok(changes)
    }
}
#[doc(hidden)]
pub fn diagnostic_field<T: FieldValue>(
    field: &FieldDescriptor,
    before: &T,
    after: &T,
    access: &DiagnosticAccess<'_>,
    out: &mut Vec<FieldChange>,
    work: &mut Work,
) -> Result<(), SchemaError> {
    if before == after && field.spec.change == ChangeRule::CanonicalEquality {
        return Ok(());
    }
    let (old, new) = if access.permits(field) {
        let a = before.value_size(&field.spec, &mut Work::new(work.limits), 0)?;
        let b = after.value_size(&field.spec, &mut Work::new(work.limits), 0)?;
        work.allocate(a + b)?;
        let mut old = Vec::with_capacity(a);
        let mut new = Vec::with_capacity(b);
        before.write(&mut old, &mut Work::new(work.limits), 0)?;
        after.write(&mut new, &mut Work::new(work.limits), 0)?;
        (Some(old), Some(new))
    } else {
        (None, None)
    };
    out.push(FieldChange {
        field: field.id,
        before: old,
        after: new,
    });
    Ok(())
}
