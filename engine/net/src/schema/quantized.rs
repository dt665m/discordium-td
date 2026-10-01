use super::*;

/// An `f32` carried as a whole number of `1 / SCALE` steps.
///
/// Construction rounds once, so the sender's baseline and the receiver's copy
/// are the same number and a delta is an exact integer difference. Compact
/// encoding writes the step count as a zigzag variable-length integer, and a
/// delta writes the change in steps, which for motion sampled at a fixed rate
/// is a byte or two. Power-of-two scales convert back to `f32` exactly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Quantized<const SCALE: u32> {
    steps: i32,
}
impl<const SCALE: u32> Quantized<SCALE> {
    pub const ZERO: Self = Self { steps: 0 };
    pub fn new(value: f32) -> Result<Self, SchemaError> {
        if !value.is_finite() {
            return Err(SchemaError::NonFinite);
        }
        let steps = (f64::from(value) * f64::from(SCALE)).round();
        if SCALE == 0 || steps < f64::from(i32::MIN) || steps > f64::from(i32::MAX) {
            return Err(SchemaError::OutOfRange);
        }
        Ok(Self {
            steps: steps as i32,
        })
    }
    pub fn get(self) -> f32 {
        (f64::from(self.steps) / f64::from(SCALE)) as f32
    }
    pub fn steps(self) -> i32 {
        self.steps
    }
    /// Round each element; for arrays of coordinates.
    pub fn array<const N: usize>(values: [f32; N]) -> Result<[Self; N], SchemaError> {
        let mut out = [Self::ZERO; N];
        for (slot, value) in out.iter_mut().zip(values) {
            *slot = Self::new(value)?;
        }
        Ok(out)
    }
    pub fn values<const N: usize>(values: [Self; N]) -> [f32; N] {
        values.map(Self::get)
    }
}

fn write_varint(out: &mut Vec<u8>, work: &Work, mut value: u64) -> Result<(), SchemaError> {
    let mut bytes = [0u8; 10];
    let mut len = 0;
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            bytes[len] = byte;
            len += 1;
            break;
        }
        bytes[len] = byte | 0x80;
        len += 1;
    }
    work.append(out, &bytes[..len])
}
fn read_varint(reader: &mut Reader<'_>) -> Result<u64, SchemaError> {
    let mut value = 0u64;
    for index in 0..10 {
        let byte = reader.take(1)?[0];
        let bits = u64::from(byte & 0x7f);
        // Reject overlong and overflowing encodings so each number has one form.
        if index == 9 && byte > 1 {
            return Err(SchemaError::InvalidEncoding);
        }
        value |= bits << (7 * index);
        if byte & 0x80 == 0 {
            if index > 0 && byte == 0 {
                return Err(SchemaError::InvalidEncoding);
            }
            return Ok(value);
        }
    }
    Err(SchemaError::InvalidEncoding)
}
fn zigzag(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}
fn unzigzag(value: u64) -> i64 {
    ((value >> 1) as i64) ^ -((value & 1) as i64)
}
fn steps_from(value: i64) -> Result<i32, SchemaError> {
    i32::try_from(value).map_err(|_| SchemaError::OutOfRange)
}

impl<const SCALE: u32> FieldValue for Quantized<SCALE> {
    fn inspect(spec: &FieldSpec, i: &mut Inspection, _: usize) -> Result<(), SchemaError> {
        let Range::Float32 { minimum, maximum } = spec.range else {
            return Err(SchemaError::InvalidRange);
        };
        if SCALE == 0 || !minimum.is_finite() || !maximum.is_finite() || minimum > maximum {
            return Err(SchemaError::InvalidRange);
        }
        if spec.interpolation == Interpolation::AngleRadians && spec.units != Units::Radians {
            return Err(SchemaError::InvalidInterpolation);
        }
        i.tag(21);
        i.number(u64::from(SCALE));
        Ok(())
    }
    fn maximum_size(w: &mut Work, d: usize) -> Result<usize, SchemaError> {
        w.node(d)?;
        Ok(4)
    }
    fn value_size(&self, s: &FieldSpec, w: &mut Work, d: usize) -> Result<usize, SchemaError> {
        w.node(d)?;
        let Range::Float32 { minimum, maximum } = s.range else {
            return Err(SchemaError::InvalidRange);
        };
        let value = self.get();
        if value < minimum || value > maximum {
            return Err(SchemaError::OutOfRange);
        }
        Ok(4)
    }
    fn write(&self, out: &mut Vec<u8>, w: &mut Work, d: usize) -> Result<(), SchemaError> {
        w.node(d)?;
        w.append(out, &self.steps.to_le_bytes())
    }
    fn read(bytes: &[u8], w: &mut Work, d: usize) -> Result<Self, SchemaError> {
        w.node(d)?;
        let steps = i32::from_le_bytes(bytes.try_into().map_err(|_| SchemaError::InvalidEncoding)?);
        Ok(Self { steps })
    }
}
impl<const SCALE: u32> CompactValue for Quantized<SCALE> {
    fn write_compact(
        &self,
        out: &mut Vec<u8>,
        work: &mut Work,
        depth: usize,
    ) -> Result<(), SchemaError> {
        work.node(depth)?;
        write_varint(out, work, zigzag(i64::from(self.steps)))
    }
    fn read_compact(
        reader: &mut Reader<'_>,
        work: &mut Work,
        depth: usize,
    ) -> Result<Self, SchemaError> {
        work.node(depth)?;
        Ok(Self {
            steps: steps_from(unzigzag(read_varint(reader)?))?,
        })
    }
    fn write_compact_delta(
        &self,
        base: &Self,
        out: &mut Vec<u8>,
        work: &mut Work,
        depth: usize,
    ) -> Result<(), SchemaError> {
        work.node(depth)?;
        write_varint(
            out,
            work,
            zigzag(i64::from(self.steps) - i64::from(base.steps)),
        )
    }
    fn read_compact_delta(
        base: &Self,
        reader: &mut Reader<'_>,
        work: &mut Work,
        depth: usize,
    ) -> Result<Self, SchemaError> {
        work.node(depth)?;
        let change = unzigzag(read_varint(reader)?);
        Ok(Self {
            steps: steps_from(
                i64::from(base.steps)
                    .checked_add(change)
                    .ok_or(SchemaError::OutOfRange)?,
            )?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_have_one_encoding() {
        let work = Work::new(SchemaLimits::default());
        for value in [0, 1, 127, 128, 300, u64::from(u32::MAX), u64::MAX] {
            let mut out = Vec::new();
            write_varint(&mut out, &work, value).unwrap();
            let mut reader = Reader::new(&out);
            assert_eq!(read_varint(&mut reader).unwrap(), value);
            reader.finish().unwrap();
        }
        // Overlong zero and an eleventh byte are both rejected.
        assert!(read_varint(&mut Reader::new(&[0x80, 0x00])).is_err());
        assert!(read_varint(&mut Reader::new(&[0xff; 11])).is_err());
        for value in [0, -1, 1, i64::from(i32::MIN), i64::from(i32::MAX)] {
            assert_eq!(unzigzag(zigzag(value)), value);
        }
    }

    #[test]
    fn rounding_happens_once_and_round_trips_exactly() {
        let value = Quantized::<1024>::new(12.345_678).unwrap();
        assert_eq!(value.steps(), 12_642);
        assert_eq!(Quantized::<1024>::new(value.get()).unwrap(), value);
        assert!(Quantized::<1024>::new(f32::NAN).is_err());
        assert!(Quantized::<1024>::new(3.0e7).is_err());
    }
}
