use super::*;

/// Deterministic single-span byte patch for positively projected DTO bytes.
/// Delta layout: target length, shared prefix length, shared suffix length (all
/// little-endian u32), followed by the replaced middle. No compression dictionary,
/// unchecked offsets, or allocation proportional to an untrusted declared length.
/// This codec validates byte structure; callers still validate decoded DTO schema
/// and authorized representation before committing/ACKing a received state.
#[derive(Debug, Clone, Copy, Default)]
pub struct BytePatch;
impl BaselineCodec<Vec<u8>> for BytePatch {
    fn encode_full(&self, value: &Vec<u8>, limit: usize) -> Result<Vec<u8>, ReplicationError> {
        self.decode_full(value, limit)
    }
    fn decode_full(&self, bytes: &[u8], limit: usize) -> Result<Vec<u8>, ReplicationError> {
        if bytes.len() > limit {
            return Err(ReplicationError::Capacity);
        }
        Ok(bytes.to_vec())
    }
    fn encode_delta(
        &self,
        base: &Vec<u8>,
        value: &Vec<u8>,
        limit: usize,
    ) -> Result<Vec<u8>, ReplicationError> {
        let prefix = base.iter().zip(value).take_while(|(a, b)| a == b).count();
        let suffix = base[prefix..]
            .iter()
            .rev()
            .zip(value[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let middle = &value[prefix..value.len() - suffix];
        let size = middle
            .len()
            .checked_add(12)
            .ok_or(ReplicationError::Capacity)?;
        if size > limit {
            return Err(ReplicationError::Capacity);
        }
        let lengths = [value.len(), prefix, suffix].map(u32::try_from);
        if lengths.iter().any(Result::is_err) {
            return Err(ReplicationError::Capacity);
        }
        let mut result = Vec::with_capacity(size);
        for length in lengths {
            result.extend_from_slice(&length.unwrap().to_le_bytes());
        }
        result.extend_from_slice(middle);
        Ok(result)
    }
    fn decode_delta(
        &self,
        base: &Vec<u8>,
        bytes: &[u8],
        limit: usize,
    ) -> Result<Vec<u8>, ReplicationError> {
        if bytes.len() < 12 {
            return Err(ReplicationError::InvalidPayload);
        }
        let field =
            |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let (length, prefix, suffix) = (field(0), field(4), field(8));
        if length > limit {
            return Err(ReplicationError::Capacity);
        }
        let shared = prefix
            .checked_add(suffix)
            .ok_or(ReplicationError::InvalidPayload)?;
        if shared > base.len() || shared > length || length - shared != bytes.len() - 12 {
            return Err(ReplicationError::InvalidPayload);
        }
        let mut result = Vec::with_capacity(length);
        result.extend_from_slice(&base[..prefix]);
        result.extend_from_slice(&bytes[12..]);
        result.extend_from_slice(&base[base.len() - suffix..]);
        Ok(result)
    }
    fn validate(&self, _: &Vec<u8>) -> bool {
        true
    }
}
