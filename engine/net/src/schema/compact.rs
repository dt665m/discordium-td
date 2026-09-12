use super::*;
use crate::codec::BoundedVec;

/// Exact values for an already negotiated schema. Unlike the canonical codec,
/// this stream carries no root fingerprint, field IDs or field lengths. The
/// enclosing protocol must reject mismatched protocol and schema identities
/// before using it. Stable field IDs still determine the generated wire order.
pub trait CompactValue: FieldValue {
    #[doc(hidden)]
    fn write_compact(
        &self,
        out: &mut Vec<u8>,
        work: &mut Work,
        depth: usize,
    ) -> Result<(), SchemaError>;
    #[doc(hidden)]
    fn read_compact(
        reader: &mut Reader<'_>,
        work: &mut Work,
        depth: usize,
    ) -> Result<Self, SchemaError>;
}

impl<S: Schema + CompactValue> SchemaCodec<S> {
    /// Encode a registered value after the enclosing session has negotiated the
    /// exact schema identity. Canonical inspection/offline encoding is separate.
    pub fn encode_compact(&self, value: &S) -> Result<Vec<u8>, SchemaError> {
        // The canonical size is a conservative capacity and retains every field
        // policy and traversal limit already checked by the registered codec.
        let size = record_size(value, &mut Work::new(self.limits), 0)?;
        let mut bytes = Vec::with_capacity(size);
        value.write_compact(&mut bytes, &mut Work::new(self.limits), 0)?;
        Ok(bytes)
    }

    /// Stage and validate a value under the session's exact negotiated schema.
    /// Callers must validate protocol/schema compatibility before this method;
    /// the compact body deliberately cannot negotiate its own interpretation.
    pub fn decode_compact(&self, bytes: &[u8]) -> Result<S, SchemaError> {
        if bytes.len() > self.limits.wire_bytes {
            return Err(SchemaError::WireBudget);
        }
        let mut reader = Reader::new(bytes);
        let value = S::read_compact(&mut reader, &mut Work::new(self.limits), 0)?;
        reader.finish()?;
        record_size(&value, &mut Work::new(self.limits), 0)?;
        Ok(value)
    }
}

macro_rules! scalar {
    ($($ty:ty),+ $(,)?) => {$(
        impl CompactValue for $ty {
            fn write_compact(&self, out: &mut Vec<u8>, work: &mut Work, depth: usize) -> Result<(), SchemaError> {
                self.write(out, work, depth)
            }
            fn read_compact(reader: &mut Reader<'_>, work: &mut Work, depth: usize) -> Result<Self, SchemaError> {
                Self::read(reader.take(std::mem::size_of::<Self>())?, work, depth)
            }
        }
    )+};
}
scalar!(u8, u16, u32, u64, i8, i16, i32, i64, bool, f32, f64);

impl<T: CompactValue, const N: usize> CompactValue for [T; N] {
    fn write_compact(
        &self,
        out: &mut Vec<u8>,
        work: &mut Work,
        depth: usize,
    ) -> Result<(), SchemaError> {
        work.node(depth)?;
        work.count(N)?;
        for value in self {
            value.write_compact(out, work, depth + 1)?;
        }
        Ok(())
    }
    fn read_compact(
        reader: &mut Reader<'_>,
        work: &mut Work,
        depth: usize,
    ) -> Result<Self, SchemaError> {
        work.node(depth)?;
        work.count(N)?;
        work.reserve::<T>(N)?;
        let mut values = Vec::with_capacity(N);
        for _ in 0..N {
            values.push(T::read_compact(reader, work, depth + 1)?);
        }
        values.try_into().map_err(|_| SchemaError::CountBudget)
    }
}

impl<T: CompactValue, const N: usize> CompactValue for BoundedVec<T, N> {
    fn write_compact(
        &self,
        out: &mut Vec<u8>,
        work: &mut Work,
        depth: usize,
    ) -> Result<(), SchemaError> {
        work.node(depth)?;
        work.count(self.as_slice().len())?;
        let count = u32::try_from(self.as_slice().len()).map_err(|_| SchemaError::CountBudget)?;
        work.append(out, &count.to_le_bytes())?;
        for value in self.as_slice() {
            value.write_compact(out, work, depth + 1)?;
        }
        Ok(())
    }
    fn read_compact(
        reader: &mut Reader<'_>,
        work: &mut Work,
        depth: usize,
    ) -> Result<Self, SchemaError> {
        work.node(depth)?;
        let count = reader.u32()? as usize;
        if count > N {
            return Err(SchemaError::CountBudget);
        }
        work.count(count)?;
        work.reserve::<T>(count)?;
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            values.push(T::read_compact(reader, work, depth + 1)?);
        }
        Self::new(values).map_err(|_| SchemaError::CountBudget)
    }
}

impl<const N: usize> CompactValue for BoundedString<N> {
    fn write_compact(
        &self,
        out: &mut Vec<u8>,
        work: &mut Work,
        depth: usize,
    ) -> Result<(), SchemaError> {
        self.write(out, work, depth)
    }
    fn read_compact(
        reader: &mut Reader<'_>,
        work: &mut Work,
        depth: usize,
    ) -> Result<Self, SchemaError> {
        work.node(depth)?;
        let count = reader.u32()? as usize;
        if count > N {
            return Err(SchemaError::TextBudget);
        }
        work.text(count)?;
        let text =
            std::str::from_utf8(reader.take(count)?).map_err(|_| SchemaError::InvalidEncoding)?;
        work.allocate(count)?;
        Self::new(text)
    }
}

impl<T: CompactValue> CompactValue for Option<T> {
    fn write_compact(
        &self,
        out: &mut Vec<u8>,
        work: &mut Work,
        depth: usize,
    ) -> Result<(), SchemaError> {
        work.node(depth)?;
        work.append(out, &[u8::from(self.is_some())])?;
        if let Some(value) = self {
            value.write_compact(out, work, depth + 1)?;
        }
        Ok(())
    }
    fn read_compact(
        reader: &mut Reader<'_>,
        work: &mut Work,
        depth: usize,
    ) -> Result<Self, SchemaError> {
        work.node(depth)?;
        match reader.take(1)?[0] {
            0 => Ok(None),
            1 => T::read_compact(reader, work, depth + 1).map(Some),
            _ => Err(SchemaError::InvalidEncoding),
        }
    }
}

impl CompactValue for () {
    fn write_compact(
        &self,
        _: &mut Vec<u8>,
        work: &mut Work,
        depth: usize,
    ) -> Result<(), SchemaError> {
        work.node(depth)
    }
    fn read_compact(
        _: &mut Reader<'_>,
        work: &mut Work,
        depth: usize,
    ) -> Result<Self, SchemaError> {
        work.node(depth)
    }
}
