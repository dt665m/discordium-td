use super::*;
use std::collections::BTreeMap;

/// Byte fragmentation covers the entire canonical group envelope, including its
/// manifest. Repeating a 32-member manifest in every 1200-byte frame can exceed
/// that frame before state bytes; only fixed publication metadata repeats here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodedGroupHeader {
    pub publication: GroupPublication,
    pub total_bytes: usize,
    pub fragments: usize,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByteFragment {
    pub header: EncodedGroupHeader,
    pub index: usize,
    pub payload: Box<[u8]>,
}
#[derive(Debug, Clone, Copy)]
pub struct ByteLimits {
    /// Actual fragment payload after transport and application header overhead.
    /// This is not the datagram envelope. Both peers must negotiate this value.
    pub fragment_payload_bytes: usize,
    pub group_bytes: usize,
    pub fragments: usize,
    pub incomplete: usize,
    pub known_groups: usize,
    pub staging_bytes: usize,
    pub max_age_ms: u64,
}
impl ByteLimits {
    pub fn validate(self) -> Result<Self, ReplicationError> {
        if self.fragment_payload_bytes == 0
            || self.group_bytes == 0
            || self.fragments == 0
            || self.incomplete == 0
            || self.known_groups < self.incomplete
            || self.staging_bytes < self.group_bytes
            || self.max_age_ms == 0
            || self
                .fragment_payload_bytes
                .checked_mul(self.fragments)
                .is_none_or(|n| n < self.group_bytes)
        {
            return Err(ReplicationError::InvalidConfiguration);
        }
        Ok(self)
    }
}

/// Fragment already bounded canonical bytes. The transport adapter must include
/// this fixed header's encoded size when calculating fragment_payload_bytes.
pub fn fragment_group(
    publication: GroupPublication,
    bytes: &[u8],
    limits: ByteLimits,
) -> Result<Vec<ByteFragment>, ReplicationError> {
    let limits = limits.validate()?;
    if publication.connection.0 == 0 || publication.revision.0 == 0 || publication.snapshot.0 == 0 {
        return Err(ReplicationError::InvalidIdentity);
    }
    if bytes.is_empty() || bytes.len() > limits.group_bytes {
        return Err(ReplicationError::Capacity);
    }
    let fragments = bytes.len().div_ceil(limits.fragment_payload_bytes);
    let header = EncodedGroupHeader {
        publication,
        total_bytes: bytes.len(),
        fragments,
    };
    Ok(bytes
        .chunks(limits.fragment_payload_bytes)
        .enumerate()
        .map(|(index, bytes)| ByteFragment {
            header,
            index,
            payload: bytes.into(),
        })
        .collect())
}

/// Proof that all bounded bytes of one encoded group arrived. Decoding is still
/// required: a byte-complete buffer alone never authorizes replica publication.
#[derive(Debug)]
pub struct CompleteGroupBytes {
    header: EncodedGroupHeader,
    bytes: Box<[u8]>,
}
impl CompleteGroupBytes {
    pub fn publication(&self) -> GroupPublication {
        self.header.publication
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
    /// The bounded codec must decode the complete manifest and all members.
    /// The decoded envelope uses one logical chunk; GroupAssembler independently
    /// validates scopes, revisions, cardinality and all-or-nothing publication.
    pub fn decode_and_publish<P: Payload>(
        self,
        groups: &mut GroupAssembler<P>,
        client: &mut ClientScopes<P>,
        now_ms: u64,
        decode: impl FnOnce(&[u8]) -> Result<GroupChunk<P>, ReplicationError>,
        validate: impl Fn(&P) -> bool,
    ) -> Result<GroupProgress, ReplicationError> {
        let decoded = decode(&self.bytes)?;
        if decoded.publication != self.header.publication
            || decoded.index != 0
            || decoded.count != 1
        {
            return Err(ReplicationError::Conflict);
        }
        groups.receive(decoded, now_ms, client, validate)
    }
}
#[derive(Debug)]
struct ByteStaging {
    chunks: Vec<Option<Box<[u8]>>>,
    started_ms: u64,
    bytes: usize,
}
#[derive(Debug)]
struct ByteFence {
    header: EncodedGroupHeader,
    complete: bool,
}

/// Bounded application reassembly. Payload bytes are counted exactly; fixed
/// metadata is bounded independently by fragments/incomplete/known_groups.
/// Final concatenation temporarily needs one additional group_bytes buffer.
#[derive(Debug)]
pub struct ByteAssembler {
    connection: ConnectionEpoch,
    limits: ByteLimits,
    assemblies: BTreeMap<GroupId, ByteStaging>,
    fences: BTreeMap<GroupId, ByteFence>,
    bytes: usize,
    now_ms: u64,
}
impl ByteAssembler {
    pub fn new(connection: ConnectionEpoch, limits: ByteLimits) -> Result<Self, ReplicationError> {
        if connection.0 == 0 {
            return Err(ReplicationError::InvalidIdentity);
        }
        Ok(Self {
            connection,
            limits: limits.validate()?,
            assemblies: BTreeMap::new(),
            fences: BTreeMap::new(),
            bytes: 0,
            now_ms: 0,
        })
    }
    pub fn staged_bytes(&self) -> usize {
        self.bytes
    }
    pub fn incomplete(&self) -> usize {
        self.assemblies.len()
    }
    pub fn publication(&self, group: GroupId) -> Option<GroupPublication> {
        self.fences
            .get(&group)
            .map(|fence| fence.header.publication)
    }
    pub fn expire(&mut self, now_ms: u64) -> Result<usize, ReplicationError> {
        if now_ms < self.now_ms {
            return Err(ReplicationError::Stale);
        }
        self.now_ms = now_ms;
        let before = self.assemblies.len();
        self.assemblies.retain(|_, assembly| {
            let keep = now_ms - assembly.started_ms < self.limits.max_age_ms;
            if !keep {
                self.bytes -= assembly.bytes;
            }
            keep
        });
        Ok(before - self.assemblies.len())
    }
    pub fn receive(
        &mut self,
        fragment: ByteFragment,
        now_ms: u64,
    ) -> Result<Option<CompleteGroupBytes>, ReplicationError> {
        self.expire(now_ms)?;
        let header = fragment.header;
        let key = header.publication;
        if key.connection != self.connection {
            return Err(ReplicationError::Stale);
        }
        if key.revision.0 == 0
            || key.snapshot.0 == 0
            || header.total_bytes == 0
            || header.total_bytes > self.limits.group_bytes
            || header.fragments == 0
            || header.fragments > self.limits.fragments
            || header.fragments
                != header
                    .total_bytes
                    .div_ceil(self.limits.fragment_payload_bytes)
            || fragment.index >= header.fragments
        {
            return Err(ReplicationError::InvalidIdentity);
        }
        let offset = fragment
            .index
            .checked_mul(self.limits.fragment_payload_bytes)
            .ok_or(ReplicationError::Capacity)?;
        let expected = (header.total_bytes - offset).min(self.limits.fragment_payload_bytes);
        if fragment.payload.len() != expected {
            return Err(ReplicationError::InvalidPayload);
        }
        let mut replace = false;
        let mut started_ms = now_ms;
        if let Some(fence) = self.fences.get(&key.group) {
            let old = fence.header.publication;
            if key.revision < old.revision
                || (key.revision == old.revision && key.snapshot < old.snapshot)
            {
                return Err(ReplicationError::Obsolete(ObsoleteReason::Publication));
            }
            if key == old && header != fence.header {
                return Err(ReplicationError::Conflict);
            }
            if !fence.complete && !self.assemblies.contains_key(&key.group) {
                return Err(ReplicationError::Expired);
            }
            if key != old {
                if let Some(assembly) = self.assemblies.get(&key.group) {
                    // Each publication is a complete full group, so the newer
                    // snapshot can replace partial older bytes. Preserve the
                    // first transfer deadline even when topology is unchanged.
                    started_ms = assembly.started_ms;
                }
                replace = true;
            }
        } else if self.fences.len() >= self.limits.known_groups {
            return Err(ReplicationError::ResetRequired);
        }
        if let Some(assembly) = self.assemblies.get(&key.group).filter(|_| !replace) {
            if let Some(previous) = &assembly.chunks[fragment.index] {
                return if **previous == *fragment.payload {
                    Ok(None)
                } else {
                    Err(ReplicationError::Conflict)
                };
            }
        } else if !self.assemblies.contains_key(&key.group)
            && self.assemblies.len() >= self.limits.incomplete
        {
            return Err(ReplicationError::Capacity);
        }
        let old_bytes = if replace {
            self.assemblies.get(&key.group).map_or(0, |a| a.bytes)
        } else {
            0
        };
        let bytes = self
            .bytes
            .checked_sub(old_bytes)
            .and_then(|n| n.checked_add(expected))
            .ok_or(ReplicationError::Capacity)?;
        if bytes > self.limits.staging_bytes {
            return Err(ReplicationError::Capacity);
        }
        if replace {
            self.assemblies.remove(&key.group);
        }
        let assembly = self
            .assemblies
            .entry(key.group)
            .or_insert_with(|| ByteStaging {
                chunks: (0..header.fragments).map(|_| None).collect(),
                started_ms,
                bytes: 0,
            });
        assembly.chunks[fragment.index] = Some(fragment.payload);
        assembly.bytes += expected;
        self.bytes = bytes;
        self.fences.insert(
            key.group,
            ByteFence {
                header,
                complete: false,
            },
        );
        if assembly.chunks.iter().any(Option::is_none) {
            return Ok(None);
        }
        let completed = self.assemblies.remove(&key.group).unwrap();
        self.bytes -= completed.bytes;
        let mut bytes = Vec::with_capacity(header.total_bytes);
        for part in completed.chunks.into_iter().flatten() {
            bytes.extend_from_slice(&part);
        }
        self.fences.get_mut(&key.group).unwrap().complete = true;
        Ok(Some(CompleteGroupBytes {
            header,
            bytes: bytes.into_boxed_slice(),
        }))
    }
    pub fn reset(&mut self, connection: ConnectionEpoch) -> Result<(), ReplicationError> {
        if connection <= self.connection {
            return Err(ReplicationError::Stale);
        }
        self.connection = connection;
        self.assemblies.clear();
        self.fences.clear();
        self.bytes = 0;
        self.now_ms = 0;
        Ok(())
    }
}

/// Fixed application header, excluding transport, encryption and IP overhead.
pub const BYTE_FRAGMENT_HEADER_BYTES: usize = 39;

impl ByteFragment {
    pub fn encode(&self, limits: ByteLimits) -> Result<Vec<u8>, ReplicationError> {
        let limits = limits.validate()?;
        let header = self.header;
        if header.publication.connection.0 == 0
            || header.publication.revision.0 == 0
            || header.publication.snapshot.0 == 0
            || header.total_bytes == 0
            || header.total_bytes > limits.group_bytes
            || header.fragments == 0
            || header.fragments > limits.fragments
            || header.fragments != header.total_bytes.div_ceil(limits.fragment_payload_bytes)
            || self.index >= header.fragments
        {
            return Err(ReplicationError::InvalidIdentity);
        }
        let expected = (header.total_bytes - self.index * limits.fragment_payload_bytes)
            .min(limits.fragment_payload_bytes);
        if self.payload.len() != expected {
            return Err(ReplicationError::InvalidPayload);
        }
        let total = u32::try_from(header.total_bytes).map_err(|_| ReplicationError::Capacity)?;
        let fragments = u16::try_from(header.fragments).map_err(|_| ReplicationError::Capacity)?;
        let index = u16::try_from(self.index).map_err(|_| ReplicationError::Capacity)?;
        let length = u16::try_from(self.payload.len()).map_err(|_| ReplicationError::Capacity)?;
        let mut out = Vec::with_capacity(BYTE_FRAGMENT_HEADER_BYTES + self.payload.len());
        out.push(1);
        out.extend_from_slice(&header.publication.connection.0.to_le_bytes());
        out.extend_from_slice(&header.publication.group.0.to_le_bytes());
        out.extend_from_slice(&header.publication.revision.0.to_le_bytes());
        out.extend_from_slice(&header.publication.snapshot.0.to_le_bytes());
        out.extend_from_slice(&total.to_le_bytes());
        out.extend_from_slice(&fragments.to_le_bytes());
        out.extend_from_slice(&index.to_le_bytes());
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&self.payload);
        Ok(out)
    }
    /// Counts and byte lengths are checked before the only payload allocation.
    /// The datagram decoder never allocates a remotely declared Vec length.
    pub fn decode(bytes: &[u8], limits: ByteLimits) -> Result<Self, ReplicationError> {
        let limits = limits.validate()?;
        if bytes.len() < BYTE_FRAGMENT_HEADER_BYTES
            || bytes.len() - BYTE_FRAGMENT_HEADER_BYTES > limits.fragment_payload_bytes
            || bytes[0] != 1
        {
            return Err(ReplicationError::InvalidPayload);
        }
        let read64 = |offset| u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
        let read32 = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let read16 =
            |offset| u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap()) as usize;
        let publication = GroupPublication {
            connection: ConnectionEpoch(read64(1)),
            group: GroupId(read32(9)),
            revision: GroupRevision(read64(13)),
            snapshot: SnapshotId(read64(21)),
        };
        let total_bytes = read32(29) as usize;
        let fragments = read16(33);
        let index = read16(35);
        let length = read16(37);
        if publication.connection.0 == 0
            || publication.revision.0 == 0
            || publication.snapshot.0 == 0
            || total_bytes == 0
            || total_bytes > limits.group_bytes
            || fragments == 0
            || fragments > limits.fragments
            || fragments != total_bytes.div_ceil(limits.fragment_payload_bytes)
            || index >= fragments
        {
            return Err(ReplicationError::InvalidIdentity);
        }
        let expected = (total_bytes - index * limits.fragment_payload_bytes)
            .min(limits.fragment_payload_bytes);
        if length != expected || length != bytes.len() - BYTE_FRAGMENT_HEADER_BYTES {
            return Err(ReplicationError::InvalidPayload);
        }
        Ok(Self {
            header: EncodedGroupHeader {
                publication,
                total_bytes,
                fragments,
            },
            index,
            payload: bytes[BYTE_FRAGMENT_HEADER_BYTES..].into(),
        })
    }
}
