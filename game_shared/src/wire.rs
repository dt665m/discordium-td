//! One bounded, lossless codec for gameplay messages. Small inputs stay raw.
use bincode::Options;
use serde::{Serialize, de::DeserializeOwned};

pub(crate) const MAX_STATE_BYTES: usize = 1024 * 1024;

pub(crate) fn invalid(message: &str) -> bincode::Error {
    Box::new(bincode::ErrorKind::Custom(message.to_owned()))
}

pub(crate) fn serialize<T: Serialize>(value: &T) -> Vec<u8> {
    bincode::serialize(value).expect("failed to serialize network payload")
}

pub(crate) fn deserialize<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, bincode::Error> {
    if bytes.len() > MAX_STATE_BYTES {
        return Err(invalid("oversized state"));
    }
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(MAX_STATE_BYTES as u64)
        .reject_trailing_bytes()
        .deserialize(bytes)
}

pub fn encode<T: Serialize>(value: &T) -> Vec<u8> {
    let raw = serialize(value);
    if raw.len() >= 256 {
        let compressed = miniz_oxide::deflate::compress_to_vec(&raw, 1);
        if compressed.len() + 5 < raw.len() + 1 {
            let mut framed = Vec::with_capacity(compressed.len() + 5);
            framed.push(1);
            framed.extend_from_slice(&(raw.len() as u32).to_le_bytes());
            framed.extend_from_slice(&compressed);
            return framed;
        }
    }
    let mut framed = Vec::with_capacity(raw.len() + 1);
    framed.push(0);
    framed.extend_from_slice(&raw);
    framed
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, bincode::Error> {
    decode_with_limit(bytes, MAX_STATE_BYTES)
}

/// Bound expansion before allocating or inflating an untrusted message.
pub fn decode_with_limit<T: DeserializeOwned>(
    bytes: &[u8],
    limit: usize,
) -> Result<T, bincode::Error> {
    let limit = limit.min(MAX_STATE_BYTES);
    if bytes.len() > limit + 1 {
        return Err(invalid("oversized message"));
    }
    match bytes.split_first() {
        Some((&0, raw)) if raw.len() <= limit => deserialize(raw),
        Some((&1, body)) if body.len() >= 4 => {
            let size = u32::from_le_bytes(body[..4].try_into().unwrap()) as usize;
            if size > limit {
                return Err(invalid("oversized inflated state"));
            }
            use miniz_oxide::inflate::{
                TINFLStatus,
                core::{DecompressorOxide, decompress, inflate_flags},
            };
            let mut raw = vec![0; size];
            let (status, consumed, written) = decompress(
                &mut Box::<DecompressorOxide>::default(),
                &body[4..],
                &mut raw,
                0,
                inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF,
            );
            if status != TINFLStatus::Done || consumed != body.len() - 4 || written != size {
                return Err(invalid("invalid compressed state or trailing bytes"));
            }
            deserialize(&raw)
        }
        _ => Err(invalid("invalid wire frame")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compressed_roundtrip_and_decode_limits() {
        let value = vec![17u32; 8192];
        let encoded = encode(&value);
        assert_eq!(encoded[0], 1);
        assert!(encoded.len() < 512);
        assert_eq!(decode::<Vec<u32>>(&encoded).unwrap(), value);
        assert!(decode::<Vec<u32>>(&encoded[..encoded.len() / 2]).is_err());
        assert!(decode_with_limit::<Vec<u32>>(&encoded, 2048).is_err());
        let mut trailing = encoded.clone();
        trailing.push(0);
        assert!(decode::<Vec<u32>>(&trailing).is_err());
        let mut oversized = encoded.clone();
        oversized[1..5].copy_from_slice(&((MAX_STATE_BYTES + 1) as u32).to_le_bytes());
        assert!(decode::<Vec<u32>>(&oversized).is_err());
        let mut undersized = encoded;
        undersized[1..5].copy_from_slice(&4u32.to_le_bytes());
        assert!(decode::<Vec<u32>>(&undersized).is_err());
    }
}
