//! Lossless patch metadata: relative entity IDs, bounded unsigned varints, and
//! per-record choice of byte mask or sparse ranges. Values remain current bytes.
use crate::{RecordPatch, StateKey, WorldPatch, wire};

const REMOVED: u8 = 0;
const RANGES: u8 = 1;
const MASK: u8 = 2;
const FULL: u8 = 3;

fn write_var(mut value: u64, out: &mut Vec<u8>) {
    while value >= 128 {
        out.push(value as u8 | 128);
        value >>= 7;
    }
    out.push(value as u8);
}
fn var_size(value: usize) -> usize {
    ((usize::BITS - value.leading_zeros()).max(1) as usize).div_ceil(7)
}
fn key_parts(key: StateKey) -> (usize, u64) {
    match key {
        StateKey::World => (0, 0),
        StateKey::Hero(id) => (1, id),
        StateKey::Enemy(id) => (2, id),
        StateKey::Tower(id) => (3, id),
    }
}

fn ranges(mask: &[u8], len: usize) -> impl Iterator<Item = std::ops::Range<usize>> + '_ {
    let mut index = 0;
    std::iter::from_fn(move || {
        while index < len && mask[index / 8] & (1 << (index % 8)) == 0 {
            index += 1;
        }
        if index == len {
            return None;
        }
        let start = index;
        while index < len && mask[index / 8] & (1 << (index % 8)) != 0 {
            index += 1;
        }
        Some(start..index)
    })
}

pub(crate) fn encode(patch: &WorldPatch) -> Result<Vec<u8>, bincode::Error> {
    let mut out = Vec::new();
    out.extend_from_slice(&patch.tick.to_le_bytes());
    write_var(
        patch.tick.wrapping_sub(patch.baseline_tick) as u64,
        &mut out,
    );
    write_var(patch.records.len() as u64, &mut out);
    let mut ids = [0; 4];
    let mut previous_key = None;
    for record in &patch.records {
        crate::replication::validate_record_patch(record)?;
        if previous_key.is_some_and(|old| old >= record.key) {
            return Err(wire::invalid("unordered records"));
        }
        previous_key = Some(record.key);
        let (kind, id) = key_parts(record.key);
        let len = record.state_len.unwrap_or_default() as usize;
        // Small repeated masks compress better across neighboring entities than
        // varying range headers, even when the uncompressed headers are shorter.
        let mut range_count = 0;
        let mut range_headers = 0;
        let mut previous_end = 0;
        if len > 256 && record.values.len() != len {
            for range in ranges(&record.mask, len) {
                range_count += 1;
                range_headers += var_size(range.start - previous_end) + var_size(range.len());
                previous_end = range.end;
            }
        }
        let mode = if record.state_len.is_none() {
            REMOVED
        } else if record.values.len() == len {
            FULL
        } else if len <= 256 || record.mask.len() < var_size(range_count) + range_headers {
            MASK
        } else {
            RANGES
        };
        out.push(kind as u8 | (mode << 2));
        write_var(id - ids[kind], &mut out);
        ids[kind] = id;
        if mode == REMOVED {
            continue;
        }
        write_var(len as u64, &mut out);
        match mode {
            MASK => {
                out.extend_from_slice(&record.mask);
                out.extend_from_slice(&record.values);
            }
            FULL => out.extend_from_slice(&record.values),
            RANGES => {
                write_var(range_count as u64, &mut out);
                let mut previous_end = 0;
                let mut value_offset = 0;
                for range in ranges(&record.mask, len) {
                    write_var((range.start - previous_end) as u64, &mut out);
                    write_var(range.len() as u64, &mut out);
                    out.extend_from_slice(&record.values[value_offset..value_offset + range.len()]);
                    value_offset += range.len();
                    previous_end = range.end;
                }
            }
            _ => unreachable!(),
        }
    }
    Ok(out)
}

struct Reader<'a> {
    bytes: &'a [u8],
}
impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], bincode::Error> {
        if count > self.bytes.len() {
            return Err(wire::invalid("truncated packed patch"));
        }
        let (head, tail) = self.bytes.split_at(count);
        self.bytes = tail;
        Ok(head)
    }
    fn byte(&mut self) -> Result<u8, bincode::Error> {
        Ok(self.take(1)?[0])
    }
    fn var(&mut self) -> Result<u64, bincode::Error> {
        let mut value = 0;
        for shift in (0..70).step_by(7) {
            let byte = self.byte()?;
            if shift == 63 && byte > 1 {
                return Err(wire::invalid("packed integer overflow"));
            }
            value |= ((byte & 127) as u64) << shift;
            if byte < 128 {
                if shift > 0 && byte == 0 {
                    return Err(wire::invalid("noncanonical packed integer"));
                }
                return Ok(value);
            }
        }
        Err(wire::invalid("packed integer overflow"))
    }
    fn size(&mut self, limit: usize) -> Result<usize, bincode::Error> {
        let value = self.var()?;
        if value > limit as u64 {
            return Err(wire::invalid("oversized packed value"));
        }
        Ok(value as usize)
    }
}

pub(crate) fn decode(bytes: &[u8]) -> Result<WorldPatch, bincode::Error> {
    if bytes.len() > wire::MAX_STATE_BYTES {
        return Err(wire::invalid("oversized packed patch"));
    }
    let mut input = Reader { bytes };
    let tick = u32::from_le_bytes(input.take(4)?.try_into().unwrap());
    let age = input.size(u32::MAX as usize)? as u32;
    let count = input.size(input.bytes.len() / 2)?;
    let mut records = Vec::new();
    let mut ids = [0u64; 4];
    let mut previous_key = None;
    let mut total = 0;
    for _ in 0..count {
        let tag = input.byte()?;
        if tag > 15 {
            return Err(wire::invalid("invalid packed record tag"));
        }
        let kind = (tag & 3) as usize;
        let mode = tag >> 2;
        let id = ids[kind]
            .checked_add(input.var()?)
            .ok_or_else(|| wire::invalid("packed identity overflow"))?;
        ids[kind] = id;
        let key = match kind {
            0 if id == 0 => StateKey::World,
            1 => StateKey::Hero(id),
            2 => StateKey::Enemy(id),
            3 => StateKey::Tower(id),
            _ => return Err(wire::invalid("invalid world identity")),
        };
        if previous_key.is_some_and(|old| old >= key) {
            return Err(wire::invalid("unordered packed records"));
        }
        previous_key = Some(key);
        let mut mask = Vec::new();
        let mut values = Vec::new();
        let state_len = if mode == REMOVED {
            None
        } else {
            let len = input.size(wire::MAX_STATE_BYTES - total)?;
            total += len;
            match mode {
                FULL => {
                    values = input.take(len)?.to_vec();
                    mask = vec![255; len.div_ceil(8)];
                    if !len.is_multiple_of(8) {
                        *mask.last_mut().unwrap() = (1 << (len % 8)) - 1;
                    }
                }
                MASK => {
                    let bytes = input.take(len.div_ceil(8))?;
                    if !len.is_multiple_of(8)
                        && bytes.last().is_some_and(|byte| byte >> (len % 8) != 0)
                    {
                        return Err(wire::invalid("out of bounds packed mask"));
                    }
                    let count = bytes.iter().map(|byte| byte.count_ones() as usize).sum();
                    values = input.take(count)?.to_vec();
                    mask = bytes.to_vec();
                }
                RANGES => {
                    let count = input.size(input.bytes.len() / 3)?;
                    let mut end = 0;
                    mask = vec![0; len.div_ceil(8)];
                    for _ in 0..count {
                        let start = end + input.size(len - end)?;
                        let count = input.size(len - start)?;
                        if count == 0 {
                            return Err(wire::invalid("empty packed range"));
                        }
                        end = start + count;
                        values.extend_from_slice(input.take(count)?);
                        for index in start..end {
                            mask[index / 8] |= 1 << (index % 8);
                        }
                    }
                }
                _ => unreachable!(),
            }
            Some(len as u32)
        };
        records.push(RecordPatch {
            key,
            state_len,
            mask,
            values,
        });
    }
    if !input.bytes.is_empty() {
        return Err(wire::invalid("trailing packed patch bytes"));
    }
    Ok(WorldPatch {
        tick,
        baseline_tick: tick.wrapping_sub(age),
        records,
    })
}

// Preserve the human-readable debug format; only binary gameplay packing changes.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(remote = "WorldPatch")]
struct Fields {
    tick: u32,
    baseline_tick: u32,
    records: Vec<RecordPatch>,
}
impl serde::Serialize for WorldPatch {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            Fields::serialize(self, serializer)
        } else {
            serializer.serialize_bytes(&encode(self).map_err(serde::ser::Error::custom)?)
        }
    }
}
impl<'de> serde::Deserialize<'de> for WorldPatch {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if deserializer.is_human_readable() {
            Fields::deserialize(deserializer)
        } else {
            let bytes = <Vec<u8> as serde::Deserialize>::deserialize(deserializer)?;
            decode(&bytes).map_err(serde::de::Error::custom)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changed_record(key: StateKey, len: usize, indices: &[usize]) -> RecordPatch {
        let mut mask = vec![0; len.div_ceil(8)];
        let mut values = Vec::new();
        for &index in indices {
            mask[index / 8] |= 1 << (index % 8);
            values.push((index.wrapping_mul(31)) as u8);
        }
        RecordPatch {
            key,
            state_len: Some(len as u32),
            mask,
            values,
        }
    }

    fn patch(records: Vec<RecordPatch>) -> WorldPatch {
        WorldPatch {
            tick: 3,
            baseline_tick: u32::MAX - 2,
            records,
        }
    }

    #[test]
    fn roundtrip_masks_ranges_full_tombstones_and_full_width_ids() {
        let fixtures = [
            patch(vec![changed_record(StateKey::World, 0, &[])]),
            patch(vec![changed_record(
                StateKey::Enemy(0),
                257,
                &[0, 63, 64, 127, 128, 255, 256],
            )]),
            patch(vec![changed_record(
                StateKey::Hero(u64::MAX),
                8192,
                &[1, 8191],
            )]),
            patch(vec![changed_record(
                StateKey::Enemy(u64::MAX),
                65,
                &(0..65).collect::<Vec<_>>(),
            )]),
            patch(vec![RecordPatch {
                key: StateKey::Tower(u64::MAX),
                state_len: None,
                mask: Vec::new(),
                values: Vec::new(),
            }]),
            patch(vec![
                changed_record(StateKey::Hero(0), 129, &[0, 128]),
                changed_record(StateKey::Hero(u64::MAX), 129, &[1, 64]),
                changed_record(StateKey::Enemy(0), 257, &[3]),
                changed_record(StateKey::Enemy(u64::MAX), 257, &[9]),
            ]),
        ];
        for expected in fixtures {
            let bytes = encode(&expected).unwrap();
            assert_eq!(decode(&bytes).unwrap(), expected);
            let wire = crate::encode(&crate::ServerWorldMessage::Patch(expected.clone()));
            assert_eq!(
                crate::decode::<crate::ServerWorldMessage>(&wire).unwrap(),
                crate::ServerWorldMessage::Patch(expected)
            );
        }
    }

    #[test]
    fn every_truncated_prefix_and_trailing_data_are_rejected() {
        let bytes = encode(&patch(vec![changed_record(
            StateKey::Enemy(1024),
            129,
            &[0, 1, 63, 64, 128],
        )]))
        .unwrap();
        for len in 0..bytes.len() {
            assert!(decode(&bytes[..len]).is_err(), "prefix {len}");
        }
        let mut trailing = bytes;
        trailing.push(0);
        assert!(decode(&trailing).is_err());
    }

    #[test]
    fn invalid_metadata_is_rejected_before_unbounded_allocation() {
        // Tick, baseline age, record count, record tag, relative ID, length.
        let mut bad_tag = vec![0, 0, 0, 0, 1, 1, 16, 0];
        assert!(decode(&bad_tag).is_err());
        bad_tag[6] = 1;
        bad_tag.extend([0x80; 10]);
        assert!(decode(&bad_tag).is_err());
        let noncanonical_age = [0, 0, 0, 0, 0x80, 0, 0];
        assert!(decode(&noncanonical_age).is_err());
        let bad_padding = [0, 0, 0, 0, 1, 1, 8, 0, 1, 0x80];
        assert!(decode(&bad_padding).is_err());
        let bad_world_id = [0, 0, 0, 0, 1, 1, 0, 1];
        assert!(decode(&bad_world_id).is_err());
        let duplicate = [0, 0, 0, 0, 1, 2, 2, 1, 2, 0];
        assert!(decode(&duplicate).is_err());
        let mut huge = vec![0, 0, 0, 0, 1, 1, 10, 1];
        write_var((wire::MAX_STATE_BYTES + 1) as u64, &mut huge);
        assert!(decode(&huge).is_err());
        let mut overflow = vec![0, 0, 0, 0, 1, 2, 2];
        write_var(u64::MAX, &mut overflow);
        overflow.extend([2, 1]);
        assert!(decode(&overflow).is_err());
    }

    #[test]
    fn arbitrary_input_and_mutations_never_panic() {
        let seed = encode(&patch(vec![changed_record(
            StateKey::Enemy(42),
            513,
            &[0, 4, 63, 64, 128, 512],
        )]))
        .unwrap();
        let mut random = 0x93ac_0271u32;
        for iteration in 0..20_000 {
            random = random.wrapping_mul(1664525).wrapping_add(1013904223);
            let mut bytes = if iteration % 2 == 0 {
                seed.clone()
            } else {
                vec![0; random as usize % 256]
            };
            for byte in &mut bytes {
                random = random.wrapping_mul(1664525).wrapping_add(1013904223);
                if iteration % 2 != 0 || random % 7 == 0 {
                    *byte = (random >> 24) as u8;
                }
            }
            if let Ok(decoded) = decode(&bytes) {
                assert_eq!(decode(&encode(&decoded).unwrap()).unwrap(), decoded);
            }
        }
    }
}
