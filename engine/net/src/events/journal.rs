use super::*;
use crate::{replication::Payload, types::*};
use std::mem::size_of;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Lifetime {
    expires: ServerTick,
    delivery: Delivery,
}
#[derive(Debug, Clone)]
struct Entry<P> {
    identity: EventIdentity,
    lifetime: Option<Lifetime>,
    payload: Option<P>,
    phase: Phase,
    decision: Decision,
    delivered: bool,
    binding: Option<EntityId>,
}
#[derive(Debug, Clone, Copy)]
struct Binding {
    entity: EntityId,
    key: EventKey,
}
/// Fixed-capacity sorted vectors make owned metadata capacity measurable. Their
/// bounded insert/lookup work never depends on a game entity or rendering API.
/// Payload implementations and validation callbacks are trusted, pure game code;
/// their Clone must not increase retained capacity. Decode wire data separately.
pub struct EventJournal<P> {
    scope: JournalScope,
    limits: Limits,
    now: ServerTick,
    retired: CommandSeq,
    entries: Vec<Entry<P>>,
    bindings: Vec<Binding>,
    rejected_actions: Vec<ActionKey>,
}
fn reserved<T>(capacity: usize) -> Result<Vec<T>, EventError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|_| EventError::Capacity)?;
    Ok(values)
}
impl<P: Payload> EventJournal<P> {
    pub fn new(scope: JournalScope, limits: Limits) -> Result<Self, EventError> {
        if scope.connection.0 == 0 || scope.stream.0 == 0 {
            return Err(EventError::InvalidIdentity);
        }
        let metadata = limits
            .records
            .checked_mul(size_of::<Entry<P>>())
            .and_then(|n| n.checked_add(limits.bindings.checked_mul(size_of::<Binding>())?))
            .and_then(|n| n.checked_add(limits.action_fences.checked_mul(size_of::<ActionKey>())?));
        let output = limits.transitions.checked_mul(size_of::<Change<P>>());
        let peak = limits
            .resident_bytes
            .checked_mul(2)
            .and_then(|n| n.checked_add(limits.transition_bytes))
            .and_then(|n| n.checked_add(limits.payload_bytes));
        if [
            limits.records,
            limits.bindings,
            limits.action_fences,
            limits.payload_bytes,
            limits.transitions,
        ]
        .contains(&0)
            || limits.history_ticks == 0
            || limits.lifetime_ticks == 0
            || metadata.is_none_or(|n| {
                n > limits.resident_bytes || limits.payload_bytes > limits.resident_bytes - n
            })
            || output.is_none_or(|n| {
                n > limits.transition_bytes || limits.payload_bytes > limits.transition_bytes - n
            })
            || peak.is_none_or(|n| n > limits.peak_bytes)
        {
            return Err(EventError::InvalidConfiguration);
        }
        Ok(Self {
            scope,
            limits,
            now: ServerTick(0),
            retired: CommandSeq(0),
            entries: reserved(limits.records)?,
            bindings: reserved(limits.bindings)?,
            rejected_actions: reserved(limits.action_fences)?,
        })
    }
    pub fn scope(&self) -> JournalScope {
        self.scope
    }
    pub fn now(&self) -> ServerTick {
        self.now
    }
    pub fn records(&self) -> usize {
        self.entries.len()
    }
    pub fn binding_fences(&self) -> usize {
        self.bindings.len()
    }
    pub fn action_fences(&self) -> usize {
        self.rejected_actions.len()
    }
    pub fn retired_through(&self) -> CommandSeq {
        self.retired
    }
    pub fn retained_bytes(&self) -> usize {
        self.entries.capacity() * size_of::<Entry<P>>()
            + self.bindings.capacity() * size_of::<Binding>()
            + self.rejected_actions.capacity() * size_of::<ActionKey>()
            + self
                .entries
                .iter()
                .filter_map(|entry| entry.payload.as_ref())
                .map(Payload::retained_bytes)
                .sum::<usize>()
    }
    /// Bounded retained records, including tombstones needed for late bindings.
    pub fn iter(&self) -> impl Iterator<Item = EventView<'_, P>> {
        self.entries.iter().map(|entry| EventView {
            identity: entry.identity,
            phase: entry.phase,
            decision: entry.decision,
            binding: entry.binding,
            payload: entry.payload.as_ref(),
        })
    }
    pub fn get(&self, key: EventKey) -> Option<EventView<'_, P>> {
        let entry = &self.entries[self.find(key).ok()?];
        Some(EventView {
            identity: entry.identity,
            phase: entry.phase,
            decision: entry.decision,
            binding: entry.binding,
            payload: entry.payload.as_ref(),
        })
    }
    fn find(&self, key: EventKey) -> Result<usize, usize> {
        self.entries
            .binary_search_by_key(&key, |entry| entry.identity.key)
    }
    fn valid_action(&self, action: ActionKey) -> Result<(), EventError> {
        if action.connection != self.scope.connection || action.stream != self.scope.stream {
            return Err(EventError::WrongScope);
        }
        if action.command.0 == 0 {
            return Err(EventError::InvalidIdentity);
        }
        if action.command <= self.retired {
            return Err(EventError::Stale);
        }
        Ok(())
    }
    fn valid_identity(&self, identity: EventIdentity) -> Result<(), EventError> {
        self.valid_action(identity.key.action)?;
        if identity.key.kind.0 == 0 || identity.schema.0 == 0 || identity.origin_tick > self.now {
            return Err(EventError::InvalidIdentity);
        }
        if let Ok(i) = self.find(identity.key) {
            if self.entries[i].identity != identity {
                return Err(EventError::Conflict);
            }
        }
        Ok(())
    }
    fn check_resident(&self) -> Result<(), EventError> {
        if self.retained_bytes() > self.limits.resident_bytes {
            Err(EventError::Capacity)
        } else {
            Ok(())
        }
    }
    fn changes(&self) -> Result<Changes<P>, EventError> {
        Ok(Changes {
            values: reserved(self.limits.transitions)?,
            payload_bytes: 0,
        })
    }
    fn emit(&self, changes: &mut Changes<P>, change: Change<P>) -> Result<(), EventError> {
        let bytes = match &change {
            Change::Create { event, .. } | Change::Update(event) => event.payload.retained_bytes(),
            _ => 0,
        };
        if changes.values.len() >= self.limits.transitions
            || changes
                .retained_bytes()
                .checked_add(bytes)
                .is_none_or(|n| n > self.limits.transition_bytes)
        {
            return Err(EventError::Capacity);
        }
        changes.payload_bytes += bytes;
        changes.values.push(change);
        Ok(())
    }
    fn transaction(
        &mut self,
        apply: impl FnOnce(&mut Self, &mut Changes<P>) -> Result<(), EventError>,
    ) -> Result<Changes<P>, EventError> {
        let mut staged = Self::new(self.scope, self.limits)?;
        staged.now = self.now;
        staged.retired = self.retired;
        staged.entries.extend(self.entries.iter().cloned());
        staged.bindings.extend_from_slice(&self.bindings);
        staged
            .rejected_actions
            .extend_from_slice(&self.rejected_actions);
        staged.check_resident()?;
        let mut changes = self.changes()?;
        apply(&mut staged, &mut changes)?;
        staged.check_resident()?;
        *self = staged;
        Ok(changes)
    }
    fn set_now(&mut self, now: ServerTick) -> Result<(), EventError> {
        if now < self.now {
            return Err(EventError::Stale);
        }
        self.now = now;
        Ok(())
    }
    fn entry(&mut self, identity: EventIdentity) -> Result<usize, EventError> {
        self.valid_identity(identity)?;
        match self.find(identity.key) {
            Ok(i) => Ok(i),
            Err(i) => {
                if self.entries.len() >= self.limits.records {
                    return Err(EventError::ResetRequired);
                }
                self.entries.insert(
                    i,
                    Entry {
                        identity,
                        lifetime: None,
                        payload: None,
                        phase: Phase::Unseen,
                        decision: Decision::Unresolved,
                        delivered: false,
                        binding: None,
                    },
                );
                Ok(i)
            }
        }
    }
    fn cancel(
        &mut self,
        i: usize,
        phase: Phase,
        reason: CancelReason,
        changes: &mut Changes<P>,
    ) -> Result<(), EventError> {
        let entry = &self.entries[i];
        if entry.phase == Phase::Live
            && entry
                .lifetime
                .is_some_and(|life| life.delivery != Delivery::OneShot)
        {
            self.emit(
                changes,
                Change::Cancel {
                    key: entry.identity.key,
                    reason,
                },
            )?;
        }
        self.entries[i].phase = phase;
        self.entries[i].payload = None;
        Ok(())
    }
    fn advance_inner(
        &mut self,
        now: ServerTick,
        changes: &mut Changes<P>,
    ) -> Result<(), EventError> {
        self.set_now(now)?;
        for i in 0..self.entries.len() {
            let entry = &self.entries[i];
            if entry.decision != Decision::Accepted
                && matches!(entry.phase, Phase::Live | Phase::ReplayAbsent)
                && entry.lifetime.is_some_and(|life| {
                    life.delivery != Delivery::SimulationOwned && life.expires <= now
                })
            {
                self.cancel(i, Phase::Expired, CancelReason::Expired, changes)?;
            }
        }
        Ok(())
    }
    /// Expire unconfirmed predictions. Confirmed events survive provisional
    /// replay and time advancement; their authority calls `expire` explicitly.
    pub fn advance(&mut self, now: ServerTick) -> Result<Changes<P>, EventError> {
        self.transaction(|staged, changes| staged.advance_inner(now, changes))
    }
    /// The range refers to event origin ticks, not render frames. Events outside
    /// it remain untouched; corrections beyond retained history require resync.
    pub fn reconcile(
        &mut self,
        now: ServerTick,
        range: ReplayRange,
        corrected: &[EventRecord<P>],
        validate: impl Fn(&P) -> bool,
    ) -> Result<Changes<P>, EventError> {
        self.transaction(|staged, changes| {
            staged.advance_inner(now, changes)?;
            if range.first > range.last || range.last > now {
                return Err(EventError::InvalidIdentity);
            }
            if now.0 - range.first.0 >= staged.limits.history_ticks {
                return Err(EventError::OutsideHistory);
            }
            if corrected.len() > staged.limits.records {
                return Err(EventError::Capacity);
            }
            // Bounded O(n²) duplicate checks avoid an uncharged staging index.
            for (n, event) in corrected.iter().enumerate() {
                staged.valid_identity(event.identity)?;
                if corrected[..n]
                    .iter()
                    .any(|old| old.identity.key == event.identity.key)
                {
                    return Err(EventError::DuplicateKey);
                }
                if event.identity.origin_tick < range.first
                    || event.identity.origin_tick > range.last
                    || event.expires_at <= event.identity.origin_tick
                    || event.expires_at.0 - event.identity.origin_tick.0
                        > staged.limits.lifetime_ticks
                {
                    return Err(EventError::InvalidIdentity);
                }
                if event.payload.retained_bytes() > staged.limits.payload_bytes
                    || !validate(&event.payload)
                {
                    return Err(EventError::InvalidPayload);
                }
                let i = staged.entry(event.identity)?;
                let life = Lifetime {
                    expires: event.expires_at,
                    delivery: event.delivery,
                };
                if staged.entries[i].lifetime.is_some_and(|old| old != life) {
                    return Err(EventError::Conflict);
                }
                staged.entries[i].lifetime = Some(life);
                if staged
                    .rejected_actions
                    .binary_search(&event.identity.key.action)
                    .is_ok()
                {
                    staged.entries[i].decision = Decision::Rejected;
                    staged.cancel(i, Phase::Rejected, CancelReason::Rejected, changes)?;
                    continue;
                }
                let entry = &staged.entries[i];
                if matches!(entry.phase, Phase::Expired | Phase::Rejected)
                    || (entry.phase == Phase::ReplayAbsent && entry.decision == Decision::Accepted)
                {
                    continue;
                }
                if event.delivery != Delivery::SimulationOwned
                    && event.expires_at <= now
                    && entry.phase != Phase::Live
                {
                    staged.cancel(i, Phase::Expired, CancelReason::Expired, changes)?;
                    continue;
                }
                let was_live = entry.phase == Phase::Live;
                let create = entry.phase != Phase::Live
                    && (!entry.delivered || event.delivery != Delivery::OneShot);
                let changed = entry.payload.as_ref() != Some(&event.payload);
                let committed = entry.decision == Decision::Accepted;
                let binding = entry.binding;
                staged.entries[i].payload = Some(event.payload.clone());
                staged.entries[i].phase = Phase::Live;
                staged.entries[i].delivered = true;
                staged.check_resident()?;
                if create {
                    staged.emit(
                        changes,
                        Change::Create {
                            event: event.clone(),
                            committed,
                            binding,
                        },
                    )?;
                } else if changed && (event.delivery != Delivery::OneShot || was_live) {
                    staged.emit(changes, Change::Update(event.clone()))?;
                }
            }
            for i in 0..staged.entries.len() {
                let entry = &staged.entries[i];
                if entry.phase == Phase::Live
                    && entry.decision != Decision::Accepted
                    && (range.first..=range.last).contains(&entry.identity.origin_tick)
                    && !corrected
                        .iter()
                        .any(|event| event.identity.key == entry.identity.key)
                {
                    staged.cancel(i, Phase::ReplayAbsent, CancelReason::ReplayAbsent, changes)?;
                }
            }
            Ok(())
        })
    }
    fn bind(
        &mut self,
        i: usize,
        entity: EntityId,
        changes: &mut Changes<P>,
    ) -> Result<(), EventError> {
        if entity.generation == 0 {
            return Err(EventError::InvalidIdentity);
        }
        let entry = &self.entries[i];
        let key = entry.identity.key;
        if let Some(old) = entry.binding {
            return if old == entity {
                Ok(())
            } else {
                Err(EventError::Conflict)
            };
        }
        match self
            .bindings
            .binary_search_by_key(&entity.index, |binding| binding.entity.index)
        {
            Ok(index) => {
                let old = self.bindings[index];
                if entity.generation < old.entity.generation {
                    return Err(EventError::Stale);
                }
                if entity.generation == old.entity.generation && old.key != key {
                    return Err(EventError::Conflict);
                }
                if old.key != key {
                    // The authenticated newer generation proves the old entity
                    // retired, even if its removal checkpoint is delayed on a
                    // different lane. Cancel before binding, and keep a tombstone
                    // so delayed prediction cannot recreate the old visual.
                    if let Ok(old_entry) = self.find(old.key) {
                        self.cancel(old_entry, Phase::Expired, CancelReason::Expired, changes)?;
                    }
                }
                self.bindings[index] = Binding { entity, key };
            }
            Err(index) => {
                if self.bindings.len() >= self.limits.bindings {
                    return Err(EventError::ResetRequired);
                }
                self.bindings.insert(index, Binding { entity, key });
            }
        }
        let retired = matches!(
            self.entries[i].phase,
            Phase::ReplayAbsent | Phase::Expired | Phase::Rejected
        );
        self.entries[i].binding = Some(entity);
        self.emit(
            changes,
            if retired {
                Change::BindRetired { key, entity }
            } else {
                Change::Bind { key, entity }
            },
        )
    }
    /// Apply an already authenticated authoritative outcome; this API does not
    /// authenticate a network message or grant ownership. Accept/reject is terminal.
    /// A later exact binding may augment acceptance;
    /// duplicate acceptance without that binding never removes it. Conflicting
    /// terminal decisions or rebinding the same event fail transactionally.
    /// A strictly newer entity generation for another event atomically retires
    /// the old presentation, which may outlive its authority due to lane ordering.
    pub fn resolve(
        &mut self,
        now: ServerTick,
        identity: EventIdentity,
        outcome: Outcome,
    ) -> Result<Changes<P>, EventError> {
        self.transaction(|staged, changes| {
            staged.advance_inner(now, changes)?;
            let i = staged.entry(identity)?;
            match outcome {
                Outcome::Rejected => {
                    if staged.entries[i].decision == Decision::Accepted {
                        return Err(EventError::Conflict);
                    }
                    staged.entries[i].decision = Decision::Rejected;
                    staged.cancel(i, Phase::Rejected, CancelReason::Rejected, changes)?;
                }
                Outcome::Accepted { binding } => {
                    if staged.entries[i].decision == Decision::Rejected
                        || staged
                            .rejected_actions
                            .binary_search(&identity.key.action)
                            .is_ok()
                    {
                        return Err(EventError::Conflict);
                    }
                    if staged.entries[i].decision != Decision::Accepted {
                        staged.entries[i].decision = Decision::Accepted;
                        staged.emit(changes, Change::Confirm { key: identity.key })?;
                    }
                    if let Some(entity) = binding {
                        staged.bind(i, entity, changes)?;
                    }
                }
            }
            Ok(())
        })
    }
    pub fn reject_action(
        &mut self,
        now: ServerTick,
        action: ActionKey,
    ) -> Result<Changes<P>, EventError> {
        self.transaction(|staged, changes| {
            staged.advance_inner(now, changes)?;
            staged.valid_action(action)?;
            if staged.entries.iter().any(|entry| {
                entry.identity.key.action == action && entry.decision == Decision::Accepted
            }) {
                return Err(EventError::Conflict);
            }
            if let Err(index) = staged.rejected_actions.binary_search(&action) {
                if staged.rejected_actions.len() >= staged.limits.action_fences {
                    return Err(EventError::ResetRequired);
                }
                staged.rejected_actions.insert(index, action);
            }
            for i in 0..staged.entries.len() {
                if staged.entries[i].identity.key.action == action {
                    staged.entries[i].decision = Decision::Rejected;
                    staged.cancel(i, Phase::Rejected, CancelReason::Rejected, changes)?;
                }
            }
            Ok(())
        })
    }
    /// Explicit simulation/authority lifecycle retirement, including committed
    /// effects. An unseen identity can be fenced before its acceptance arrives.
    pub fn expire(
        &mut self,
        now: ServerTick,
        identity: EventIdentity,
    ) -> Result<Changes<P>, EventError> {
        self.transaction(|staged, changes| {
            staged.advance_inner(now, changes)?;
            let i = staged.entry(identity)?;
            if staged.entries[i].phase != Phase::Rejected {
                staged.cancel(i, Phase::Expired, CancelReason::Expired, changes)?;
            }
            Ok(())
        })
    }
    /// Caller must have finalized and discarded these command histories. No live
    /// lower sequence may be crossed. The watermark rejects all delayed keys,
    /// even after tombstones are reclaimed and even with forged newer origins.
    pub fn retire_through(&mut self, sequence: CommandSeq) -> Result<(), EventError> {
        if sequence < self.retired {
            return Err(EventError::Stale);
        }
        if self.entries.iter().any(|entry| {
            entry.identity.key.action.command <= sequence && entry.phase == Phase::Live
        }) {
            return Err(EventError::LiveHistory);
        }
        self.entries
            .retain(|entry| entry.identity.key.action.command > sequence);
        self.rejected_actions
            .retain(|action| action.command > sequence);
        self.retired = sequence;
        Ok(())
    }
    pub fn reset(&mut self, scope: JournalScope) -> Result<Changes<P>, EventError> {
        if scope <= self.scope {
            return Err(EventError::Stale);
        }
        let replacement = Self::new(scope, self.limits)?;
        let mut changes = self.changes()?;
        self.emit(
            &mut changes,
            Change::Reset {
                previous: self.scope,
                current: scope,
            },
        )?;
        *self = replacement;
        Ok(changes)
    }
}
