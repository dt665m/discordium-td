//! Graph-qualified, bounded scheduling. Transport acceptance advances cadence;
//! selection and failed sends do not. Payload projection remains with the game.
use crate::{
    interest::{EligibleEntry, EligibleSet, PredictionAdmission},
    types::{ConnectionId, EntityId, GroupId, PolicyRevision, ServerTick},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum UnitId {
    Entity(EntityId),
    PredictionGroup(GroupId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cadence {
    pub period_ticks: u32,
    pub maximum_age_ticks: u32,
    pub priority: u8,
    pub critical: bool,
}

/// Exact on-wire cost (including framing) of an atomic transmission. A fragmented
/// group may be sent together if it fits. Multi-frame transfers use a frozen,
/// deadline-bounded transfer outside this scheduler and publish only when complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitRequest {
    id: UnitId,
    members: BTreeSet<EntityId>,
    wire_bytes: usize,
    cadence: Cadence,
}
impl UnitRequest {
    pub fn entity(entity: EntityId, wire_bytes: usize, cadence: Cadence) -> Self {
        Self {
            id: UnitId::Entity(entity),
            members: BTreeSet::from([entity]),
            wire_bytes,
            cadence,
        }
    }
    /// The complete graph-approved closure is the sole source of group members.
    /// A denied or missing dependency cannot be made schedulable by splitting it.
    pub fn prediction_group(
        eligible: &EligibleSet,
        group: GroupId,
        wire_bytes: usize,
        cadence: Cadence,
    ) -> Result<Self, ScheduleError> {
        let PredictionAdmission::Admitted { members } = eligible.prediction() else {
            return Err(ScheduleError::Unauthorized);
        };
        if members.is_empty() || group.0 == 0 {
            return Err(ScheduleError::InvalidRequest);
        }
        Ok(Self {
            id: UnitId::PredictionGroup(group),
            members: members.clone(),
            wire_bytes,
            cadence,
        })
    }
    pub fn id(&self) -> UnitId {
        self.id
    }
    pub fn members(&self) -> &BTreeSet<EntityId> {
        &self.members
    }
    pub fn wire_bytes(&self) -> usize {
        self.wire_bytes
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SchedulerLimits {
    pub units: usize,
    pub bytes_per_second: u64,
    pub burst_bytes: usize,
    /// Integer basis points, 0..=10_000. Unused reserve is borrowable.
    pub critical_reserve: u16,
    pub maximum_accumulated_age_ticks: u32,
}
impl Default for SchedulerLimits {
    fn default() -> Self {
        Self {
            units: 4096,
            bytes_per_second: 60_000,
            burst_bytes: 16_384,
            critical_reserve: 4000,
            maximum_accumulated_age_ticks: 65_535,
        }
    }
}

/// Supplied by the transport after its framing, congestion and queue allowances.
/// This API never assumes an application payload from a nominal MTU.
#[derive(Debug, Clone, Copy)]
pub struct TransportBudget {
    pub available_wire_bytes: usize,
    pub maximum_atomic_wire_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleError {
    InvalidConfiguration,
    InvalidRequest,
    Unauthorized,
    Capacity,
    Stale,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScheduleMetrics {
    pub due_units: usize,
    pub selected_units: usize,
    pub selected_wire_bytes: usize,
    pub deferred_units: usize,
    pub infeasible_units: usize,
    pub deadline_misses: usize,
    pub critical_deadline_misses: usize,
    pub maximum_update_age_ticks: u64,
}

#[derive(Debug, Clone, Copy)]
struct History {
    first_due: ServerTick,
    last_transmitted: Option<ServerTick>,
}

/// A plan borrows its exact authorization barrier; it stores no unrestricted
/// payload. A newer plan or policy/world barrier invalidates an unsent old plan.
pub struct SchedulePlan<'a> {
    eligible: &'a EligibleSet,
    serial: u64,
    selected: Vec<UnitRequest>,
    metrics: ScheduleMetrics,
}
impl SchedulePlan<'_> {
    pub fn metrics(&self) -> ScheduleMetrics {
        self.metrics
    }
    pub fn selected(&self) -> impl ExactSizeIterator<Item = &UnitRequest> {
        self.selected.iter()
    }
}

pub struct AuthorizedUnit<'a> {
    request: &'a UnitRequest,
    eligible: &'a EligibleSet,
}
impl AuthorizedUnit<'_> {
    pub fn id(&self) -> UnitId {
        self.request.id
    }
    pub fn wire_bytes(&self) -> usize {
        self.request.wire_bytes
    }
    pub fn entries(&self) -> impl ExactSizeIterator<Item = &EligibleEntry> {
        self.request
            .members
            .iter()
            .map(|id| self.eligible.get(*id).unwrap())
    }
    pub fn committed_tick(&self) -> ServerTick {
        self.eligible.tick()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransmitMetrics {
    pub accepted_units: usize,
    pub accepted_wire_bytes: usize,
    pub rejected_units: usize,
}

#[derive(Debug)]
pub struct Scheduler {
    connection: ConnectionId,
    limits: SchedulerLimits,
    history: BTreeMap<UnitId, History>,
    now: Duration,
    tick: ServerTick,
    world_revision: u64,
    policy_revision: PolicyRevision,
    serial: u64,
    // Nanobytes retain fractional refill and cannot accumulate rounding drift.
    tokens: u128,
}
impl Scheduler {
    pub fn new(connection: ConnectionId, limits: SchedulerLimits) -> Result<Self, ScheduleError> {
        if limits.units == 0
            || limits.bytes_per_second == 0
            || limits.burst_bytes == 0
            || limits.critical_reserve > 10_000
            || limits.maximum_accumulated_age_ticks == 0
        {
            return Err(ScheduleError::InvalidConfiguration);
        }
        Ok(Self {
            connection,
            limits,
            history: BTreeMap::new(),
            now: Duration::ZERO,
            tick: ServerTick(0),
            world_revision: 0,
            policy_revision: PolicyRevision(0),
            serial: 0,
            tokens: limits.burst_bytes as u128 * 1_000_000_000,
        })
    }
    pub fn known_units(&self) -> usize {
        self.history.len()
    }
    pub fn last_transmitted(&self, unit: UnitId) -> Option<ServerTick> {
        self.history.get(&unit).and_then(|h| h.last_transmitted)
    }
    /// Credit available at this instant before constructing bounded transfer units.
    /// Selection and charging remain exclusively owned by `schedule`/`transmit`.
    pub fn available_wire_bytes(&self, now: Duration) -> usize {
        let refill = now
            .saturating_sub(self.now)
            .as_nanos()
            .saturating_mul(self.limits.bytes_per_second as u128);
        (self
            .tokens
            .saturating_add(refill)
            .min(self.limits.burst_bytes as u128 * 1_000_000_000)
            / 1_000_000_000) as usize
    }
    pub fn schedule<'a>(
        &mut self,
        eligible: &'a EligibleSet,
        requests: &[UnitRequest],
        now: Duration,
        transport: TransportBudget,
    ) -> Result<SchedulePlan<'a>, ScheduleError> {
        if eligible.connection() != self.connection {
            return Err(ScheduleError::Unauthorized);
        }
        if now < self.now
            || eligible.tick() < self.tick
            || eligible.world_revision() < self.world_revision
            || eligible.policy_revision() < self.policy_revision
        {
            return Err(ScheduleError::Stale);
        }
        if requests.len() > self.limits.units {
            return Err(ScheduleError::Capacity);
        }
        let mut ids = BTreeSet::new();
        let mut members = BTreeSet::new();
        for request in requests {
            if request.wire_bytes == 0
                || request.cadence.period_ticks == 0
                || request.cadence.maximum_age_ticks < request.cadence.period_ticks
                || request.cadence.maximum_age_ticks > self.limits.maximum_accumulated_age_ticks
                || !ids.insert(request.id)
                || request.members.is_empty()
            {
                return Err(ScheduleError::InvalidRequest);
            }
            if let UnitId::PredictionGroup(_) = request.id {
                if !matches!(eligible.prediction(), PredictionAdmission::Admitted { members } if *members == request.members)
                {
                    return Err(ScheduleError::Unauthorized);
                }
            }
            for entity in &request.members {
                if eligible.get(*entity).is_none() {
                    return Err(ScheduleError::Unauthorized);
                }
                if !members.insert(*entity) {
                    return Err(ScheduleError::InvalidRequest);
                }
            }
        }
        let serial = self.serial.checked_add(1).ok_or(ScheduleError::Capacity)?;
        let refill = now
            .saturating_sub(self.now)
            .as_nanos()
            .saturating_mul(self.limits.bytes_per_second as u128);
        self.tokens = self
            .tokens
            .saturating_add(refill)
            .min(self.limits.burst_bytes as u128 * 1_000_000_000);
        self.now = now;
        self.tick = eligible.tick();
        self.world_revision = eligible.world_revision();
        self.policy_revision = eligible.policy_revision();
        self.serial = serial;
        // Revoked or omitted units retain neither payload nor age state. Requests
        // must include unchanged active units when cadence history is wanted.
        self.history.retain(|id, _| ids.contains(id));
        for request in requests {
            self.history.entry(request.id).or_insert(History {
                first_due: self.tick,
                last_transmitted: None,
            });
        }
        let budget = transport
            .available_wire_bytes
            .min((self.tokens / 1_000_000_000) as usize);
        let mut due = Vec::with_capacity(requests.len());
        let mut metrics = ScheduleMetrics::default();
        for request in requests {
            let h = self.history[&request.id];
            let age = self.tick.0 - h.last_transmitted.unwrap_or(h.first_due).0;
            metrics.maximum_update_age_ticks = metrics.maximum_update_age_ticks.max(age);
            if h.last_transmitted.is_some() && age < request.cadence.period_ticks as u64 {
                continue;
            }
            metrics.due_units += 1;
            let infeasible = request.wire_bytes > self.limits.burst_bytes
                || request.wire_bytes > transport.maximum_atomic_wire_bytes;
            if infeasible {
                metrics.infeasible_units += 1;
            }
            let score = age.min(self.limits.maximum_accumulated_age_ticks as u64)
                + request.cadence.priority as u64;
            due.push((request, age, score, infeasible));
        }
        // Bounded age beats any fixed priority eventually. Stable IDs resolve ties.
        due.sort_by(|a, b| {
            let overdue = |x: &(&UnitRequest, u64, u64, bool)| {
                (
                    x.1 >= x.0.cadence.maximum_age_ticks as u64,
                    x.1.saturating_sub(x.0.cadence.maximum_age_ticks as u64),
                )
            };
            overdue(b)
                .cmp(&overdue(a))
                .then_with(|| b.2.cmp(&a.2))
                .then_with(|| a.0.id.cmp(&b.0.id))
        });
        let reserve = (budget as u128 * self.limits.critical_reserve as u128 / 10_000) as usize;
        let mut selected_ids = BTreeSet::new();
        let mut selected = Vec::new();
        let mut used = 0;
        for (request, _, _, infeasible) in &due {
            if !infeasible
                && request.cadence.critical
                && used < reserve
                && request.wire_bytes <= budget - used
            {
                used += request.wire_bytes;
                selected_ids.insert(request.id);
                selected.push((*request).clone());
            }
        }
        for (request, _, _, infeasible) in &due {
            if !infeasible
                && !selected_ids.contains(&request.id)
                && request.wire_bytes <= budget - used
            {
                used += request.wire_bytes;
                selected_ids.insert(request.id);
                selected.push((*request).clone());
            }
        }
        for (request, age, _, _) in &due {
            if !selected_ids.contains(&request.id) {
                metrics.deferred_units += 1;
                if *age >= request.cadence.maximum_age_ticks as u64 {
                    metrics.deadline_misses += 1;
                    metrics.critical_deadline_misses += usize::from(request.cadence.critical);
                }
            }
        }
        metrics.selected_units = selected.len();
        metrics.selected_wire_bytes = used;
        Ok(SchedulePlan {
            eligible,
            serial,
            selected,
            metrics,
        })
    }
    /// The callback must synchronously attempt the exact priced transmission and
    /// return true only when transport accepts all of it. It must not enqueue an
    /// unbounded or revision-blind deferred payload. A plan is consumed once.
    pub fn transmit(
        &mut self,
        plan: SchedulePlan<'_>,
        world_revision: u64,
        policy_revision: PolicyRevision,
        mut send: impl FnMut(AuthorizedUnit<'_>) -> bool,
    ) -> Result<TransmitMetrics, ScheduleError> {
        if plan.serial != self.serial || !plan.eligible.is_current(world_revision, policy_revision)
        {
            return Err(ScheduleError::Stale);
        }
        // Invalidate before invoking application code, including an empty plan.
        self.serial = self.serial.checked_add(1).ok_or(ScheduleError::Capacity)?;
        let mut metrics = TransmitMetrics::default();
        for request in &plan.selected {
            if send(AuthorizedUnit {
                request,
                eligible: plan.eligible,
            }) {
                self.tokens -= request.wire_bytes as u128 * 1_000_000_000;
                self.history.get_mut(&request.id).unwrap().last_transmitted =
                    Some(plan.eligible.tick());
                metrics.accepted_units += 1;
                metrics.accepted_wire_bytes += request.wire_bytes;
            } else {
                metrics.rejected_units += 1;
            }
        }
        Ok(metrics)
    }
}

#[cfg(test)]
mod tests;
