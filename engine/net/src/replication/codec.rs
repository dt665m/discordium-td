//! Explicit bounded canonical group envelope. All integers are little-endian;
//! payload encoding remains the registered representation codec's responsibility.
use super::*;

/// One independently meaningful complete scope. All header fields are fixed
/// width and payload length is bounded before allocating any replica bytes.
pub fn encode_full_state(
    state: &FullState<Vec<u8>>,
    limit: usize,
) -> Result<Vec<u8>, ReplicationError> {
    state.validate_identity(state.scope.connection)?;
    if state
        .payload
        .len()
        .checked_add(68)
        .is_none_or(|n| n > limit)
    {
        return Err(ReplicationError::Capacity);
    }
    let mut out = Vec::with_capacity(68 + state.payload.len());
    put_scope(&mut out, state.scope);
    put64(&mut out, state.baseline_generation.0);
    put64(&mut out, state.snapshot.0);
    put64(&mut out, state.version.0);
    put64(&mut out, state.end_tick.0);
    let length = u32::try_from(state.payload.len()).map_err(|_| ReplicationError::Capacity)?;
    out.extend_from_slice(&length.to_le_bytes());
    out.extend_from_slice(&state.payload);
    Ok(out)
}

pub fn decode_full_state(
    bytes: &[u8],
    connection: ConnectionEpoch,
    limit: usize,
) -> Result<FullState<Vec<u8>>, ReplicationError> {
    if bytes.len() > limit {
        return Err(ReplicationError::Capacity);
    }
    let mut input = Reader(bytes);
    let scope = input.scope()?;
    validate_scope(scope, connection)?;
    let baseline_generation = BaselineGeneration(input.u64()?);
    let snapshot = SnapshotId(input.u64()?);
    let version = StateVersion(input.u64()?);
    let end_tick = ServerTick(input.u64()?);
    let length = input.u32()? as usize;
    if length > limit || length != input.0.len() {
        return Err(ReplicationError::InvalidPayload);
    }
    let mut state = FullState {
        scope,
        baseline_generation,
        snapshot,
        version,
        end_tick,
        payload: Vec::new(),
    };
    state.validate_identity(connection)?;
    state.payload = input.take(length)?.to_vec();
    Ok(state)
}

/// Compact complete state for a frame that already authenticates its connection.
/// All remaining identity fields use canonical unsigned LEB128. This is a
/// separate wire format; the fixed-width codec remains unchanged.
pub fn encode_compact_full_state(
    state: &FullState<Vec<u8>>,
    limit: usize,
) -> Result<Vec<u8>, ReplicationError> {
    state.validate_identity(state.scope.connection)?;
    let mut header = Vec::with_capacity(90);
    for value in [
        state.scope.entity.index,
        u64::from(state.scope.entity.generation),
        state.scope.scope.0,
        u64::from(state.scope.representation.0),
        state.baseline_generation.0,
        state.snapshot.0,
        state.version.0,
        state.end_tick.0,
        u64::try_from(state.payload.len()).map_err(|_| ReplicationError::Capacity)?,
    ] {
        put_varint(&mut header, value);
    }
    if header
        .len()
        .checked_add(state.payload.len())
        .is_none_or(|size| size > limit)
    {
        return Err(ReplicationError::Capacity);
    }
    header.extend_from_slice(&state.payload);
    Ok(header)
}

/// `connection` must come from the validated enclosing frame, never its payload.
/// Rejects noncanonical integers and bounds the payload before allocating it.
pub fn decode_compact_full_state(
    bytes: &[u8],
    connection: ConnectionEpoch,
    limit: usize,
) -> Result<FullState<Vec<u8>>, ReplicationError> {
    if bytes.len() > limit {
        return Err(ReplicationError::Capacity);
    }
    let mut input = Reader(bytes);
    let scope = ScopeIdentity {
        connection,
        entity: EntityId {
            index: input.varint()?,
            generation: input.varint_u32()?,
        },
        scope: ScopeEpoch(input.varint()?),
        representation: RepresentationRevision(input.varint_u32()?),
    };
    let mut state = FullState {
        scope,
        baseline_generation: BaselineGeneration(input.varint()?),
        snapshot: SnapshotId(input.varint()?),
        version: StateVersion(input.varint()?),
        end_tick: ServerTick(input.varint()?),
        payload: Vec::new(),
    };
    state.validate_identity(connection)?;
    let length = usize::try_from(input.varint()?).map_err(|_| ReplicationError::InvalidPayload)?;
    if length > limit || length != input.0.len() {
        return Err(ReplicationError::InvalidPayload);
    }
    state.payload = input.take(length)?.to_vec();
    Ok(state)
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 128 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// Encode exactly one complete group. Payload callbacks must encode only the
/// approved representation and apply their own bounded schema rules.
pub fn encode_group<P: Payload>(
    group: &GroupChunk<P>,
    limits: GroupLimits,
    encode_payload: impl Fn(&P) -> Result<Vec<u8>, ReplicationError>,
) -> Result<Vec<u8>, ReplicationError> {
    if group.count != 1
        || group.index != 0
        || group.manifest.is_empty()
        || group.manifest.len() > limits.members
        || group.members.len() != group.manifest.len()
        || group.publication.connection.0 == 0
        || group.publication.revision.0 == 0
        || group.publication.snapshot.0 == 0
    {
        return Err(ReplicationError::InvalidIdentity);
    }
    let count = u16::try_from(group.manifest.len()).map_err(|_| ReplicationError::Capacity)?;
    let minimum = 39usize
        .checked_add(
            group
                .manifest
                .len()
                .checked_mul(100)
                .ok_or(ReplicationError::Capacity)?,
        )
        .ok_or(ReplicationError::Capacity)?;
    if minimum > limits.group_bytes {
        return Err(ReplicationError::Capacity);
    }
    let mut out = Vec::with_capacity(minimum);
    out.push(1);
    put64(&mut out, group.publication.connection.0);
    out.extend_from_slice(&group.publication.group.0.to_le_bytes());
    put64(&mut out, group.publication.revision.0);
    put64(&mut out, group.publication.snapshot.0);
    put64(&mut out, group.end_tick.0);
    out.extend_from_slice(&count.to_le_bytes());
    for (index, scope) in group.manifest.iter().enumerate() {
        validate_scope(*scope, group.publication.connection)?;
        if index > 0
            && (group.manifest[index - 1] >= *scope
                || group.manifest[index - 1].entity.index == scope.entity.index)
        {
            return Err(ReplicationError::Conflict);
        }
        put_scope(&mut out, *scope);
    }
    for scope in &group.manifest {
        let mut matching = group.members.iter().filter(|state| state.scope == *scope);
        let state = matching.next().ok_or(ReplicationError::Incomplete)?;
        if matching.next().is_some() || state.end_tick != group.end_tick {
            return Err(ReplicationError::Conflict);
        }
        state.validate_identity(group.publication.connection)?;
        let payload = encode_payload(&state.payload)?;
        let total = out
            .len()
            .checked_add(68)
            .and_then(|n| n.checked_add(payload.len()))
            .ok_or(ReplicationError::Capacity)?;
        if total > limits.group_bytes {
            return Err(ReplicationError::Capacity);
        }
        let length = u32::try_from(payload.len()).map_err(|_| ReplicationError::Capacity)?;
        put_scope(&mut out, state.scope);
        put64(&mut out, state.baseline_generation.0);
        put64(&mut out, state.snapshot.0);
        put64(&mut out, state.version.0);
        put64(&mut out, state.end_tick.0);
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&payload);
    }
    Ok(out)
}

/// Decode count/length-delimited fields only after checking configured limits.
/// The callback sees a bounded slice and must itself bound all nested schema
/// allocations (e.g. never use unrestricted serde allocation on remote lengths).
pub fn decode_group<P: Payload>(
    bytes: &[u8],
    limits: GroupLimits,
    decode_payload: impl Fn(&[u8]) -> Result<P, ReplicationError>,
) -> Result<GroupChunk<P>, ReplicationError> {
    if bytes.len() > limits.group_bytes {
        return Err(ReplicationError::Capacity);
    }
    let mut input = Reader(bytes);
    if input.take(1)? != [1] {
        return Err(ReplicationError::InvalidPayload);
    }
    let publication = GroupPublication {
        connection: ConnectionEpoch(input.u64()?),
        group: GroupId(input.u32()?),
        revision: GroupRevision(input.u64()?),
        snapshot: SnapshotId(input.u64()?),
    };
    if publication.connection.0 == 0 || publication.revision.0 == 0 || publication.snapshot.0 == 0 {
        return Err(ReplicationError::InvalidIdentity);
    }
    let end_tick = ServerTick(input.u64()?);
    let count = input.u16()? as usize;
    if count == 0
        || count > limits.members
        || count.checked_mul(100).is_none_or(|n| n > input.0.len())
    {
        return Err(ReplicationError::Capacity);
    }
    let mut manifest: Vec<ScopeIdentity> = Vec::with_capacity(count);
    for _ in 0..count {
        let scope = input.scope()?;
        validate_scope(scope, publication.connection)?;
        if manifest
            .last()
            .is_some_and(|old| *old >= scope || old.entity.index == scope.entity.index)
        {
            return Err(ReplicationError::Conflict);
        }
        manifest.push(scope);
    }
    let mut members = Vec::with_capacity(count);
    let mut retained = 0usize;
    for scope in &manifest {
        let member_scope = input.scope()?;
        if member_scope != *scope {
            return Err(ReplicationError::Conflict);
        }
        let baseline_generation = BaselineGeneration(input.u64()?);
        let snapshot = SnapshotId(input.u64()?);
        let version = StateVersion(input.u64()?);
        let tick = ServerTick(input.u64()?);
        let length = input.u32()? as usize;
        if length > limits.group_bytes || tick != end_tick {
            return Err(ReplicationError::InvalidPayload);
        }
        let payload = decode_payload(input.take(length)?)?;
        retained = retained
            .checked_add(payload.retained_bytes())
            .ok_or(ReplicationError::Capacity)?;
        if retained > limits.group_bytes {
            return Err(ReplicationError::Capacity);
        }
        let state = FullState {
            scope: member_scope,
            baseline_generation,
            snapshot,
            version,
            end_tick: tick,
            payload,
        };
        state.validate_identity(publication.connection)?;
        members.push(state);
    }
    if !input.0.is_empty() {
        return Err(ReplicationError::InvalidPayload);
    }
    Ok(GroupChunk {
        publication,
        end_tick,
        manifest,
        index: 0,
        count: 1,
        members,
    })
}
fn put64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}
fn put_scope(out: &mut Vec<u8>, scope: ScopeIdentity) {
    put64(out, scope.connection.0);
    put64(out, scope.entity.index);
    out.extend_from_slice(&scope.entity.generation.to_le_bytes());
    put64(out, scope.scope.0);
    out.extend_from_slice(&scope.representation.0.to_le_bytes());
}
struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], ReplicationError> {
        if count > self.0.len() {
            return Err(ReplicationError::InvalidPayload);
        }
        let (value, rest) = self.0.split_at(count);
        self.0 = rest;
        Ok(value)
    }
    fn varint(&mut self) -> Result<u64, ReplicationError> {
        let mut value = 0u64;
        for byte_index in 0..10 {
            let byte = self.take(1)?[0];
            if byte_index == 9 && byte > 1 {
                return Err(ReplicationError::InvalidPayload);
            }
            value |= u64::from(byte & 0x7f) << (byte_index * 7);
            if byte & 0x80 == 0 {
                if byte_index != 0 && byte == 0 {
                    return Err(ReplicationError::InvalidPayload);
                }
                return Ok(value);
            }
        }
        Err(ReplicationError::InvalidPayload)
    }
    fn varint_u32(&mut self) -> Result<u32, ReplicationError> {
        u32::try_from(self.varint()?).map_err(|_| ReplicationError::InvalidPayload)
    }
    fn u16(&mut self) -> Result<u16, ReplicationError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, ReplicationError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, ReplicationError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn scope(&mut self) -> Result<ScopeIdentity, ReplicationError> {
        Ok(ScopeIdentity {
            connection: ConnectionEpoch(self.u64()?),
            entity: EntityId {
                index: self.u64()?,
                generation: self.u32()?,
            },
            scope: ScopeEpoch(self.u64()?),
            representation: RepresentationRevision(self.u32()?),
        })
    }
}

#[cfg(test)]
mod compact_tests {
    use super::*;

    fn state() -> FullState<Vec<u8>> {
        FullState {
            scope: ScopeIdentity {
                connection: ConnectionEpoch(91),
                entity: EntityId {
                    index: 4,
                    generation: 1,
                },
                scope: ScopeEpoch(1),
                representation: RepresentationRevision(1),
            },
            baseline_generation: BaselineGeneration(1),
            snapshot: SnapshotId(120),
            version: StateVersion(100),
            end_tick: ServerTick(120),
            payload: vec![5, 8, 13],
        }
    }

    #[test]
    fn compact_full_roundtrips_without_changing_fixed_format() {
        let value = state();
        let bytes = encode_compact_full_state(&value, 128).unwrap();
        assert_eq!(bytes, [4, 1, 1, 1, 1, 120, 100, 120, 3, 5, 8, 13]);
        assert_eq!(
            decode_compact_full_state(&bytes, value.scope.connection, 128).unwrap(),
            value
        );
        let fixed = encode_full_state(&value, 128).unwrap();
        assert_eq!(fixed.len(), 68 + value.payload.len());
        assert_eq!(
            decode_full_state(&fixed, value.scope.connection, 128).unwrap(),
            value
        );
    }

    #[test]
    fn compact_full_preserves_maximum_width_fields() {
        let mut value = state();
        value.scope.entity.index = u64::MAX;
        value.scope.entity.generation = u32::MAX;
        value.scope.scope = ScopeEpoch(u64::MAX);
        value.scope.representation = RepresentationRevision(u32::MAX);
        value.baseline_generation = BaselineGeneration(u64::MAX);
        value.snapshot = SnapshotId(u64::MAX);
        value.version = StateVersion(u64::MAX);
        value.end_tick = ServerTick(u64::MAX);
        let bytes = encode_compact_full_state(&value, 128).unwrap();
        assert_eq!(
            decode_compact_full_state(&bytes, value.scope.connection, 128).unwrap(),
            value
        );
    }

    #[test]
    fn compact_full_integer_boundaries_are_canonical() {
        for boundary in [
            0,
            1,
            127,
            128,
            16_383,
            16_384,
            u64::from(u32::MAX),
            u64::MAX,
        ] {
            let mut value = state();
            value.scope.entity.index = boundary;
            value.end_tick = ServerTick(boundary);
            let bytes = encode_compact_full_state(&value, 128).unwrap();
            let decoded = decode_compact_full_state(&bytes, value.scope.connection, 128).unwrap();
            assert_eq!(decoded, value);
            assert_eq!(encode_compact_full_state(&decoded, 128).unwrap(), bytes);
        }
    }

    #[test]
    fn compact_full_rejects_truncation_trailing_and_limits() {
        let value = state();
        let bytes = encode_compact_full_state(&value, 128).unwrap();
        for length in 0..bytes.len() {
            assert!(
                decode_compact_full_state(&bytes[..length], value.scope.connection, 128).is_err()
            );
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(decode_compact_full_state(&trailing, value.scope.connection, 128).is_err());
        assert!(encode_compact_full_state(&value, bytes.len() - 1).is_err());
        assert!(
            decode_compact_full_state(&bytes, value.scope.connection, bytes.len() - 1).is_err()
        );
        let mut oversized = bytes[..8].to_vec();
        put_varint(&mut oversized, u64::MAX);
        assert!(decode_compact_full_state(&oversized, value.scope.connection, 128).is_err());
    }

    #[test]
    fn compact_full_rejects_noncanonical_overflow_and_invalid_identity() {
        let value = state();
        let bytes = encode_compact_full_state(&value, 128).unwrap();
        for prefix in [vec![0x84, 0], vec![0x80; 10], vec![0xff; 10]] {
            let mut malformed = prefix;
            malformed.extend_from_slice(&bytes[1..]);
            assert!(decode_compact_full_state(&malformed, value.scope.connection, 128).is_err());
        }
        let mut wide_generation = vec![bytes[0]];
        put_varint(&mut wide_generation, u64::from(u32::MAX) + 1);
        wide_generation.extend_from_slice(&bytes[2..]);
        assert!(decode_compact_full_state(&wide_generation, value.scope.connection, 128).is_err());
        for index in 1..=6 {
            let mut invalid = bytes.clone();
            invalid[index] = 0;
            assert!(decode_compact_full_state(&invalid, value.scope.connection, 128).is_err());
        }
        assert!(decode_compact_full_state(&bytes, ConnectionEpoch(0), 128).is_err());
        let mut invalid = value;
        invalid.scope.scope = ScopeEpoch(0);
        assert!(encode_compact_full_state(&invalid, 128).is_err());
    }
}
