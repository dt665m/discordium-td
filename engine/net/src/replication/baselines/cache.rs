use super::*;
use crate::replication::ObsoleteReason;

#[derive(Debug, Clone)]
struct Entry<P> {
    state: BaselineState<P>,
    decoded: bool,
}
#[derive(Debug)]
struct Cache<P> {
    context: BaselineContext,
    generation: BaselineGeneration,
    limits: BaselineLimits,
    entries: Vec<Entry<P>>,
    bytes: usize,
    retired: SnapshotId,
    latest: Option<BaselineReceipt>,
    closed: bool,
}
// Vec::clone may shrink spare capacity; preserve it so future inserts cannot
// grow beyond the negotiated metadata allocation after atomic group staging.
impl<P: Clone> Clone for Cache<P> {
    fn clone(&self) -> Self {
        let mut entries = Vec::with_capacity(self.entries.capacity());
        entries.extend(self.entries.iter().cloned());
        Self {
            context: self.context,
            generation: self.generation,
            limits: self.limits,
            entries,
            bytes: self.bytes,
            retired: self.retired,
            latest: self.latest,
            closed: self.closed,
        }
    }
}
impl<P: Payload> Cache<P> {
    fn new(
        context: BaselineContext,
        generation: BaselineGeneration,
        limits: BaselineLimits,
    ) -> Result<Self, ReplicationError> {
        context.validate()?;
        if generation.0 == 0 {
            return Err(ReplicationError::InvalidIdentity);
        }
        let metadata = limits
            .slots
            .checked_mul(std::mem::size_of::<Entry<P>>())
            .and_then(|n| n.checked_add(std::mem::size_of::<ClientBaselines<P>>()))
            .ok_or(ReplicationError::InvalidConfiguration)?;
        let minimum = metadata
            .checked_add(limits.payload_bytes)
            .ok_or(ReplicationError::InvalidConfiguration)?;
        let peak = limits
            .resident_bytes
            .checked_add(limits.payload_bytes)
            .and_then(|n| n.checked_add(limits.packet_bytes))
            .ok_or(ReplicationError::InvalidConfiguration)?;
        if limits.slots < 2
            || limits.payload_bytes == 0
            || limits.packet_bytes == 0
            || limits.resident_bytes < minimum
            || limits.peak_bytes < peak
            || limits.repair_interval_ms == 0
        {
            return Err(ReplicationError::InvalidConfiguration);
        }
        let entries = Vec::with_capacity(limits.slots);
        // Account actual capacity rather than assuming the allocator's request.
        let bytes = entries
            .capacity()
            .checked_mul(std::mem::size_of::<Entry<P>>())
            .and_then(|n| n.checked_add(std::mem::size_of::<ClientBaselines<P>>()))
            .ok_or(ReplicationError::Capacity)?;
        if bytes
            .checked_add(limits.payload_bytes)
            .is_none_or(|n| n > limits.resident_bytes)
        {
            return Err(ReplicationError::Capacity);
        }
        Ok(Self {
            context,
            generation,
            limits,
            entries,
            bytes,
            retired: SnapshotId(0),
            latest: None,
            closed: false,
        })
    }
    fn envelope(
        &self,
        context: BaselineContext,
        generation: BaselineGeneration,
    ) -> Result<(), ReplicationError> {
        if self.closed || context != self.context || generation != self.generation {
            return Err(ReplicationError::Stale);
        }
        Ok(())
    }
    fn identity(&self, receipt: BaselineReceipt) -> Result<(), ReplicationError> {
        self.envelope(receipt.context, receipt.generation)?;
        if receipt.snapshot.0 == 0 || receipt.version.0 == 0 {
            return Err(ReplicationError::InvalidIdentity);
        }
        if receipt.snapshot <= self.retired {
            return Err(ReplicationError::Obsolete(ObsoleteReason::RetiredBaseline));
        }
        Ok(())
    }
    fn entry(&self, receipt: BaselineReceipt) -> Result<&Entry<P>, ReplicationError> {
        self.identity(receipt)?;
        let entry = self
            .entries
            .iter()
            .find(|e| e.state.receipt.snapshot == receipt.snapshot)
            .ok_or(ReplicationError::Unknown)?;
        if entry.state.receipt != receipt {
            return Err(ReplicationError::Conflict);
        }
        Ok(entry)
    }
    fn check(&self, state: &BaselineState<P>) -> Result<bool, ReplicationError> {
        self.identity(state.receipt)?;
        let size = state.payload.retained_bytes();
        if size > self.limits.payload_bytes {
            return Err(ReplicationError::Capacity);
        }
        if let Some(entry) = self
            .entries
            .iter()
            .find(|e| e.state.receipt.snapshot == state.receipt.snapshot)
        {
            return if entry.state == *state {
                Ok(false)
            } else {
                Err(ReplicationError::Conflict)
            };
        }
        if let Some(latest) = self.latest {
            if state.receipt.version == latest.version
                && self
                    .entries
                    .last()
                    .is_some_and(|e| e.state.payload != state.payload)
            {
                return Err(ReplicationError::Conflict);
            }
            if state.receipt.snapshot < latest.snapshot
                && state.receipt.version <= latest.version
                && state.receipt.end_tick <= latest.end_tick
            {
                return Err(ReplicationError::Obsolete(ObsoleteReason::Publication));
            }
            if state.receipt.snapshot <= latest.snapshot
                || state.receipt.version < latest.version
                || state.receipt.end_tick < latest.end_tick
            {
                return Err(ReplicationError::Conflict);
            }
        }
        if self.entries.len() >= self.limits.slots
            || self
                .bytes
                .checked_add(size)
                .is_none_or(|n| n > self.limits.resident_bytes)
        {
            return Err(ReplicationError::Capacity);
        }
        Ok(true)
    }
    fn insert(&mut self, state: BaselineState<P>, decoded: bool) {
        self.bytes += state.payload.retained_bytes();
        self.latest = Some(state.receipt);
        self.entries.push(Entry { state, decoded });
    }
    fn retirement(&self, fence: BaselineRetirement) -> Result<(), ReplicationError> {
        self.envelope(fence.context, fence.generation)?;
        if fence.through.0 == 0 {
            return Err(ReplicationError::InvalidIdentity);
        }
        if fence.through <= self.retired {
            return Ok(());
        }
        // Retain at least one decoded usable replacement. No cross-lane ordering
        // is assumed: delayed deltas against the prefix can still be rejected.
        if !self
            .entries
            .iter()
            .any(|e| e.decoded && e.state.receipt.snapshot > fence.through)
        {
            return Err(ReplicationError::Incomplete);
        }
        Ok(())
    }
    fn discard(&mut self, through: SnapshotId) {
        if through <= self.retired {
            return;
        }
        self.entries.retain(|e| {
            if e.state.receipt.snapshot <= through {
                self.bytes -= e.state.payload.retained_bytes();
                false
            } else {
                true
            }
        });
        self.retired = through;
    }
    fn reset(&mut self, reset: BaselineReset) -> Result<(), ReplicationError> {
        if self.closed || reset.context != self.context || reset.generation < self.generation {
            return Err(ReplicationError::Stale);
        }
        if reset.generation == self.generation {
            return Ok(());
        }
        self.clear();
        self.generation = reset.generation;
        // Keep latest snapshot/tick/version high water, even across reset.
        Ok(())
    }
    fn clear(&mut self) {
        for e in self.entries.drain(..) {
            self.bytes -= e.state.payload.retained_bytes();
        }
    }
    fn close(&mut self) {
        self.clear();
        self.closed = true;
    }
}

/// Server-side sent-state cache. No entry becomes a delta base until its exact
/// decoded-retention receipt arrives. Selection/failed transport never inserts.
#[derive(Debug)]
pub struct ServerBaselines<P> {
    cache: Cache<P>,
}
impl<P: Payload> ServerBaselines<P> {
    pub fn new(
        context: BaselineContext,
        generation: BaselineGeneration,
        limits: BaselineLimits,
    ) -> Result<Self, ReplicationError> {
        Ok(Self {
            cache: Cache::new(context, generation, limits)?,
        })
    }
    pub fn retained_bytes(&self) -> usize {
        self.cache.bytes
    }
    pub fn retained_states(&self) -> usize {
        self.cache.entries.len()
    }
    pub fn generation(&self) -> BaselineGeneration {
        self.cache.generation
    }
    pub fn newest_decoded(&self) -> Option<BaselineReceipt> {
        self.cache
            .entries
            .iter()
            .rev()
            .find(|e| e.decoded)
            .map(|e| e.state.receipt)
    }
    /// `target` must be current positively projected authorized state. `send`
    /// must synchronously recheck graph/policy admission and actual framed budget.
    /// The packet is borrowed so this cache never leaves an unsent stale queue.
    /// `None` explicitly selects a keyframe. A delta which is not smaller uses a
    /// full encoding; unsupported/invalid codec errors are never hidden.
    pub fn transmit(
        &mut self,
        target: BaselineState<P>,
        base: Option<BaselineReceipt>,
        codec: &impl BaselineCodec<P>,
        send: impl FnOnce(&BaselinePacket) -> bool,
    ) -> Result<bool, ReplicationError> {
        let insert = self.cache.check(&target)?;
        if !codec.validate(&target.payload) {
            return Err(ReplicationError::InvalidPayload);
        }
        let (encoding, bytes) = if let Some(base) = base {
            let entry = self.cache.entry(base)?;
            if !entry.decoded {
                return Err(ReplicationError::Unknown);
            }
            if base.snapshot >= target.receipt.snapshot {
                return Err(ReplicationError::InvalidIdentity);
            }
            // Measure canonical full bytes, not P's heap capacity. Drop the
            // scratch before encoding a delta to keep a single-packet peak.
            let full = codec.encode_full(&target.payload, self.cache.limits.packet_bytes)?;
            if full.capacity() > self.cache.limits.packet_bytes {
                return Err(ReplicationError::Capacity);
            }
            let full_len = full.len();
            drop(full);
            let delta = codec.encode_delta(
                &entry.state.payload,
                &target.payload,
                self.cache.limits.packet_bytes,
            );
            match delta {
                Ok(delta) if delta.capacity() > self.cache.limits.packet_bytes => {
                    return Err(ReplicationError::Capacity);
                }
                Ok(delta) if delta.len() < full_len => (BaselineEncoding::Delta { base }, delta),
                Ok(delta) => {
                    drop(delta);
                    (
                        BaselineEncoding::Full,
                        codec.encode_full(&target.payload, self.cache.limits.packet_bytes)?,
                    )
                }
                Err(ReplicationError::Capacity) => (
                    BaselineEncoding::Full,
                    codec.encode_full(&target.payload, self.cache.limits.packet_bytes)?,
                ),
                Err(error) => return Err(error),
            }
        } else {
            (
                BaselineEncoding::Full,
                codec.encode_full(&target.payload, self.cache.limits.packet_bytes)?,
            )
        };
        if bytes.capacity() > self.cache.limits.packet_bytes {
            return Err(ReplicationError::Capacity);
        }
        let packet = BaselinePacket {
            target: target.receipt,
            encoding,
            bytes,
        };
        if !send(&packet) {
            return Ok(false);
        }
        if insert {
            self.cache.insert(target, false);
        }
        Ok(true)
    }
    /// Check an exact sent proof without changing decoded retention. Atomic
    /// groups must preflight every member before acknowledging any member.
    pub fn validate_acknowledgment(
        &self,
        receipt: BaselineReceipt,
    ) -> Result<(), ReplicationError> {
        self.cache.entry(receipt).map(|_| ())
    }
    pub fn acknowledge(&mut self, receipt: BaselineReceipt) -> Result<(), ReplicationError> {
        self.validate_acknowledgment(receipt)?;
        self.cache
            .entries
            .iter_mut()
            .find(|e| e.state.receipt == receipt)
            .unwrap()
            .decoded = true;
        Ok(())
    }
    /// Check a cumulative fence without discarding retained state. Group callers
    /// must validate every fence before retiring any member.
    pub fn validate_retirement(&self, request: BaselineRetirement) -> Result<(), ReplicationError> {
        self.cache.retirement(request)
    }
    /// Stops all future use of the retired prefix before returning the fence to
    /// send reliably. The caller can retry the same request after fence loss.
    pub fn retire(
        &mut self,
        request: BaselineRetirement,
    ) -> Result<BaselineRetirement, ReplicationError> {
        self.validate_retirement(request)?;
        self.cache.discard(request.through);
        Ok(request)
    }
    /// Clears sent/decoded bases before returning the reliable reset control.
    /// Retransmit this control until acknowledged; only a full state can seed the
    /// new generation. Do not repeatedly advance generation just because of loss.
    pub fn begin_reset(&mut self) -> Result<BaselineReset, ReplicationError> {
        let generation = self
            .cache
            .generation
            .checked_next()
            .ok_or(ReplicationError::ResetRequired)?;
        let reset = BaselineReset {
            context: self.cache.context,
            generation,
        };
        self.cache.reset(reset)?;
        Ok(reset)
    }
    /// Sticky fail-closed fence for policy revocation/exit. A new scope needs a
    /// new instance; no reset can reopen this representation.
    pub fn close(&mut self) {
        self.cache.close();
    }
}

/// Client retained baselines. Clone is provided for outer atomic group staging;
/// charge that clone against the caller's group staging budget before creating it.
#[derive(Debug, Clone)]
pub struct ClientBaselines<P> {
    cache: Cache<P>,
    proposed: Option<BaselineRetirement>,
    repair_pending: bool,
    last_repair_ms: Option<u64>,
}
impl<P: Payload> ClientBaselines<P> {
    pub fn new(
        context: BaselineContext,
        generation: BaselineGeneration,
        limits: BaselineLimits,
    ) -> Result<Self, ReplicationError> {
        Ok(Self {
            cache: Cache::new(context, generation, limits)?,
            proposed: None,
            repair_pending: false,
            last_repair_ms: None,
        })
    }
    pub fn retained_bytes(&self) -> usize {
        self.cache.bytes
    }
    pub fn retained_states(&self) -> usize {
        self.cache.entries.len()
    }
    pub fn current(&self) -> Option<&BaselineState<P>> {
        self.cache.entries.last().map(|e| &e.state)
    }
    pub fn generation(&self) -> BaselineGeneration {
        self.cache.generation
    }
    /// Return the exact retained target, including an older duplicate. Never
    /// substitute the newest state for a different receipt during group staging.
    pub fn state(&self, receipt: BaselineReceipt) -> Result<&BaselineState<P>, ReplicationError> {
        self.cache.entry(receipt).map(|entry| &entry.state)
    }
    /// Decode and validate without modifying retained/published state on failure.
    /// For groups call only on bounded staging clones until every member passes.
    pub fn receive(
        &mut self,
        packet: &BaselinePacket,
        codec: &impl BaselineCodec<P>,
        validate: impl FnOnce(&P) -> bool,
    ) -> Result<BaselineReceipt, ReplicationError> {
        if packet.bytes.capacity() > self.cache.limits.packet_bytes {
            return Err(ReplicationError::Capacity);
        }
        packet.target.context.validate()?;
        if packet.target.generation.0 == 0
            || packet.target.snapshot.0 == 0
            || packet.target.version.0 == 0
        {
            return Err(ReplicationError::InvalidIdentity);
        }
        // Validate delta identity independently of whether its target or base
        // has already retired; malformed envelopes remain hard rejections.
        if let BaselineEncoding::Delta { base } = packet.encoding {
            if base.context != packet.target.context
                || base.generation != packet.target.generation
                || base.snapshot.0 == 0
                || base.version.0 == 0
                || base.snapshot >= packet.target.snapshot
                || base.version > packet.target.version
                || base.end_tick > packet.target.end_tick
            {
                return Err(ReplicationError::InvalidIdentity);
            }
        }
        // Snapshot order must agree with the committed metadata even when the
        // referenced base has retired. A missing old base cannot excuse a
        // contradictory new publication.
        if let Some(latest) = self.cache.latest {
            if (packet.target.snapshot < latest.snapshot
                && (packet.target.version > latest.version
                    || packet.target.end_tick > latest.end_tick))
                || (packet.target.snapshot > latest.snapshot
                    && (packet.target.version < latest.version
                        || packet.target.end_tick < latest.end_tick))
                || (packet.target.snapshot == latest.snapshot
                    && (packet.target.version != latest.version
                        || packet.target.end_tick != latest.end_tick))
            {
                return Err(ReplicationError::Conflict);
            }
        }
        if !self.cache.closed
            && packet.target.context == self.cache.context
            && packet.target.generation < self.cache.generation
        {
            if matches!(packet.encoding, BaselineEncoding::Full) {
                let payload = codec.decode_full(&packet.bytes, self.cache.limits.payload_bytes)?;
                if !codec.validate(&payload) || !validate(&payload) {
                    return Err(ReplicationError::InvalidPayload);
                }
            }
            return Err(ReplicationError::Obsolete(ObsoleteReason::RetiredBaseline));
        }
        if !self.cache.closed
            && packet.target.context == self.cache.context
            && self.cache.generation.checked_next() == Some(packet.target.generation)
            && matches!(packet.encoding, BaselineEncoding::Full)
        {
            let payload = codec.decode_full(&packet.bytes, self.cache.limits.payload_bytes)?;
            if !codec.validate(&payload) || !validate(&payload) {
                return Err(ReplicationError::InvalidPayload);
            }
            return Err(ReplicationError::BaselineResetPending);
        }
        if let Err(error) = self.cache.identity(packet.target) {
            if matches!(error, ReplicationError::Obsolete(_))
                && matches!(packet.encoding, BaselineEncoding::Full)
            {
                let payload = codec.decode_full(&packet.bytes, self.cache.limits.payload_bytes)?;
                if !codec.validate(&payload) || !validate(&payload) {
                    return Err(ReplicationError::InvalidPayload);
                }
            }
            return Err(error);
        }
        if self
            .cache
            .latest
            .is_some_and(|r| packet.target.snapshot < r.snapshot)
            && !self
                .cache
                .entries
                .iter()
                .any(|e| e.state.receipt.snapshot == packet.target.snapshot)
        {
            let latest = self.cache.latest.unwrap();
            if packet.target.version > latest.version || packet.target.end_tick > latest.end_tick {
                return Err(ReplicationError::Conflict);
            }
            // A full payload can still be validated after it is superseded.
            // Delta decoding may require a base whose retention promise ended.
            if matches!(packet.encoding, BaselineEncoding::Full) {
                let payload = codec.decode_full(&packet.bytes, self.cache.limits.payload_bytes)?;
                if !codec.validate(&payload) || !validate(&payload) {
                    return Err(ReplicationError::InvalidPayload);
                }
            }
            return Err(ReplicationError::Obsolete(ObsoleteReason::Publication));
        }
        let payload = match packet.encoding {
            BaselineEncoding::Full => {
                codec.decode_full(&packet.bytes, self.cache.limits.payload_bytes)?
            }
            BaselineEncoding::Delta { base } => {
                if base.context != packet.target.context
                    || base.generation != packet.target.generation
                    || base.snapshot >= packet.target.snapshot
                {
                    return Err(ReplicationError::InvalidIdentity);
                }
                let entry = match self.cache.entry(base) {
                    Ok(entry) => entry,
                    Err(error) => {
                        if matches!(error, ReplicationError::Obsolete(_)) {
                            return Err(error);
                        }
                        if self
                            .cache
                            .latest
                            .is_none_or(|r| packet.target.snapshot > r.snapshot)
                        {
                            self.repair_pending = true;
                        }
                        return Err(error);
                    }
                };
                codec.decode_delta(
                    &entry.state.payload,
                    &packet.bytes,
                    self.cache.limits.payload_bytes,
                )?
            }
        };
        if !codec.validate(&payload) || !validate(&payload) {
            return Err(ReplicationError::InvalidPayload);
        }
        let state = BaselineState {
            receipt: packet.target,
            payload,
        };
        let insert = self.cache.check(&state)?;
        if insert {
            self.cache.insert(state, true);
        }
        if matches!(packet.encoding, BaselineEncoding::Full) {
            self.repair_pending = false;
        }
        Ok(packet.target)
    }
    /// Proposal retains all promised states until the matching server fence.
    /// At most one cumulative request is outstanding; repeat it after loss.
    pub fn propose_retirement(
        &mut self,
        through: SnapshotId,
    ) -> Result<BaselineRetirement, ReplicationError> {
        let request = BaselineRetirement {
            context: self.cache.context,
            generation: self.cache.generation,
            through,
        };
        self.cache.retirement(request)?;
        // An already acknowledged prefix has no outstanding promise. Do not
        // install it as a proposal: its duplicate ACK returns at the watermark.
        if through <= self.cache.retired {
            return Err(ReplicationError::Stale);
        }
        if self.proposed.is_some_and(|old| old != request) {
            return Err(ReplicationError::Incomplete);
        }
        self.proposed = Some(request);
        Ok(request)
    }
    pub fn acknowledge_retirement(
        &mut self,
        fence: BaselineRetirement,
    ) -> Result<(), ReplicationError> {
        self.cache.envelope(fence.context, fence.generation)?;
        if fence.through <= self.cache.retired {
            return Ok(());
        }
        if self.proposed != Some(fence) {
            return Err(ReplicationError::Unknown);
        }
        self.cache.retirement(fence)?;
        self.cache.discard(fence.through);
        self.proposed = None;
        Ok(())
    }
    /// Explicit baseline loss uses reset, never silent eviction. Until reset is
    /// received, retaining promised states remains mandatory.
    pub fn request_reset(&mut self) {
        self.repair_pending = true;
    }
    pub fn repair_request(&mut self, now_ms: u64) -> Option<BaselineRepair> {
        if self.cache.closed
            || !self.repair_pending
            || self.last_repair_ms.is_some_and(|last| {
                now_ms < last.saturating_add(self.cache.limits.repair_interval_ms)
            })
        {
            return None;
        }
        self.last_repair_ms = Some(now_ms);
        Some(BaselineRepair {
            context: self.cache.context,
            generation: self.cache.generation,
        })
    }
    pub fn apply_reset(&mut self, reset: BaselineReset) -> Result<(), ReplicationError> {
        let changed = reset.generation != self.cache.generation;
        self.cache.reset(reset)?;
        if changed {
            self.proposed = None;
            self.repair_pending = true;
        }
        Ok(())
    }
    pub fn close(&mut self) {
        self.cache.close();
        self.proposed = None;
        self.repair_pending = false;
    }
}
