//! Delayed, renderer-only remote samples. These values must never become physics,
//! gameplay, replay dependencies or authoritative replication receipts.
//!
//! The caller admits samples only after authenticated scope publication and owns
//! teleport/death/parent-change detection. Increment `segment` at those barriers.
//! Retired scope fences remain bounded and are never evicted to accept new actors.
use crate::{replication::Payload, synchronization::PresentationCursor, types::*};
use std::{collections::BTreeMap, time::Duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteIdentity {
    pub scope: ScopeIdentity,
    pub scene: SceneRevision,
    /// Monotonic within an exact scope/scene; nonzero. A change snaps history.
    pub segment: u64,
}
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteSample<P> {
    pub identity: RemoteIdentity,
    pub tick: ServerTick,
    pub value: P,
}
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Includes retired scope fences. Exhaustion requires an explicit session reset.
    pub known_entities: usize,
    pub samples_per_entity: usize,
    pub total_samples: usize,
    pub history_ticks: u64,
    pub sample_heap_bytes: usize,
    /// Charges sample structs, reported heap capacities and conservative map cells.
    pub retained_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            known_entities: 4096,
            samples_per_entity: 32,
            total_samples: 32768,
            history_ticks: 120,
            sample_heap_bytes: 4096,
            retained_bytes: 8 * 1024 * 1024,
        }
    }
}
#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub rate: TickRate,
    /// Total presentation age, including publication/downlink age and jitter slack.
    pub delay: Duration,
    /// Initial profile: at most 100 ms. Zero explicitly disables extrapolation.
    pub extrapolation_cap: Duration,
    pub limits: Limits,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidConfiguration,
    InvalidIdentity,
    InvalidPayload,
    WrongEpoch,
    Stale,
    Conflict,
    Capacity,
    TimeRange,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Received {
    Inserted,
    Duplicate,
    TooOld,
    Discontinuity,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreezeReason {
    ExtrapolationLimit,
    InsufficientHistory,
    Policy,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Buffering,
    Sampled,
    Interpolated,
    Extrapolated,
    /// Explicit degraded output; drift has stopped at the configured cap.
    Frozen(FreezeReason),
}
#[derive(Debug, Clone, PartialEq)]
pub struct Presented<P> {
    pub identity: RemoteIdentity,
    pub value: P,
    pub status: Status,
    pub cursor: Duration,
    pub newest_tick: ServerTick,
    /// Effective pose time stops when extrapolation freezes; cursor may advance.
    pub pose_time: Duration,
    pub total_age: Duration,
}
/// Game-specific pose policy. Clamp tangents/overshoot and use shortest-arc
/// orientation interpolation here. Returning None freezes rather than guessing.
/// The engine supplies alpha in [0,1] and extrapolation no longer than its cap.
pub trait PosePolicy<P> {
    fn valid(&self, value: &P) -> bool;
    fn interpolate(&self, earlier: &P, later: &P, alpha: f32) -> Option<P>;
    fn extrapolate(
        &self,
        earlier: &P,
        latest: &P,
        interval: Duration,
        ahead: Duration,
    ) -> Option<P>;
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Accounting {
    pub known_entities: usize,
    pub active_entities: usize,
    pub samples: usize,
    pub retained_bytes: usize,
}
struct Track<P> {
    identity: RemoteIdentity,
    retired: bool,
    samples: Box<[RemoteSample<P>]>,
}
pub struct RemoteBuffer<P> {
    connection: ConnectionEpoch,
    config: Config,
    tracks: BTreeMap<u64, Track<P>>,
    accounting: Accounting,
    clock: PresentationCursor,
    at: Duration,
    estimated_now: Duration,
}
impl<P: Payload> RemoteBuffer<P> {
    pub fn new(connection: ConnectionEpoch, config: Config) -> Result<Self, Error> {
        let limits = config.limits;
        if connection.0 == 0 {
            return Err(Error::InvalidIdentity);
        }
        if limits.known_entities == 0
            || limits.samples_per_entity < 2
            || limits.total_samples < 2
            || limits.history_ticks == 0
            || limits.retained_bytes < track_charge::<P>()
            || config.extrapolation_cap > Duration::from_millis(100)
        {
            return Err(Error::InvalidConfiguration);
        }
        Ok(Self {
            connection,
            config,
            tracks: BTreeMap::new(),
            accounting: Accounting::default(),
            clock: PresentationCursor::default(),
            at: Duration::ZERO,
            estimated_now: Duration::ZERO,
        })
    }
    /// Current renderer identity, for adapter-owned discontinuity detection.
    pub fn identities(&self) -> impl Iterator<Item = RemoteIdentity> + '_ {
        self.tracks
            .values()
            .filter(|v| !v.retired)
            .map(|v| v.identity)
    }
    pub fn identity(&self, entity: EntityId) -> Option<RemoteIdentity> {
        self.tracks
            .get(&entity.index)
            .filter(|v| !v.retired && v.identity.scope.entity == entity)
            .map(|v| v.identity)
    }
    pub fn latest(&self, scope: ScopeIdentity) -> Option<&RemoteSample<P>> {
        self.tracks
            .get(&scope.entity.index)
            .filter(|v| !v.retired && v.identity.scope == scope)
            .and_then(|v| v.samples.last())
    }
    pub fn accounting(&self) -> Accounting {
        self.accounting
    }
    pub fn cursor(&self) -> Duration {
        self.at
    }
    pub fn set_delay(&mut self, delay: Duration) {
        self.config.delay = delay;
    }
    /// Uses the existing presentation clock. A reordered estimate or larger delay
    /// can pause playback but never reverses its cursor within a connection epoch.
    pub fn advance(&mut self, estimated_server_now: Duration) -> Duration {
        self.estimated_now = self.estimated_now.max(estimated_server_now);
        self.at = self.clock.advance(self.estimated_now, self.config.delay);
        self.at
    }
    pub fn receive(
        &mut self,
        sample: RemoteSample<P>,
        policy: &impl PosePolicy<P>,
    ) -> Result<Received, Error> {
        let identity = sample.identity;
        validate_identity(identity, self.connection)?;
        if self.config.rate.deadline(sample.tick).is_none() {
            return Err(Error::TimeRange);
        }
        if !policy.valid(&sample.value) {
            return Err(Error::InvalidPayload);
        }
        if sample.value.retained_bytes() > self.config.limits.sample_heap_bytes {
            return Err(Error::Capacity);
        }
        let old = self.tracks.get(&identity.scope.entity.index);
        let changed = if let Some(old) = old {
            if old.retired
                && identity.scope.entity.generation == old.identity.scope.entity.generation
                && identity.scope.scope <= old.identity.scope.scope
            {
                return Err(Error::Stale);
            }
            if identity != old.identity {
                if !newer_identity(identity, old.identity) {
                    return Err(Error::Stale);
                }
                true
            } else {
                if old.retired {
                    return Err(Error::Stale);
                }
                false
            }
        } else {
            true
        };
        if old.is_none() && self.accounting.known_entities == self.config.limits.known_entities {
            return Err(Error::Capacity);
        }
        let prior = old
            .filter(|_| !changed)
            .map_or(&[][..], |v| v.samples.as_ref());
        if let Some(existing) = prior.iter().find(|v| v.tick == sample.tick) {
            return if existing == &sample {
                Ok(Received::Duplicate)
            } else {
                Err(Error::Conflict)
            };
        }
        let newest = prior
            .last()
            .map_or(sample.tick, |v| v.tick.max(sample.tick));
        let floor = newest.0.saturating_sub(self.config.limits.history_ticks);
        if sample.tick.0 < floor
            || (prior.len() == self.config.limits.samples_per_entity && sample.tick < prior[0].tick)
        {
            return Ok(Received::TooOld);
        }
        // At most one bounded per-entity history is staged before atomic replacement.
        // Returned render values and this temporary history are caller/scratch storage.
        let mut samples: Vec<_> = prior
            .iter()
            .filter(|v| v.tick.0 >= floor)
            .cloned()
            .collect();
        let index = samples.partition_point(|v| v.tick < sample.tick);
        samples.insert(index, sample);
        if samples.len() > self.config.limits.samples_per_entity {
            samples.remove(0);
        }
        let samples = samples.into_boxed_slice();
        let old_samples = old.map_or(0, |v| v.samples.len());
        let old_bytes = old.map_or(0, |v| samples_charge(&v.samples));
        let sample_count = self.accounting.samples - old_samples + samples.len();
        let bytes = self
            .accounting
            .retained_bytes
            .checked_sub(old_bytes)
            .and_then(|v| v.checked_add(samples_charge(&samples)))
            .and_then(|v| {
                v.checked_add(if old.is_none() {
                    track_charge::<P>()
                } else {
                    0
                })
            })
            .ok_or(Error::Capacity)?;
        if sample_count > self.config.limits.total_samples
            || bytes > self.config.limits.retained_bytes
        {
            return Err(Error::Capacity);
        }
        let newly_active = old.is_none_or(|v| v.retired);
        if old.is_none() {
            self.accounting.known_entities += 1;
        }
        if newly_active {
            self.accounting.active_entities += 1;
        }
        self.accounting.samples = sample_count;
        self.accounting.retained_bytes = bytes;
        let result = if changed && old.is_some() {
            Received::Discontinuity
        } else {
            Received::Inserted
        };
        self.tracks.insert(
            identity.scope.entity.index,
            Track {
                identity,
                retired: false,
                samples,
            },
        );
        Ok(result)
    }
    /// Retains the incarnation fence while releasing history. A late sample from
    /// this exact scope cannot resurrect it; a higher authenticated scope may enter.
    pub fn exit(&mut self, scope: ScopeIdentity) -> Result<(), Error> {
        let track = self
            .tracks
            .get_mut(&scope.entity.index)
            .ok_or(Error::Stale)?;
        if track.identity.scope != scope {
            return Err(Error::Stale);
        }
        if !track.retired {
            self.accounting.active_entities -= 1;
            self.accounting.samples -= track.samples.len();
            self.accounting.retained_bytes -= samples_charge(&track.samples);
            track.samples = Box::default();
            track.retired = true;
        }
        Ok(())
    }
    pub fn sample(
        &self,
        scope: ScopeIdentity,
        policy: &impl PosePolicy<P>,
    ) -> Result<Option<Presented<P>>, Error> {
        let Some(track) = self.tracks.get(&scope.entity.index) else {
            return Ok(None);
        };
        if track.retired || track.identity.scope != scope {
            return Ok(None);
        }
        let samples = &track.samples;
        let Some(latest) = samples.last() else {
            return Ok(None);
        };
        let first = &samples[0];
        let at = self.at;
        let first_time = self
            .config
            .rate
            .deadline(first.tick)
            .ok_or(Error::TimeRange)?;
        let mut pose_time = at;
        let (value, status) = if at < first_time {
            pose_time = first_time;
            (first.value.clone(), Status::Buffering)
        } else {
            let upper = samples.partition_point(|v| {
                self.config
                    .rate
                    .deadline(v.tick)
                    .is_some_and(|time| time <= at)
            });
            if upper < samples.len() {
                let earlier = &samples[upper - 1];
                let later = &samples[upper];
                let start = self
                    .config
                    .rate
                    .deadline(earlier.tick)
                    .ok_or(Error::TimeRange)?;
                let end = self
                    .config
                    .rate
                    .deadline(later.tick)
                    .ok_or(Error::TimeRange)?;
                let alpha = (at.saturating_sub(start).as_secs_f64() / (end - start).as_secs_f64())
                    .clamp(0.0, 1.0) as f32;
                if alpha == 0.0 {
                    (earlier.value.clone(), Status::Sampled)
                } else {
                    match policy.interpolate(&earlier.value, &later.value, alpha) {
                        Some(value) => (value, Status::Interpolated),
                        None => {
                            pose_time = start;
                            (earlier.value.clone(), Status::Frozen(FreezeReason::Policy))
                        }
                    }
                }
            } else {
                let latest_time = self
                    .config
                    .rate
                    .deadline(latest.tick)
                    .ok_or(Error::TimeRange)?;
                let ahead = at.saturating_sub(latest_time);
                if ahead.is_zero() {
                    (latest.value.clone(), Status::Sampled)
                } else if samples.len() < 2 {
                    pose_time = latest_time;
                    (
                        latest.value.clone(),
                        Status::Frozen(FreezeReason::InsufficientHistory),
                    )
                } else if self.config.extrapolation_cap.is_zero() {
                    pose_time = latest_time;
                    (
                        latest.value.clone(),
                        Status::Frozen(FreezeReason::ExtrapolationLimit),
                    )
                } else {
                    let previous = &samples[samples.len() - 2];
                    let interval = latest_time
                        - self
                            .config
                            .rate
                            .deadline(previous.tick)
                            .ok_or(Error::TimeRange)?;
                    pose_time = latest_time
                        .checked_add(ahead.min(self.config.extrapolation_cap))
                        .ok_or(Error::TimeRange)?;
                    match policy.extrapolate(
                        &previous.value,
                        &latest.value,
                        interval,
                        ahead.min(self.config.extrapolation_cap),
                    ) {
                        Some(value) => (
                            value,
                            if ahead > self.config.extrapolation_cap {
                                Status::Frozen(FreezeReason::ExtrapolationLimit)
                            } else {
                                Status::Extrapolated
                            },
                        ),
                        None => {
                            pose_time = latest_time;
                            (latest.value.clone(), Status::Frozen(FreezeReason::Policy))
                        }
                    }
                }
            }
        };
        if !policy.valid(&value) {
            return Err(Error::InvalidPayload);
        }
        Ok(Some(Presented {
            identity: track.identity,
            value,
            status,
            cursor: at,
            newest_tick: latest.tick,
            pose_time,
            total_age: self.estimated_now.saturating_sub(pose_time),
        }))
    }
}
fn validate_identity(identity: RemoteIdentity, connection: ConnectionEpoch) -> Result<(), Error> {
    if identity.scope.connection != connection {
        return Err(Error::WrongEpoch);
    }
    if identity.scope.entity.generation == 0
        || identity.scope.scope.0 == 0
        || identity.scope.representation.0 == 0
        || identity.scene.0 == 0
        || identity.segment == 0
    {
        return Err(Error::InvalidIdentity);
    }
    Ok(())
}
fn newer_identity(new: RemoteIdentity, old: RemoteIdentity) -> bool {
    if new.scene < old.scene {
        return false;
    }
    if new.scope.entity.generation != old.scope.entity.generation {
        return new.scope.entity.generation > old.scope.entity.generation;
    }
    if new.scope.scope != old.scope.scope {
        return new.scope.scope > old.scope.scope;
    }
    if new.scope.representation != old.scope.representation {
        return new.scope.representation > old.scope.representation;
    }
    if new.scene != old.scene {
        return new.scene > old.scene;
    }
    new.segment > old.segment
}
fn track_charge<P>() -> usize {
    std::mem::size_of::<(u64, Track<P>)>() + 64
}
fn samples_charge<P: Payload>(samples: &[RemoteSample<P>]) -> usize {
    samples
        .iter()
        .fold(std::mem::size_of_val(samples), |sum, sample| {
            sum.saturating_add(sample.value.retained_bytes())
        })
}
#[cfg(test)]
mod tests;
