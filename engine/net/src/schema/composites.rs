use super::*;

/// UTF-8 with a byte bound in its type. No unbounded `String` gains FieldValue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundedString<const N: usize>(String);
impl<const N: usize> BoundedString<N> {
    pub fn new(value: impl AsRef<str>) -> Result<Self, SchemaError> {
        let text = value.as_ref();
        if text.len() > N {
            return Err(SchemaError::TextBudget);
        }
        Ok(Self(text.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn into_string(self) -> String {
        self.0
    }
}
impl<const N: usize> Default for BoundedString<N> {
    fn default() -> Self {
        Self(String::new())
    }
}
impl<const N: usize> FieldValue for BoundedString<N> {
    fn inspect(s: &FieldSpec, i: &mut Inspection, _: usize) -> Result<(), SchemaError> {
        if s.range != Range::None {
            return Err(SchemaError::InvalidRange);
        }
        if !matches!(s.interpolation, Interpolation::None | Interpolation::Step) {
            return Err(SchemaError::InvalidInterpolation);
        }
        if N > i.limits.text_bytes {
            return Err(SchemaError::TextBudget);
        }
        i.tag(15);
        i.number(N as u64);
        Ok(())
    }
    fn maximum_size(w: &mut Work, d: usize) -> Result<usize, SchemaError> {
        w.node(d)?;
        w.text(N)?;
        w.allocate(N)?;
        w.size(4 + N)
    }
    fn value_size(&self, s: &FieldSpec, w: &mut Work, d: usize) -> Result<usize, SchemaError> {
        if s.range != Range::None {
            return Err(SchemaError::InvalidRange);
        }
        w.node(d)?;
        w.text(self.0.len())?;
        w.size(4 + self.0.len())
    }
    fn write(&self, out: &mut Vec<u8>, w: &mut Work, d: usize) -> Result<(), SchemaError> {
        w.node(d)?;
        w.text(self.0.len())?;
        w.append(out, &(self.0.len() as u32).to_le_bytes())?;
        w.append(out, self.0.as_bytes())
    }
    fn read(bytes: &[u8], w: &mut Work, d: usize) -> Result<Self, SchemaError> {
        w.node(d)?;
        let mut reader = Reader::new(bytes);
        let count = reader.u32()? as usize;
        if count > N {
            return Err(SchemaError::TextBudget);
        }
        w.text(count)?;
        let text =
            std::str::from_utf8(reader.take(count)?).map_err(|_| SchemaError::InvalidEncoding)?;
        reader.finish()?;
        w.allocate(count)?;
        Ok(Self(text.to_owned()))
    }
}
impl<T: FieldValue> FieldValue for Option<T> {
    fn inspect(s: &FieldSpec, i: &mut Inspection, d: usize) -> Result<(), SchemaError> {
        i.tag(14);
        T::inspect(s, i, d + 1)
    }
    fn maximum_size(w: &mut Work, d: usize) -> Result<usize, SchemaError> {
        w.node(d)?;
        let size = T::maximum_size(w, d + 1)?;
        w.size(1 + size)
    }
    fn value_size(&self, s: &FieldSpec, w: &mut Work, d: usize) -> Result<usize, SchemaError> {
        w.node(d)?;
        let size = if let Some(value) = self {
            value.value_size(s, w, d + 1)?
        } else {
            0
        };
        w.size(1 + size)
    }
    fn write(&self, out: &mut Vec<u8>, w: &mut Work, d: usize) -> Result<(), SchemaError> {
        w.node(d)?;
        w.append(out, &[u8::from(self.is_some())])?;
        if let Some(value) = self {
            value.write(out, w, d + 1)?;
        }
        Ok(())
    }
    fn read(bytes: &[u8], w: &mut Work, d: usize) -> Result<Self, SchemaError> {
        w.node(d)?;
        let (&tag, body) = bytes.split_first().ok_or(SchemaError::Truncated)?;
        match tag {
            0 if body.is_empty() => Ok(None),
            1 => T::read(body, w, d + 1).map(Some),
            _ => Err(SchemaError::InvalidEncoding),
        }
    }
}
#[doc(hidden)]
pub fn enum_ids(ids: &[u16]) -> Result<([u16; 256], usize), SchemaError> {
    if ids.is_empty() || ids.len() > 256 {
        return Err(SchemaError::CountBudget);
    }
    let mut sorted = [0; 256];
    sorted[..ids.len()].copy_from_slice(ids);
    sorted[..ids.len()].sort_unstable();
    if sorted[0] == 0 || sorted[..ids.len()].windows(2).any(|p| p[0] == p[1]) {
        return Err(SchemaError::InvalidSchema);
    }
    Ok((sorted, ids.len()))
}
/// Empty variant payload for tagged sums that mix unit and data variants.
impl FieldValue for () {
    fn inspect(spec: &FieldSpec, inspection: &mut Inspection, _: usize) -> Result<(), SchemaError> {
        if spec.range != Range::None {
            return Err(SchemaError::InvalidRange);
        }
        if !matches!(
            spec.interpolation,
            Interpolation::None | Interpolation::Step
        ) {
            return Err(SchemaError::InvalidInterpolation);
        }
        inspection.tag(18);
        Ok(())
    }
    fn maximum_size(work: &mut Work, depth: usize) -> Result<usize, SchemaError> {
        work.node(depth)?;
        Ok(0)
    }
    fn value_size(
        &self,
        _: &FieldSpec,
        work: &mut Work,
        depth: usize,
    ) -> Result<usize, SchemaError> {
        work.node(depth)?;
        Ok(0)
    }
    fn write(&self, _: &mut Vec<u8>, work: &mut Work, depth: usize) -> Result<(), SchemaError> {
        work.node(depth)
    }
    fn read(bytes: &[u8], work: &mut Work, depth: usize) -> Result<Self, SchemaError> {
        work.node(depth)?;
        if bytes.is_empty() {
            Ok(())
        } else {
            Err(SchemaError::InvalidEncoding)
        }
    }
}
