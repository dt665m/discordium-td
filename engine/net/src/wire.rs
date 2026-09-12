//! One bounded, lossless codec for gameplay messages. By default every input is
//! immediately eligible; compressed framing is selected only when strictly smaller
//! than raw framing. Policy can disable compression or raise the eligibility size.
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

/// Outgoing compression selection. Decoders always accept both wire forms.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CompressionMode {
    #[default]
    Auto,
    Off,
}

/// Local encoding policy; it does not alter the negotiated wire format.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompressionPolicy {
    pub mode: CompressionMode,
    /// Minimum serialized body bytes, before the raw/compressed framing. Zero
    /// considers every message; an eligible candidate still has to save bytes.
    pub minimum_bytes: usize,
}

pub fn encode<T: Serialize>(value: &T) -> Vec<u8> {
    encode_with_compression(value, CompressionPolicy::default())
}

pub fn encode_with_compression<T: Serialize>(value: &T, policy: CompressionPolicy) -> Vec<u8> {
    let raw = serialize(value);
    if policy.mode == CompressionMode::Auto && raw.len() >= policy.minimum_bytes {
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
    fn small_repetitive_state_compresses_with_bounded_exact_decode() {
        let value = vec![17u8; 20];
        let raw = serialize(&value);
        let encoded = encode(&value);
        assert_eq!(encoded[0], 1);
        assert!(encoded.len() < raw.len() + 1);
        assert_eq!(
            decode_with_limit::<Vec<u8>>(&encoded, raw.len()).unwrap(),
            value
        );
        assert!(decode_with_limit::<Vec<u8>>(&encoded, raw.len() - 1).is_err());
        let mut trailing = encoded.clone();
        trailing.push(0);
        assert!(decode::<Vec<u8>>(&trailing).is_err());
        assert!(decode::<Vec<u8>>(&encoded[..encoded.len() - 1]).is_err());
    }
    #[test]
    fn configured_compression_preserves_raw_and_exact_threshold_semantics() {
        let value = vec![17u8; 20];
        let raw_bytes = serialize(&value).len();
        let off = encode_with_compression(
            &value,
            CompressionPolicy {
                mode: CompressionMode::Off,
                minimum_bytes: 0,
            },
        );
        assert_eq!(off[0], 0);
        assert_eq!(off.len(), raw_bytes + 1);
        assert_eq!(decode::<Vec<u8>>(&off).unwrap(), value);
        let skipped = encode_with_compression(
            &value,
            CompressionPolicy {
                minimum_bytes: raw_bytes + 1,
                ..Default::default()
            },
        );
        assert_eq!(skipped, off);
        let eligible = encode_with_compression(
            &value,
            CompressionPolicy {
                minimum_bytes: raw_bytes,
                ..Default::default()
            },
        );
        assert_eq!(eligible[0], 1);
        assert_eq!(eligible, encode(&value));
        assert_eq!(decode::<Vec<u8>>(&eligible).unwrap(), value);
    }
    #[test]
    fn expanding_compression_candidate_keeps_the_raw_frame() {
        let value = 17u8;
        assert_eq!(encode(&value), vec![0, value]);
        assert_eq!(decode::<u8>(&encode(&value)).unwrap(), value);
    }
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
