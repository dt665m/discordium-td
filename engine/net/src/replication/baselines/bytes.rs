use super::*;

/// Deterministic sparse byte patch for positively projected DTO bytes.
///
/// Delta layout: the target length, then runs of `skip`, `len` and `len`
/// replacement bytes, all lengths LEB128 `u32`. Skipped bytes and whatever
/// follows the last run are copied from the base. A state whose changes are
/// scattered (a position near the start, cooldowns near the end) therefore
/// costs roughly its changed bytes, not the span between its first and last
/// change. No compression dictionary, unchecked offset, or allocation beyond
/// the caller's limit. This codec validates byte structure; callers still
/// validate the decoded DTO schema and authorized representation before
/// committing or acknowledging a received state.
#[derive(Debug, Clone, Copy, Default)]
pub struct BytePatch;

/// Unchanged gaps shorter than a run header are cheaper to resend in place.
const MERGE_GAP: usize = 3;

fn put_varint(out: &mut Vec<u8>, mut value: u32) {
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

fn take_varint(bytes: &[u8], cursor: &mut usize) -> Result<usize, ReplicationError> {
    let mut value = 0u32;
    for shift in (0..35).step_by(7) {
        let byte = *bytes.get(*cursor).ok_or(ReplicationError::InvalidPayload)?;
        *cursor += 1;
        let low = u32::from(byte & 0x7f);
        if shift == 28 && low > 0x0f {
            return Err(ReplicationError::InvalidPayload);
        }
        value |= low << shift;
        if byte & 0x80 == 0 {
            return Ok(value as usize);
        }
    }
    Err(ReplicationError::InvalidPayload)
}

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
        let length = u32::try_from(value.len()).map_err(|_| ReplicationError::Capacity)?;
        let changed = |index: usize| base.get(index) != value.get(index);
        let mut result = Vec::new();
        put_varint(&mut result, length);
        let mut copied = 0;
        let mut index = 0;
        while index < value.len() {
            if !changed(index) {
                index += 1;
                continue;
            }
            // Extend the run across short unchanged gaps; stop at a gap long
            // enough to pay for a new run header.
            let start = index;
            let mut end = index + 1;
            while end < value.len() {
                if changed(end) {
                    end += 1;
                    continue;
                }
                let gap = (end..value.len().min(end + MERGE_GAP))
                    .take_while(|&at| !changed(at))
                    .count();
                if gap >= MERGE_GAP || end + gap >= value.len() {
                    break;
                }
                end += gap;
            }
            put_varint(&mut result, (start - copied) as u32);
            put_varint(&mut result, (end - start) as u32);
            result.extend_from_slice(&value[start..end]);
            if result.len() > limit {
                return Err(ReplicationError::Capacity);
            }
            copied = end;
            index = end;
        }
        Ok(result)
    }
    fn decode_delta(
        &self,
        base: &Vec<u8>,
        bytes: &[u8],
        limit: usize,
    ) -> Result<Vec<u8>, ReplicationError> {
        let mut cursor = 0;
        let length = take_varint(bytes, &mut cursor)?;
        if length > limit {
            return Err(ReplicationError::Capacity);
        }
        let mut result = Vec::with_capacity(length);
        while cursor < bytes.len() {
            let skip = take_varint(bytes, &mut cursor)?;
            let run = take_varint(bytes, &mut cursor)?;
            let from = result.len();
            let kept = from
                .checked_add(skip)
                .ok_or(ReplicationError::InvalidPayload)?;
            let end = kept
                .checked_add(run)
                .ok_or(ReplicationError::InvalidPayload)?;
            let replacement = cursor
                .checked_add(run)
                .and_then(|stop| bytes.get(cursor..stop))
                .ok_or(ReplicationError::InvalidPayload)?;
            if run == 0 || kept > base.len() || end > length {
                return Err(ReplicationError::InvalidPayload);
            }
            result.extend_from_slice(&base[from..kept]);
            result.extend_from_slice(replacement);
            cursor += run;
        }
        let from = result.len();
        if from < length {
            // Everything after the last run is unchanged, so it must exist.
            let tail = base
                .get(from..length)
                .ok_or(ReplicationError::InvalidPayload)?;
            result.extend_from_slice(tail);
        }
        Ok(result)
    }
    fn validate(&self, _: &Vec<u8>) -> bool {
        true
    }
}

#[cfg(test)]
mod sparse_tests {
    use super::*;

    fn roundtrip(base: &[u8], value: &[u8]) -> usize {
        let (base, value) = (base.to_vec(), value.to_vec());
        let delta = BytePatch.encode_delta(&base, &value, 1 << 16).unwrap();
        assert_eq!(
            BytePatch.decode_delta(&base, &delta, 1 << 16).unwrap(),
            value
        );
        delta.len()
    }

    #[test]
    fn scattered_changes_cost_their_bytes_not_their_span() {
        let base: Vec<u8> = (0..1200).map(|i| (i % 251) as u8).collect();
        let mut value = base.clone();
        for at in [3, 4, 5, 6, 400, 401, 1195, 1196, 1197, 1198] {
            value[at] ^= 0xff;
        }
        let size = roundtrip(&base, &value);
        assert!(size < 40, "three small edits cost {size} bytes");
        assert_eq!(roundtrip(&base, &base), 2, "no change is just the length");
    }

    #[test]
    fn growth_shrinkage_and_pseudo_random_edits_round_trip() {
        let mut state = 0x9e37_79b9_u32;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        for _ in 0..500 {
            let base: Vec<u8> = (0..next() % 300).map(|_| next() as u8).collect();
            let mut value = base.clone();
            value.truncate((next() % 320) as usize);
            while value.len() < (next() % 320) as usize {
                value.push(next() as u8);
            }
            for _ in 0..next() % 20 {
                if !value.is_empty() {
                    let at = next() as usize % value.len();
                    value[at] = next() as u8;
                }
            }
            roundtrip(&base, &value);
        }
    }

    #[test]
    fn malformed_patches_are_rejected_without_panicking() {
        let base = vec![1u8; 16];
        for bytes in [
            vec![],                                // no length
            vec![0x80],                            // truncated varint
            vec![0xff, 0xff, 0xff, 0xff, 0x7f],    // length beyond u32
            vec![20],                              // longer than base with no runs
            vec![16, 17, 1, 9],                    // skip beyond base
            vec![16, 0, 0],                        // empty run
            vec![16, 0, 4, 1, 2],                  // truncated replacement
            vec![4, 0, 8, 1, 1, 1, 1, 1, 1, 1, 1], // run past target length
        ] {
            assert!(
                BytePatch.decode_delta(&base, &bytes, 1024).is_err(),
                "{bytes:?}"
            );
        }
        assert_eq!(
            BytePatch.decode_delta(&base, &[200, 1], 100),
            Err(ReplicationError::Capacity)
        );
    }
}
