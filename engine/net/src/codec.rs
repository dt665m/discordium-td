//! Versioned application frames and allocation-bounded collections.
//!
//! Transport adapters supply the actual application payload allowance after
//! framing and transport overhead. No plaintext size is assumed to be a path MTU.
use crate::types::ConnectionEpoch;
use bincode::Options;
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{DeserializeOwned, SeqAccess, Visitor},
};
use std::{fmt, marker::PhantomData};

pub const PROTOCOL_MAJOR: u16 = 1;
pub const PROTOCOL_MINOR: u16 = 0;
pub const HEADER_BYTES: usize = 24;
const MAGIC: [u8; 4] = *b"NRG1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    InvalidLimit,
    Oversized,
    Truncated,
    TrailingBytes,
    ProtocolMismatch,
    InvalidLane,
    InvalidFlags,
    WrongEpoch,
    InvalidPayload(String),
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for CodecError {}

/// A collection whose declared wire count is checked before reserving storage.
/// Nested payload collections must also be bounded; a byte cap is not an entity cap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct BoundedVec<T, const N: usize>(Vec<T>);

impl<T, const N: usize> Default for BoundedVec<T, N> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<T, const N: usize> BoundedVec<T, N> {
    pub fn new(values: Vec<T>) -> Result<Self, CodecError> {
        if values.len() > N {
            Err(CodecError::Oversized)
        } else {
            Ok(Self(values))
        }
    }
    pub fn as_slice(&self) -> &[T] {
        &self.0
    }
    pub fn into_vec(self) -> Vec<T> {
        self.0
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn push(&mut self, value: T) -> Result<(), CodecError> {
        if self.0.len() == N {
            return Err(CodecError::Oversized);
        }
        self.0.push(value);
        Ok(())
    }
}

impl<'de, T: Deserialize<'de>, const N: usize> Deserialize<'de> for BoundedVec<T, N> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct BoundedVisitor<T, const N: usize>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>, const N: usize> Visitor<'de> for BoundedVisitor<T, N> {
            type Value = BoundedVec<T, N>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "at most {N} elements")
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                if sequence.size_hint().is_some_and(|size| size > N) {
                    return Err(serde::de::Error::custom("collection count exceeds limit"));
                }
                let mut values = Vec::with_capacity(sequence.size_hint().unwrap_or(0).min(N));
                while let Some(value) = sequence.next_element()? {
                    if values.len() == N {
                        return Err(serde::de::Error::custom("collection count exceeds limit"));
                    }
                    values.push(value);
                }
                Ok(BoundedVec(values))
            }
        }
        deserializer.deserialize_seq(BoundedVisitor::<T, N>(PhantomData))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Lane {
    Control = 0,
    Input = 1,
    State = 2,
    Outcomes = 3,
    Diagnostics = 4,
}

impl TryFrom<u8> for Lane {
    type Error = CodecError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Control),
            1 => Ok(Self::Input),
            2 => Ok(Self::State),
            3 => Ok(Self::Outcomes),
            4 => Ok(Self::Diagnostics),
            _ => Err(CodecError::InvalidLane),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub lane: Lane,
    pub connection: ConnectionEpoch,
    pub sequence: u32,
}

/// The transport-supplied maximum application frame, including our header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameLimit(usize);

impl FrameLimit {
    pub fn new(max_application_frame_bytes: usize) -> Result<Self, CodecError> {
        if !(HEADER_BYTES..=HEADER_BYTES + u16::MAX as usize).contains(&max_application_frame_bytes)
        {
            Err(CodecError::InvalidLimit)
        } else {
            Ok(Self(max_application_frame_bytes))
        }
    }

    pub fn payload_bytes(self) -> usize {
        self.0 - HEADER_BYTES
    }
}

pub fn encode_frame(
    header: FrameHeader,
    payload: &[u8],
    limit: FrameLimit,
) -> Result<Vec<u8>, CodecError> {
    if header.connection.0 == 0 {
        return Err(CodecError::WrongEpoch);
    }
    if payload.len() > limit.payload_bytes() {
        return Err(CodecError::Oversized);
    }
    write_frame(header, 0, payload)
}

/// The payload was deflated with [`crate::wire::deflate`]; only
/// [`decode_frame_inflated`] accepts it.
const FLAG_DEFLATED: u8 = 1;

/// [`encode_frame`], deflating the payload when that makes it smaller.
/// Fixed-width fields and repeated records compress well, and a frame that
/// does not compress is sent exactly as [`encode_frame`] would send it, so
/// no worst case grows.
pub fn encode_frame_deflated(
    header: FrameHeader,
    payload: &[u8],
    limit: FrameLimit,
) -> Result<Vec<u8>, CodecError> {
    match crate::wire::deflate(payload) {
        Some(deflated) if header.connection.0 != 0 && payload.len() <= limit.payload_bytes() => {
            write_frame(header, FLAG_DEFLATED, &deflated)
        }
        _ => encode_frame(header, payload, limit),
    }
}

fn write_frame(header: FrameHeader, flags: u8, payload: &[u8]) -> Result<Vec<u8>, CodecError> {
    let length = u16::try_from(payload.len()).map_err(|_| CodecError::Oversized)?;
    let mut bytes = Vec::with_capacity(HEADER_BYTES + payload.len());
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&PROTOCOL_MAJOR.to_le_bytes());
    bytes.extend_from_slice(&PROTOCOL_MINOR.to_le_bytes());
    bytes.push(header.lane as u8);
    bytes.push(flags);
    bytes.extend_from_slice(&header.connection.0.to_le_bytes());
    bytes.extend_from_slice(&header.sequence.to_le_bytes());
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(payload);
    Ok(bytes)
}

/// Validation and slicing allocate no memory and do not mutate application state.
pub fn decode_frame(
    bytes: &[u8],
    expected: ConnectionEpoch,
    limit: FrameLimit,
) -> Result<(FrameHeader, &[u8]), CodecError> {
    let (header, flags, payload) = parse_frame(bytes, expected, limit)?;
    if flags != 0 {
        return Err(CodecError::InvalidFlags);
    }
    Ok((header, payload))
}

/// [`decode_frame`] for lanes sent with [`encode_frame_deflated`]. Inflation is
/// bounded by the frame's payload limit before any allocation.
pub fn decode_frame_inflated(
    bytes: &[u8],
    expected: ConnectionEpoch,
    limit: FrameLimit,
) -> Result<(FrameHeader, std::borrow::Cow<'_, [u8]>), CodecError> {
    let (header, flags, payload) = parse_frame(bytes, expected, limit)?;
    match flags {
        0 => Ok((header, std::borrow::Cow::Borrowed(payload))),
        FLAG_DEFLATED => crate::wire::inflate(payload, limit.payload_bytes())
            .map(|raw| (header, std::borrow::Cow::Owned(raw)))
            .map_err(|error| CodecError::InvalidPayload(error.to_string())),
        _ => Err(CodecError::InvalidFlags),
    }
}

fn parse_frame(
    bytes: &[u8],
    expected: ConnectionEpoch,
    limit: FrameLimit,
) -> Result<(FrameHeader, u8, &[u8]), CodecError> {
    if bytes.len() > limit.0 {
        return Err(CodecError::Oversized);
    }
    if bytes.len() < HEADER_BYTES {
        return Err(CodecError::Truncated);
    }
    if bytes[..4] != MAGIC
        || bytes[4..6] != PROTOCOL_MAJOR.to_le_bytes()
        || bytes[6..8] != PROTOCOL_MINOR.to_le_bytes()
    {
        return Err(CodecError::ProtocolMismatch);
    }
    let lane = Lane::try_from(bytes[8])?;
    let flags = bytes[9];
    let connection = ConnectionEpoch(u64::from_le_bytes(bytes[10..18].try_into().unwrap()));
    if expected.0 == 0 || connection != expected {
        return Err(CodecError::WrongEpoch);
    }
    let sequence = u32::from_le_bytes(bytes[18..22].try_into().unwrap());
    let length = u16::from_le_bytes(bytes[22..24].try_into().unwrap()) as usize;
    match bytes.len().cmp(&(HEADER_BYTES + length)) {
        std::cmp::Ordering::Less => return Err(CodecError::Truncated),
        std::cmp::Ordering::Greater => return Err(CodecError::TrailingBytes),
        std::cmp::Ordering::Equal => {}
    }
    Ok((
        FrameHeader {
            lane,
            connection,
            sequence,
        },
        flags,
        &bytes[HEADER_BYTES..],
    ))
}

/// Append `value` as an unsigned LEB128 integer in its shortest form.
pub fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// A bounds-checked cursor over a received payload. Every read fails on
/// truncation rather than panicking, and varints must use their shortest form.
pub struct ByteReader<'a> {
    bytes: &'a [u8],
}
impl<'a> ByteReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
    pub fn remaining(&self) -> usize {
        self.bytes.len()
    }
    pub fn take(&mut self, size: usize) -> Result<&'a [u8], CodecError> {
        if size > self.bytes.len() {
            return Err(CodecError::Truncated);
        }
        let (value, rest) = self.bytes.split_at(size);
        self.bytes = rest;
        Ok(value)
    }
    pub fn u8(&mut self) -> Result<u8, CodecError> {
        Ok(self.take(1)?[0])
    }
    pub fn u64_le(&mut self) -> Result<u64, CodecError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn varint(&mut self) -> Result<u64, CodecError> {
        let mut value = 0u64;
        for index in 0..10 {
            let byte = self.u8()?;
            if index == 9 && byte > 1 {
                return Err(CodecError::InvalidPayload("varint overflow".into()));
            }
            value |= u64::from(byte & 0x7f) << (7 * index);
            if byte & 0x80 == 0 {
                if index > 0 && byte == 0 {
                    return Err(CodecError::InvalidPayload("overlong varint".into()));
                }
                return Ok(value);
            }
        }
        Err(CodecError::InvalidPayload("varint overflow".into()))
    }
    pub fn varint_u32(&mut self) -> Result<u32, CodecError> {
        u32::try_from(self.varint()?).map_err(|_| CodecError::InvalidPayload("varint range".into()))
    }
    pub fn finish(&self) -> Result<(), CodecError> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err(CodecError::TrailingBytes)
        }
    }
}

/// Game schemas validate finite numerics, identity and semantic ranges here.
/// Receiving a decoded value is not permission to apply it to the world.
pub trait Validate {
    fn validate(&self) -> Result<(), CodecError>;
}

fn options(limit: usize) -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .with_limit(limit as u64)
        .reject_trailing_bytes()
}

pub fn encode_payload<T: Serialize + Validate>(
    value: &T,
    limit: usize,
) -> Result<Vec<u8>, CodecError> {
    if limit == 0 || limit > u16::MAX as usize {
        return Err(CodecError::InvalidLimit);
    }
    value.validate()?;
    let size = options(limit)
        .serialized_size(value)
        .map_err(|e| CodecError::InvalidPayload(e.to_string()))?;
    if size > limit as u64 {
        return Err(CodecError::Oversized);
    }
    options(limit)
        .serialize(value)
        .map_err(|e| CodecError::InvalidPayload(e.to_string()))
}

pub fn decode_payload<T: DeserializeOwned + Validate>(
    bytes: &[u8],
    limit: usize,
) -> Result<T, CodecError> {
    if limit == 0 || limit > u16::MAX as usize {
        return Err(CodecError::InvalidLimit);
    }
    if bytes.len() > limit {
        return Err(CodecError::Oversized);
    }
    let value: T = options(limit)
        .deserialize(bytes)
        .map_err(|e| CodecError::InvalidPayload(e.to_string()))?;
    value.validate()?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Sample {
        values: BoundedVec<f32, 8>,
    }
    impl Validate for Sample {
        fn validate(&self) -> Result<(), CodecError> {
            if self
                .values
                .as_slice()
                .iter()
                .all(|v| v.is_finite() && v.abs() <= 1.0)
            {
                Ok(())
            } else {
                Err(CodecError::InvalidPayload(
                    "nonfinite or out-of-range control".into(),
                ))
            }
        }
    }

    #[test]
    fn frame_golden_vector_and_epoch_fences() {
        let limit = FrameLimit::new(64).unwrap();
        let header = FrameHeader {
            lane: Lane::Input,
            connection: ConnectionEpoch(7),
            sequence: u32::MAX,
        };
        let bytes = encode_frame(header, &[0x42], limit).unwrap();
        assert_eq!(
            bytes,
            [
                78, 82, 71, 49, 1, 0, 0, 0, 1, 0, 7, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 1, 0,
                66
            ]
        );
        assert_eq!(
            decode_frame(&bytes, ConnectionEpoch(7), limit).unwrap(),
            (header, &[0x42][..])
        );
        assert_eq!(
            decode_frame(&bytes, ConnectionEpoch(8), limit),
            Err(CodecError::WrongEpoch)
        );
        for length in 0..bytes.len() {
            assert!(decode_frame(&bytes[..length], ConnectionEpoch(7), limit).is_err());
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(
            decode_frame(&trailing, ConnectionEpoch(7), limit),
            Err(CodecError::TrailingBytes)
        );
        assert!(decode_frame(&bytes, ConnectionEpoch(7), FrameLimit::new(24).unwrap()).is_err());
        for offset in [0, 4, 6, 8, 9, 22] {
            let mut corrupt = bytes.clone();
            corrupt[offset] = 0xff;
            assert!(decode_frame(&corrupt, ConnectionEpoch(7), limit).is_err());
        }
    }

    #[test]
    fn bounded_counts_reject_before_reserving_and_numeric_validation_precedes_use() {
        let sample = Sample {
            values: BoundedVec::new(vec![0.25, -0.5]).unwrap(),
        };
        let bytes = encode_payload(&sample, 128).unwrap();
        assert_eq!(decode_payload::<Sample>(&bytes, 128).unwrap(), sample);
        assert!(decode_payload::<Sample>(&u64::MAX.to_le_bytes(), 128).is_err());
        let mut oversized = 9u64.to_le_bytes().to_vec();
        oversized.extend([0; 36]);
        assert!(decode_payload::<Sample>(&oversized, 128).is_err());
        let invalid = Sample {
            values: BoundedVec::new(vec![f32::NAN]).unwrap(),
        };
        assert!(encode_payload(&invalid, 128).is_err());
        let raw = bincode::serialize(&invalid).unwrap();
        assert!(decode_payload::<Sample>(&raw, 128).is_err());
        assert!(encode_payload(&sample, 4).is_err());
    }

    #[test]
    fn deflated_frames_shrink_repetition_and_never_grow_a_frame() {
        let limit = FrameLimit::new(1200).unwrap();
        let header = FrameHeader {
            lane: Lane::Input,
            connection: ConnectionEpoch(3),
            sequence: 9,
        };
        let repetitive = [1u8, 0, 0, 0, 0, 0, 0, 0].repeat(100);
        let wire = encode_frame_deflated(header, &repetitive, limit).unwrap();
        assert!(wire.len() < HEADER_BYTES + repetitive.len() / 4);
        let (decoded, payload) = decode_frame_inflated(&wire, ConnectionEpoch(3), limit).unwrap();
        assert_eq!(decoded, header);
        assert_eq!(&*payload, repetitive.as_slice());
        // The plain decoder does not accept a deflated frame.
        assert_eq!(
            decode_frame(&wire, ConnectionEpoch(3), limit),
            Err(CodecError::InvalidFlags)
        );
        // Incompressible bytes are sent as encode_frame would send them.
        let noise: Vec<u8> = (0..64u32)
            .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
            .collect();
        assert_eq!(
            encode_frame_deflated(header, &noise, limit).unwrap(),
            encode_frame(header, &noise, limit).unwrap()
        );
        // Inflation beyond the frame's payload limit is refused.
        let mut oversized = wire.clone();
        oversized[HEADER_BYTES..HEADER_BYTES + 4].copy_from_slice(&5000u32.to_le_bytes());
        assert!(decode_frame_inflated(&oversized, ConnectionEpoch(3), limit).is_err());
        let mut unknown = wire;
        unknown[9] = 2;
        assert_eq!(
            decode_frame_inflated(&unknown, ConnectionEpoch(3), limit).map(|_| ()),
            Err(CodecError::InvalidFlags)
        );
    }
}
