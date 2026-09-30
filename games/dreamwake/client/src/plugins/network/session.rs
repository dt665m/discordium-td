//! Authenticated scoped session state, independent of Bevy and hardware polling.
use super::model::*;
use dreamwake_protocol::live::baselines as baseline_wire;
use dreamwake_protocol::{
    DreamAction,
    live::*,
    replication::{ReplicaPayload, decode_replica},
};
use dreamwake_sim::replication::DreamPresentation;
use engine_net::replication::baselines::BaselineEncoding;
use engine_net::{
    codec::BoundedVec,
    input::InputJournal,
    prediction::*,
    replication::*,
    synchronization::{
        ClockConfig, ClockEstimator, ClockSample, CommittedClock, LeadController, ResyncReason,
        SyncError, TimeExchange,
    },
    types::*,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub(crate) struct DeliveryMetrics {
    pub group_messages: u64,
    pub groups_completed: u64,
    pub baseline_stale: u64,
    pub baseline_reset_pending: u64,
    pub replay_requested: u64,
    pub maximum_replay_requested: u64,
    pub replay_cpu_micros: u64,
    pub maximum_replay_cpu_micros: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionMetrics {
    pub server_instance: u64,
    /// Server transport peer identity; stream epochs may change independently.
    pub transport_connection: u64,
    pub active: bool,
    pub recovering: bool,
    pub connection_epoch: ConnectionEpoch,
    pub match_epoch: u32,
    pub estimated_server_tick: Option<ServerTick>,
    pub estimated_simulation_tick: Option<ServerTick>,
    pub committed_server_tick: ServerTick,
    pub clock_anchor_tick: ServerTick,
    pub checkpoint_server_tick: Option<ServerTick>,
    pub finalized_tick: ServerTick,
    pub predicted_tick: Option<ServerTick>,
    pub gameplay_tick: Option<u32>,
    pub combat_view_tick: Option<ServerTick>,
    pub known_scopes: usize,
    pub live_scopes: usize,
    pub incomplete_groups: usize,
    pub prediction_groups: usize,
    pub lead_ticks: u8,
    pub arrival_slack: Option<u8>,
    pub arrival_feedback: ArrivalFeedback,
    pub arrival_feedback_applied: u64,
    pub arrival_feedback_ignored: u64,
    pub lead_policy: u64,
    pub scope_bytes: usize,
    pub group_bytes: usize,
    pub baseline_bytes: usize,
    pub baseline_repairs: u64,
    pub delivery: DeliveryMetrics,
    pub prediction_bytes: usize,
    pub replay_ticks: usize,
    pub predicted_ticks_this_frame: usize,
    pub interpolation: super::RemotePoseMetrics,
}
// Eight maximum-width StateReceipts fit the minimum admitted 1088-byte frame.
// Each receive pass drains its final partial batch before transport publication.
const DECODED_RECEIPTS_PER_CONTROL: usize = 8;

#[derive(Clone, Copy)]
struct CommandIssuance {
    sequence: CommandSeq,
    target: TargetTick,
    policy: Option<u64>,
    feedback_seen: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResyncRequest {
    Idle,
    PendingEnqueue,
    AwaitingWelcome,
}

/// A complete reliable message may precede its reset on the control channel.
/// Renet has already acknowledged its bytes, so keep one validated message until
/// that control arrives. This is not byte assembly or application retransmission.
struct PendingResetGroup {
    publication: GroupPublication,
    bytes: Box<[u8]>,
    received_at: Duration,
    started_at: Duration,
}

pub(crate) struct Session {
    pub(super) model: OwnerModel,
    pub(super) baselines: super::baselines::OwnerBaselines,
    baseline_repairs: u64,
    delivery: DeliveryMetrics,
    pub(super) poses: super::presentation::RemotePoses,
    pub(super) scopes: ClientScopes<Vec<u8>>,
    pub(super) groups: GroupAssembler<Vec<u8>>,
    pending_reset_group: Option<PendingResetGroup>,
    pub(super) predictor: PredictionManager<OwnerModel>,
    pub(crate) events: super::predicted_events::PredictedEvents,
    pub(super) journal: InputJournal<TickInput, DreamAction>,
    pub(super) input_deadline: Option<Duration>,
    pub(super) clock: ClockEstimator,
    pub(super) simulation_clock: CommittedClock,
    pub(super) lead: LeadController,
    pub(super) entities: BTreeSet<EntityId>,
    pub(super) finalized: ServerTick,
    pub(super) finalized_ack_pending: Option<ServerTick>,
    pub(super) outcomes_ack_pending: Option<u64>,
    pub(super) estimated_server_tick: Option<ServerTick>,
    pub(super) estimated_simulation_tick: Option<ServerTick>,
    pub(super) arrival_slack: Option<u8>,
    arrival_feedback: ArrivalFeedback,
    arrival_feedback_applied: u64,
    arrival_feedback_ignored: u64,
    lead_policy: u64,
    lead_policy_head: Option<TargetTick>,
    issued_commands: std::collections::VecDeque<CommandIssuance>,
    retired_issuance_through: CommandSeq,
    pub(super) active: bool,
    pub(super) recovering: bool,
    pub(super) sequence: CommandSeq,
    pub(super) last_input_retry_committed: Option<ServerTick>,
    pub(super) charge_episode: Option<u64>,
    pub(super) action_backlog: std::collections::VecDeque<DreamAction>,
    pub(super) autoplay_edges: [bool; 5],
    pub(super) frame: u64,
    pub(super) control_sequence: u32,
    pending_decoded_receipts: BoundedVec<StateReceipt, DECODED_RECEIPTS_PER_CONTROL>,
    pub(super) decoded_groups: BTreeMap<SnapshotId, ServerTick>,
    combat_references: std::collections::VecDeque<(StateReceipt, ServerTick)>,
    pub(super) acknowledged_active: Option<(SnapshotId, ServerTick)>,
    pub(super) last_probe: Duration,
    pub(super) last_resync: Option<Duration>,
    pub(super) resync_request: ResyncRequest,
    /// Why recovery was requested. The protocol carries a typed reason and it
    /// is the only attribution the server ever sees, so it must describe the
    /// fault that actually happened rather than a fixed placeholder.
    pub(super) resync_reason: dreamwake_protocol::live::ResyncReason,
    /// The fault text as observed, carried to the server because a deployed
    /// browser's own log cannot be read from there.
    pub(super) resync_detail: String,
}
impl Session {
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn public_hero_scopes(&self) -> Vec<(u64, ScopeIdentity, ServerTick, [f32; 2])> {
        self.entities
            .iter()
            .filter_map(|entity| {
                let state = self.scopes.state(*entity)?;
                match decode_replica(&state.payload, None).ok()? {
                    ReplicaPayload::Actor(dreamwake_sim::replication::PublicReplica::Hero(
                        hero,
                    )) => Some((hero.id, state.scope, state.end_tick, hero.position)),
                    _ => None,
                }
            })
            .collect()
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn public_enemy_scopes(&self) -> Vec<(u64, ScopeIdentity, ServerTick, [f32; 2])> {
        self.entities
            .iter()
            .filter_map(|entity| {
                let state = self.scopes.state(*entity)?;
                match decode_replica(&state.payload, None).ok()? {
                    ReplicaPayload::Actor(dreamwake_sim::replication::PublicReplica::Enemy(
                        enemy,
                    )) => Some((enemy.id, state.scope, state.end_tick, enemy.position)),
                    _ => None,
                }
            })
            .collect()
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn scope_identities(&self) -> Vec<ScopeIdentity> {
        self.entities
            .iter()
            .filter_map(|entity| self.scopes.state(*entity).map(|state| state.scope))
            .collect()
    }
    pub(crate) fn metrics(&self) -> SessionMetrics {
        let checkpoint = self.scopes.state(self.model.welcome.owner_entity);
        let work = self.predictor.work();
        SessionMetrics {
            server_instance: self.model.welcome.server_instance,
            transport_connection: self.model.welcome.client.0,
            active: self.active,
            recovering: self.recovering,
            connection_epoch: self.model.welcome.stream.epoch,
            match_epoch: self.model.welcome.match_epoch,
            estimated_server_tick: self.estimated_server_tick,
            estimated_simulation_tick: self.estimated_simulation_tick,
            committed_server_tick: self.simulation_clock.committed(),
            clock_anchor_tick: self.simulation_clock.anchor().0,
            checkpoint_server_tick: checkpoint.map(|v| v.end_tick),
            finalized_tick: self.finalized,
            predicted_tick: self.predictor.predicted_tick(OWNER_GROUP),
            gameplay_tick: self
                .predictor
                .state(OWNER_GROUP)
                .map(|v| v.0.stamp().gameplay_tick),
            combat_view_tick: self.poses.combat_view_tick(),
            known_scopes: self.scopes.known_entities(),
            live_scopes: self
                .entities
                .iter()
                .filter(|entity| self.scopes.state(**entity).is_some())
                .count(),
            incomplete_groups: self.groups.incomplete()
                + usize::from(self.pending_reset_group.is_some()),
            prediction_groups: self.predictor.group_count(),
            lead_ticks: self.lead.lead(),
            arrival_slack: self.arrival_slack,
            arrival_feedback: self.arrival_feedback,
            arrival_feedback_applied: self.arrival_feedback_applied,
            arrival_feedback_ignored: self.arrival_feedback_ignored,
            lead_policy: self.lead_policy,
            scope_bytes: self.scopes.retained_bytes(),
            group_bytes: self.groups.staged_bytes()
                + self
                    .pending_reset_group
                    .as_ref()
                    .map_or(0, |group| group.bytes.len()),
            baseline_bytes: self.baselines.retained_bytes(),
            baseline_repairs: self.baseline_repairs,
            delivery: self.delivery,
            prediction_bytes: self.predictor.retained_bytes(),
            replay_ticks: work.replay_ticks,
            predicted_ticks_this_frame: work.predicted_ticks,
            interpolation: self.poses.metrics(),
        }
    }

    pub(super) fn new(welcome: Welcome) -> Result<Self, String> {
        let epoch = welcome.stream.epoch;
        let rate = TickRate::new(dreamwake_sim::TICK_HZ).unwrap();
        let server_epoch = Duration::from_nanos(welcome.server_time_nanos)
            .saturating_sub(rate.deadline(welcome.tick).ok_or("clock range")?);
        Ok(Self {
            baselines: Default::default(),
            baseline_repairs: 0,
            delivery: DeliveryMetrics::default(),
            scopes: ClientScopes::new(epoch, baseline_wire::scope_limits()).map_err(err)?,
            groups: GroupAssembler::new(epoch, group_limits()).map_err(err)?,
            pending_reset_group: None,
            predictor: new_predictor(epoch)?,
            events: super::predicted_events::PredictedEvents::new(
                engine_net::events::JournalScope {
                    connection: epoch,
                    stream: welcome.stream.stream,
                },
            )?,
            journal: InputJournal::new(TickInput::default(), 256).map_err(err)?,
            input_deadline: None,
            clock: ClockEstimator::new(rate, server_epoch, ClockConfig::default()).map_err(err)?,
            simulation_clock: CommittedClock::new(
                rate,
                welcome.tick,
                Duration::from_nanos(welcome.server_time_nanos),
                PredictionLimits::default().replay_ticks_per_frame as u32,
            )
            .map_err(err)?,
            lead: LeadController::new(3).map_err(err)?,
            entities: BTreeSet::new(),
            finalized: welcome.tick,
            finalized_ack_pending: None,
            outcomes_ack_pending: None,
            estimated_server_tick: None,
            estimated_simulation_tick: None,
            arrival_slack: None,
            arrival_feedback: ArrivalFeedback::default(),
            arrival_feedback_applied: 0,
            arrival_feedback_ignored: 0,
            lead_policy: 0,
            lead_policy_head: None,
            issued_commands: Default::default(),
            retired_issuance_through: CommandSeq(0),
            active: false,
            recovering: false,
            sequence: CommandSeq(0),
            last_input_retry_committed: None,
            charge_episode: None,
            action_backlog: Default::default(),
            autoplay_edges: [false; 5],
            frame: 0,
            control_sequence: 0,
            pending_decoded_receipts: BoundedVec::default(),
            decoded_groups: BTreeMap::new(),
            combat_references: Default::default(),
            acknowledged_active: None,
            last_probe: Duration::ZERO,
            last_resync: None,
            resync_request: ResyncRequest::Idle,
            resync_reason: dreamwake_protocol::live::ResyncReason::InvalidCheckpoint,
            resync_detail: String::new(),
            poses: super::presentation::RemotePoses::new(epoch, welcome.content.scene_revision)
                .map_err(|e| format!("remote presentation: {e:?}"))?,
            model: OwnerModel::new(welcome),
        })
    }
    /// Returns whether this probe initialized or advanced the clock. A probe
    /// that was discarded commits nothing, so callers must not commit coupled
    /// state for it either.
    pub(super) fn observe_clock(&mut self, exchange: TimeExchange) -> Result<bool, SyncError> {
        // Initial renderer/asset stalls can delay the first reply far beyond the
        // command horizon. Validate the coupled lead before committing an offset;
        // the next usable probe must still initialize both pieces together.
        let initial_lead = if self.clock.sample_count() == 0 {
            let sample = ClockSample::from_exchange(exchange)?;
            match LeadController::bootstrap(
                TickRate::new(dreamwake_sim::TICK_HZ).unwrap(),
                sample.network_round_trip,
                Duration::from_millis(20),
            ) {
                Ok(lead) => Some(lead),
                // A local renderer or asset stall delays this reply after the
                // server sent it, so the round trip it reports is the stall,
                // not the network. Discard the probe and wait for the next
                // one, exactly as the coupled-initialization rule above
                // requires. Escalating instead would reject the reply, leave
                // the clock uninitialized, and spend a recovery attempt that
                // cannot make a stalled frame arrive sooner; three of those
                // inside the sliding window end the connection. A round trip
                // that is genuinely past the horizon still stops the join at
                // the bootstrap deadline.
                Err(SyncError::ResyncRequired(ResyncReason::LeadExceeded)) => return Ok(false),
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        self.clock.observe(exchange)?;
        if let Some(lead) = initial_lead {
            self.lead = lead;
        }
        Ok(true)
    }
    pub(super) fn observe_clock_reply(
        &mut self,
        exchange: TimeExchange,
        tick: ServerTick,
    ) -> Result<(), SyncError> {
        let mut simulation_clock = self.simulation_clock;
        simulation_clock.observe(tick, exchange.server_send)?;
        // Coupled initialization: a discarded probe advances neither piece.
        if !self.observe_clock(exchange)? {
            return Ok(());
        }
        self.simulation_clock = simulation_clock;
        Ok(())
    }
    /// Choose the fresh command head from authenticated committed progress.
    /// Forecast ticks before that head use checkpointed missing-input state;
    /// authored ticks consume hardware samples at their own capture deadlines.
    /// Lead changes may pause the input deadline but cannot drain it backward.
    /// This never edits an already assigned command or its target.
    pub(super) fn assign_command_target(
        &mut self,
        estimated: ServerTick,
    ) -> Result<Option<TargetTick>, String> {
        if self.unstarted_prediction() {
            let current = self.predictor.predicted_tick(OWNER_GROUP).unwrap();
            let target = estimated
                .0
                .checked_add(u64::from(self.lead.lead()))
                .ok_or("clock range")?;
            let remaining = self
                .predictor
                .remaining_prediction_ticks(OWNER_GROUP)
                .ok_or("missing owner prediction")?;
            // Admission can finish while its initial checkpoint is still old.
            // Wait for a current complete checkpoint before assigning any intent;
            // otherwise assigning a target here poisons the lead controller even
            // though no command could fit the unchanged frame work budget.
            if target.saturating_sub(current.0) > remaining as u64 {
                return Ok(None);
            }
        }
        self.lead.assign_target(estimated).map_err(err)
    }
    pub(super) fn forecast_gap(&self, estimated: ServerTick) -> bool {
        self.sequence == CommandSeq(0)
            || self
                .predictor
                .predicted_tick(OWNER_GROUP)
                .is_some_and(|tick| tick < estimated)
    }
    pub(super) fn record_command_issuance(
        &mut self,
        sequence: CommandSeq,
        target: TargetTick,
        newest: TargetTick,
    ) {
        // Increasing lead can fill extra targets before its new head. Those
        // transitions have not benefited from the new policy yet.
        let head = *self.lead_policy_head.get_or_insert(newest);
        if self.issued_commands.len() == 256 {
            if let Some(retired) = self.issued_commands.pop_front() {
                self.retired_issuance_through = retired.sequence;
            }
        }
        self.issued_commands.push_back(CommandIssuance {
            sequence,
            target,
            policy: (target >= head).then_some(self.lead_policy),
            feedback_seen: false,
        });
    }
    pub(super) fn observe_arrival_feedback(
        &mut self,
        feedback: ArrivalFeedback,
    ) -> Result<(), SyncError> {
        if !feedback.validate()
            || feedback.observed_commands < self.arrival_feedback.observed_commands
            || feedback.late_commands < self.arrival_feedback.late_commands
            || (feedback.sample.is_some()
                && feedback.observed_commands == self.arrival_feedback.observed_commands)
        {
            return Err(SyncError::InvalidSample);
        }
        if feedback.late_commands - self.arrival_feedback.late_commands
            > feedback.observed_commands - self.arrival_feedback.observed_commands
        {
            return Err(SyncError::InvalidSample);
        }
        let Some(sample) = feedback.sample else {
            self.arrival_feedback = feedback;
            self.arrival_slack = None;
            return Ok(());
        };
        if sample.sequence > self.sequence {
            return Err(SyncError::InvalidSample);
        }
        let index = self
            .issued_commands
            .iter()
            .position(|issued| issued.sequence == sample.sequence);
        let eligible = if let Some(index) = index {
            let issued = &self.issued_commands[index];
            if issued.target != sample.target || issued.feedback_seen {
                return Err(SyncError::InvalidSample);
            }
            issued.policy == Some(self.lead_policy)
        } else if sample.sequence <= self.retired_issuance_through {
            false
        } else {
            return Err(SyncError::InvalidSample);
        };
        self.arrival_feedback = feedback;
        self.arrival_slack = Some(sample.slack);
        if let Some(index) = index {
            self.issued_commands[index].feedback_seen = true;
        }
        if !eligible {
            self.arrival_feedback_ignored = self.arrival_feedback_ignored.saturating_add(1);
            return Ok(());
        }
        self.arrival_feedback_applied = self.arrival_feedback_applied.saturating_add(1);
        let previous = self.lead.lead();
        self.lead.observe_arrival_slack(i32::from(sample.slack))?;
        if self.lead.lead() != previous {
            self.lead_policy = self
                .lead_policy
                .checked_add(1)
                .ok_or(SyncError::TimeRange)?;
            self.lead_policy_head = None;
        }
        Ok(())
    }
    fn unstarted_prediction(&self) -> bool {
        self.sequence == CommandSeq(0)
            && self
                .predictor
                .predicted_tick(OWNER_GROUP)
                .is_some_and(|tick| {
                    self.predictor
                        .replay_input_history(OWNER_GROUP, tick)
                        .is_none()
                        && self
                            .predictor
                            .state(OWNER_GROUP)
                            .is_some_and(|state| state.0.stamp().server_tick == tick.0)
                })
    }
    pub(super) fn capture_combat_view(&mut self, at: Duration) -> Option<CombatViewStamp> {
        if !self.active || self.recovering {
            return None;
        }
        let rate = TickRate::new(dreamwake_sim::TICK_HZ)?;
        let server_now = self.clock.estimate_server_time(at).ok()?;
        let sampled_time = self
            .simulation_clock
            .estimate_elapsed(server_now, self.lead.lead())
            .ok()?;
        let (sampled_at, sampled_fraction) = rate.elapsed_tick_phase(sampled_time)?;
        let (viewed_at, viewed_fraction) =
            rate.elapsed_tick_phase(self.poses.combat_view_time()?)?;
        if (viewed_at, viewed_fraction) > (sampled_at, sampled_fraction) {
            return None;
        }
        let reference = reference_for_view(&self.combat_references, viewed_at)?;
        Some(CombatViewStamp {
            sampled_at,
            sampled_fraction,
            viewed_at,
            viewed_fraction,
            reference,
        })
    }
    pub(super) fn advance_presentation_clock(&mut self, at: Duration) {
        if let Some(state) = self.scopes.state(self.model.welcome.global_entity)
            && let Ok(ReplicaPayload::Global(global)) = decode_replica(&state.payload, None)
        {
            self.poses
                .set_paused(global.paused, ServerTick(global.stamp.server_tick));
        }
        if let Ok(elapsed) = self.clock.estimate_server_elapsed(at) {
            self.estimated_server_tick =
                TickRate::new(dreamwake_sim::TICK_HZ).and_then(|rate| rate.elapsed_ticks(elapsed));
        }
        if let Ok(server_now) = self.clock.estimate_server_time(at)
            && let Ok(elapsed) = self
                .simulation_clock
                .estimate_elapsed(server_now, self.lead.lead())
        {
            self.estimated_simulation_tick =
                TickRate::new(dreamwake_sim::TICK_HZ).and_then(|rate| rate.elapsed_ticks(elapsed));
            self.poses.advance_time(elapsed);
        }
    }
    /// Capture optional public targeting inputs once. Replay reads the retained
    /// dependency frame, never a newer or retired presentation replica.
    pub(super) fn prediction_dependencies(
        &self,
        at: ServerTick,
    ) -> Result<DependencyFrame<CollisionDependency>, String> {
        let mut dependencies = self
            .predictor
            .checkpoint_dependencies(OWNER_GROUP)
            .cloned()
            .ok_or("missing admitted checkpoint dependencies")?;
        let owner = self
            .predictor
            .state(OWNER_GROUP)
            .ok_or("missing owner prediction")?
            .0
            .hero_view()
            .position;
        let mut targets = Vec::new();
        for entity in &self.entities {
            let Some(state) = self.scopes.state(*entity) else {
                continue;
            };
            if state.end_tick > at {
                continue;
            }
            let ReplicaPayload::Actor(dreamwake_sim::replication::PublicReplica::Enemy(target)) =
                decode_replica(&state.payload, Some(self.model.expectation())).map_err(err)?
            else {
                continue;
            };
            if target.hp <= 0.0 {
                continue;
            }
            let distance = engine_core::spatial::distance(target.position, owner);
            targets.push((distance, state.scope, state.version, target));
        }
        targets.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.entity.cmp(&b.1.entity)));
        for (_, scope, version, target) in targets.into_iter().take(MAX_PRESENTATION_TARGETS) {
            dependencies.records.insert(
                scope.entity,
                DependencyRecord {
                    scope,
                    version,
                    scene_revision: self.model.welcome.content.scene_revision,
                    value: DependencyValue::Known(CollisionDependency::PresentationTarget(target)),
                },
            );
        }
        Ok(dependencies)
    }
    pub(super) fn next_action(
        &mut self,
        edges: &[DreamAction],
    ) -> Result<Option<DreamAction>, String> {
        if self.action_backlog.len() + edges.len() > 32 {
            return Err("Pending action capacity".into());
        }
        self.action_backlog.extend(edges.iter().copied());
        Ok(self
            .action_backlog
            .pop_front()
            .and_then(|edge| self.resolve_charge_edge(edge)))
    }
    fn resolve_charge_edge(&mut self, action: DreamAction) -> Option<DreamAction> {
        Some(match action {
            DreamAction::ChargeBegin => {
                self.charge_episode = Some(self.sequence.0);
                DreamAction::ChargeBegin
            }
            DreamAction::ChargeRelease { episode: 0 } => DreamAction::ChargeRelease {
                episode: self.charge_episode.take()?,
            },
            DreamAction::ChargeCancel { episode: 0 } => DreamAction::ChargeCancel {
                episode: self.charge_episode.take()?,
            },
            action => action,
        })
    }
    pub(super) fn command_sample(
        &mut self,
        tick: TargetTick,
        newest: TargetTick,
        now: Duration,
    ) -> Result<engine_net::input::JournalFrame<TickInput, DreamAction>, String> {
        let behind = newest
            .0
            .checked_sub(tick.0)
            .ok_or("input target ordering")?;
        let offset = TickRate::new(dreamwake_sim::TICK_HZ)
            .unwrap()
            .deadline(ServerTick(behind))
            .ok_or("input clock range")?;
        let deadline = now
            .saturating_sub(offset)
            .max(self.input_deadline.unwrap_or(Duration::ZERO));
        let sample = self.journal.drain_until(deadline).map_err(err)?;
        self.input_deadline = Some(deadline);
        Ok(sample)
    }
    pub(super) fn begin_frame(&mut self) -> Result<(), String> {
        self.frame = self.frame.checked_add(1).ok_or("frame exhausted")?;
        self.predictor
            .begin_frame(ReplicationFrame(self.frame))
            .map_err(err)
    }
    pub(super) fn acknowledge_active(&mut self, owner_snapshot: SnapshotId, through: ServerTick) {
        self.acknowledged_active = Some((owner_snapshot, through));
        self.finalized = self.finalized.max(through);
        self.simulation_clock.commit(through);
    }
    /// Activation is scoped to this authenticated Session and remains valid after
    /// its initial proof ages out of bounded decode history. Recovery/new Welcome
    /// explicitly resets it; ordinary checkpoint retention is not revocation.
    pub(super) fn update_connection(
        &self,
        connection: &mut crate::DreamConnection,
        rtt_ms: f64,
        party_size: usize,
    ) {
        connection.connected = self.active && !self.recovering;
        connection.client_id = self.model.welcome.client.0;
        connection.player_id = self.model.welcome.player.get();
        connection.party_size = party_size;
        connection.rtt_ms = rtt_ms;
        connection.status = if connection.connected {
            "Connected to the shared dream"
        } else {
            "Synchronizing the shared dream…"
        }
        .into();
    }
    pub(super) fn control(&mut self, message: &ClientControl) -> Result<Vec<u8>, String> {
        self.control_sequence = self
            .control_sequence
            .checked_add(1)
            .ok_or("control sequence exhausted")?;
        encode_control(
            message,
            self.model.welcome.stream.epoch,
            self.control_sequence,
            self.model.welcome.frame_limit().map_err(err)?,
        )
        .map_err(err)
    }
    /// Queue only completed public-state decode proofs. Every receipt is kept,
    /// including successive scope incarnations for the same entity.
    pub(super) fn queue_decoded_receipt(&mut self, receipt: StateReceipt) -> Option<ClientControl> {
        self.pending_decoded_receipts
            .push(receipt)
            .expect("decoded receipt batch is drained at capacity");
        if self.pending_decoded_receipts.len() == DECODED_RECEIPTS_PER_CONTROL {
            self.take_decoded_receipts()
        } else {
            None
        }
    }
    /// Flush at the end of the same receive pass; batching never waits for a
    /// future frame, tick or timer. A new Session starts with no queued proofs.
    pub(super) fn take_decoded_receipts(&mut self) -> Option<ClientControl> {
        if self.pending_decoded_receipts.is_empty() {
            return None;
        }
        Some(ClientControl::Decoded {
            receipts: BoundedVec::new(
                std::mem::take(&mut self.pending_decoded_receipts).into_vec(),
            )
            .expect("eight receipts fit the protocol's twelve-receipt capacity"),
        })
    }
    /// Call only after publication succeeds. In particular, a new attachment
    /// has no pose sample but still retires the older generation's pose/history.
    fn remember_published_entity(&mut self, entity: EntityId) {
        let first = EntityId {
            generation: 0,
            ..entity
        };
        while let Some(old) = self.entities.range(first..entity).next().copied() {
            self.entities.remove(&old);
            self.poses.remove(old);
        }
        self.entities.insert(entity);
    }
    pub(super) fn expire_groups(&mut self, now: Duration) -> Result<usize, ReplicationError> {
        let mut expired = self.groups.expire(now.as_millis() as u64)?;
        if self.pending_reset_group.as_ref().is_some_and(|group| {
            now.saturating_sub(group.started_at).as_millis()
                >= u128::from(group_limits().max_age_ms)
        }) {
            self.pending_reset_group = None;
            expired += 1;
        }
        Ok(expired)
    }
    pub(super) fn receive_pending_reset_group(
        &mut self,
        now: Duration,
    ) -> Result<Option<(ClientControl, Duration)>, super::ReceiveRejection> {
        let Some(pending) = &self.pending_reset_group else {
            return Ok(None);
        };
        if now.saturating_sub(pending.started_at).as_millis()
            >= u128::from(group_limits().max_age_ms)
        {
            return Err(super::ReceiveRejection::replication(
                ReplicationError::Expired,
                || "owner group awaiting baseline reset".into(),
            ));
        }
        let bytes = pending.bytes.clone();
        let received_at = pending.received_at;
        Ok(self
            .receive_state(&bytes, now)?
            .map(|receipt| (receipt, received_at)))
    }
    pub(super) fn receive_state(
        &mut self,
        bytes: &[u8],
        now: Duration,
    ) -> Result<Option<ClientControl>, super::ReceiveRejection> {
        let welcome = &self.model.welcome;
        let expected = self.model.expectation();
        let frame = decode_state(
            bytes,
            welcome.stream.epoch,
            welcome.frame_limit().map_err(err)?,
        )
        .map_err(err)?;
        let receipts = match frame {
            StateFrame::Full(state) => {
                // Global and owner checkpoints are only accepted together through
                // the atomic publication path, including subsequent corrections.
                if state.scope.entity == welcome.global_entity
                    || state.scope.entity == welcome.owner_entity
                    || state.scope.entity == welcome.collision_entity
                    || !matches!(
                        decode_replica(&state.payload, Some(expected)),
                        Ok(ReplicaPayload::Actor(_))
                    )
                {
                    return Err("non-public standalone state".into());
                }
                let entity = state.scope.entity;
                let incoming = (state.receipt(), state.end_tick);
                let current = self.scopes.fence(entity.index);
                let receipt = self.scopes.apply_full(state, |_| true).map_err(|error| {
                    super::ReceiveRejection::replication(error, || {
                        format!("public Full incoming={incoming:?} current={current:?}")
                    })
                })?;
                if let Some(state) = self.scopes.state(entity) {
                    if let Ok(ReplicaPayload::Actor(actor)) =
                        decode_replica(&state.payload, Some(expected))
                    {
                        self.poses
                            .receive(state.scope, state.end_tick, &actor)
                            .map_err(|e| format!("remote presentation: {e:?}"))?;
                    }
                }
                self.remember_published_entity(entity);
                vec![receipt]
            }
            StateFrame::Group(group) => {
                self.delivery.group_messages = self.delivery.group_messages.saturating_add(1);
                let publication = group.publication;
                let model = &self.model;
                let current_group = self.groups.publication(publication.group);
                let mut staged = self.baselines.stage().map_err(err)?;
                let mut proofs = Vec::new();
                let result = (|| {
                    let mut decoded = group;
                    if !baseline_wire::valid_member_count(decoded.members.len())
                        || [
                            model.welcome.global_entity,
                            model.welcome.owner_entity,
                            model.welcome.collision_entity,
                        ]
                        .iter()
                        .any(|entity| !decoded.manifest.iter().any(|scope| scope.entity == *entity))
                    {
                        return Err(engine_net::replication::ReplicationError::InvalidPayload);
                    }
                    if current_group.is_some_and(|current| current.revision == publication.revision)
                        && self
                            .groups
                            .manifest(publication.group)
                            .is_some_and(|manifest| manifest != decoded.manifest)
                    {
                        return Err(ReplicationError::Conflict);
                    }
                    // Decode every bounded envelope before using age evidence.
                    // Inspect the original caches before manifest retention can
                    // discard a removed or reentered platform's newer context.
                    let packets = decoded
                        .members
                        .iter()
                        .map(|state| {
                            baseline_wire::decode_member(&state.payload, state, decoded.publication)
                                .map_err(|_| ReplicationError::InvalidPayload)
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    if let Some(error) = packets
                        .iter()
                        .find_map(|packet| staged.obsolete_context(packet))
                    {
                        let all_full = packets
                            .iter()
                            .all(|packet| matches!(packet.encoding, BaselineEncoding::Full));
                        for (state, packet) in decoded.members.iter_mut().zip(packets) {
                            if matches!(packet.encoding, BaselineEncoding::Full) {
                                state.payload = packet.bytes;
                                if !all_full {
                                    decode_owner_member(model, state, decoded.end_tick)
                                        .map_err(|_| ReplicationError::InvalidPayload)?;
                                }
                            } else {
                                // A Delta implies an earlier decoded scope. An
                                // unrelated or future optional member cannot use
                                // another member's older revision as age proof.
                                let incoming = packet.target.context.scope;
                                let known = self
                                    .scopes
                                    .fence(incoming.entity.index)
                                    .ok_or(ReplicationError::InvalidIdentity)?
                                    .scope;
                                if known.connection != incoming.connection
                                    || known.entity.generation < incoming.entity.generation
                                    || (known.entity.generation == incoming.entity.generation
                                        && (known.scope < incoming.scope
                                            || known.representation < incoming.representation
                                            || (known.scope == incoming.scope
                                                && known.representation
                                                    != incoming.representation)))
                                {
                                    return Err(ReplicationError::Conflict);
                                }
                            }
                        }
                        if all_full {
                            validate_owner_group(
                                model,
                                decoded.publication.group,
                                decoded.end_tick,
                                decoded.members.iter(),
                            )
                            .map_err(|_| ReplicationError::InvalidPayload)?;
                        }
                        return Err(error);
                    }
                    staged.retain_manifest(&decoded.manifest);
                    let mut reset_pending = false;
                    let mut obsolete_full = None;
                    for (state, packet) in decoded.members.iter_mut().zip(packets) {
                        let rejected_seed = staged.rejected_full_seed(&packet);
                        match staged.reconstruct(state, packet) {
                            Ok(receipt) => proofs.push(baseline_wire::proof(state, receipt)),
                            Err(ReplicationError::BaselineResetPending) => {
                                state.payload =
                                    rejected_seed.ok_or(ReplicationError::InvalidIdentity)?;
                                reset_pending = true;
                            }
                            Err(error @ ReplicationError::Obsolete(_))
                                if rejected_seed.is_some() =>
                            {
                                state.payload = rejected_seed.unwrap();
                                obsolete_full = Some(error);
                            }
                            Err(error) => return Err(error),
                        }
                    }
                    validate_owner_group(
                        model,
                        decoded.publication.group,
                        decoded.end_tick,
                        decoded.members.iter(),
                    )
                    .map_err(|_| engine_net::replication::ReplicationError::InvalidPayload)?;
                    if let Some(error) = obsolete_full {
                        return Err(error);
                    }
                    if reset_pending {
                        return Err(ReplicationError::BaselineResetPending);
                    }
                    self.groups
                        .receive(decoded, now.as_millis() as u64, &mut self.scopes, |_| true)
                })();
                let progress = match result {
                    Ok(progress) => progress,
                    Err(ReplicationError::BaselineResetPending) => {
                        self.delivery.baseline_reset_pending =
                            self.delivery.baseline_reset_pending.saturating_add(1);
                        // Only fully decoded and game-validated members reach here.
                        // A later message can replace the bounded slot, but cannot
                        // extend the deadline for the missing reliable reset.
                        if self
                            .pending_reset_group
                            .as_ref()
                            .is_none_or(|pending| publication > pending.publication)
                        {
                            let started_at = self
                                .pending_reset_group
                                .as_ref()
                                .map_or(now, |pending| pending.started_at);
                            self.pending_reset_group = Some(PendingResetGroup {
                                publication,
                                bytes: bytes.into(),
                                received_at: now,
                                started_at,
                            });
                        }
                        return Ok(None);
                    }
                    Err(
                        engine_net::replication::ReplicationError::Unknown
                        | engine_net::replication::ReplicationError::Capacity,
                    ) => {
                        let repairs = self.baselines.repairs(now.as_millis() as u64);
                        if repairs.is_empty() {
                            return Ok(None);
                        }
                        self.baseline_repairs = self.baseline_repairs.saturating_add(1);
                        return Ok(Some(ClientControl::BaselineRepair {
                            requests: BoundedVec::new(repairs).map_err(err)?,
                        }));
                    }
                    Err(error @ ReplicationError::Obsolete(_)) => {
                        self.delivery.baseline_stale =
                            self.delivery.baseline_stale.saturating_add(1);
                        return Err(super::ReceiveRejection::replication(error, || {
                            format!("Group incoming={publication:?} current={current_group:?}")
                        }));
                    }
                    Err(error) => {
                        return Err(super::ReceiveRejection::replication(error, || {
                            format!(
                                "Group incoming={publication:?} current={current_group:?} receive_ms={}",
                                now.as_millis()
                            )
                        }));
                    }
                };
                match progress {
                    GroupProgress::Staged => return Ok(None),
                    GroupProgress::Published(receipts) => {
                        if self
                            .pending_reset_group
                            .as_ref()
                            .is_some_and(|pending| publication >= pending.publication)
                        {
                            self.pending_reset_group = None;
                        }
                        self.delivery.groups_completed =
                            self.delivery.groups_completed.saturating_add(1);
                        self.baselines = staged;
                        for receipt in &receipts {
                            if let Some(state) = self.scopes.state(receipt.scope.entity) {
                                if let Ok(ReplicaPayload::Actor(actor)) =
                                    decode_replica(&state.payload, Some(self.model.expectation()))
                                {
                                    self.poses
                                        .receive(state.scope, state.end_tick, &actor)
                                        .map_err(|error| {
                                            format!("group presentation: {error:?}")
                                        })?;
                                }
                            }
                            self.remember_published_entity(receipt.scope.entity);
                        }
                        if let Some(state) = self.scopes.state(self.model.welcome.owner_entity) {
                            self.simulation_clock.commit(state.end_tick);
                            self.decoded_groups.insert(state.snapshot, state.end_tick);
                            self.combat_references
                                .push_back((state.receipt(), state.end_tick));
                            while self.combat_references.len() > 64 {
                                self.combat_references.pop_front();
                            }
                            while self.decoded_groups.len() > 32 {
                                self.decoded_groups.pop_first();
                            }
                        }
                        return Ok(Some(ClientControl::OwnerDecoded {
                            proofs: BoundedVec::new(proofs).map_err(err)?,
                        }));
                    }
                }
            }
        };
        Ok(Some(ClientControl::Decoded {
            receipts: BoundedVec::new(receipts).map_err(err)?,
        }))
    }
    pub(super) fn reconcile(&mut self) -> Result<(), String> {
        if self.recovering {
            return Ok(());
        }
        let Some(proof) = self.groups.published_group(OWNER_GROUP, &self.scopes) else {
            return Ok(());
        };
        // Admission may publish an inactive owner first and a committed active
        // owner second. No speculative commands exist before this gate opens.
        if !self.active && self.sequence.0 == 0 {
            self.predictor.remove_group(OWNER_GROUP);
        }
        if self.predictor.group_count() == 0 {
            self.predictor
                .initialize(&proof, self.model.binding(), &self.model)
                .map_err(err)?;
        } else {
            // State and reliable finalization proofs travel on independent lanes.
            // A decoded checkpoint is retained while its command-finalization
            // proof catches up; arrival ordering is not a prediction failure.
            if proof.end_tick() > self.finalized {
                return Ok(());
            }
            let predicted = self
                .predictor
                .predicted_tick(OWNER_GROUP)
                .ok_or("missing prediction history")?;
            self.delivery.replay_requested = predicted.0.saturating_sub(proof.end_tick().0);
            self.delivery.maximum_replay_requested = self
                .delivery
                .maximum_replay_requested
                .max(self.delivery.replay_requested);
            let same_identity = self
                .predictor
                .identity(OWNER_GROUP)
                .is_some_and(|identity| {
                    identity.binding == self.model.binding()
                        && identity.group_revision == proof.publication().revision
                        && identity.scopes == proof.manifest().iter().copied().collect()
                });
            let advanced_topology = self
                .predictor
                .identity(OWNER_GROUP)
                .is_some_and(|identity| {
                    identity.binding == self.model.binding()
                        && proof.publication().revision > identity.group_revision
                        && [
                            self.model.welcome.global_entity,
                            self.model.welcome.owner_entity,
                            self.model.welcome.collision_entity,
                        ]
                        .into_iter()
                        .all(|entity| {
                            identity.scopes.iter().find(|scope| scope.entity == entity)
                                == proof.manifest().iter().find(|scope| scope.entity == entity)
                        })
                });
            if advanced_topology {
                // An intermediate closure may be lost on the state lane. A
                // later revision can therefore contain the same scopes again;
                // it still requires an atomic replacement and bounded replay.
                let unstarted = self.unstarted_prediction();
                self.predictor =
                    replace_topology(&self.predictor, &self.model, &proof, self.frame, unstarted)
                        .map_err(|error| {
                        format!(
                            "{error}; active={} sequence={}",
                            self.active, self.sequence.0
                        )
                    })?;
            } else if proof.end_tick() > predicted
                && (proof.end_tick().0 - predicted.0
                    <= u64::from(engine_net::types::MAX_COMMAND_LEAD_TICKS)
                    || self.unstarted_prediction())
                && same_identity
            {
                // Receive precedes command generation. A small authoritative
                // advance can therefore outrun the last rendered prediction.
                // Before the first command, any newer complete checkpoint needs
                // no replay; no speculative history or assigned intent exists.
                // Every discarded target is already finalized; adopt a fully
                // decoded replacement atomically, without inventing inputs or
                // discarding the hardware journal/future command sequence.
                let mut replacement = new_predictor(self.model.welcome.stream.epoch)?;
                replacement
                    .begin_frame(ReplicationFrame(self.frame))
                    .map_err(err)?;
                replacement
                    .initialize(&proof, self.model.binding(), &self.model)
                    .map_err(err)?;
                self.predictor = replacement;
            } else {
                let started = bevy::platform::time::Instant::now();
                let result = self.predictor.reconcile(
                    &proof,
                    self.model.binding(),
                    self.finalized,
                    &self.model,
                );
                self.delivery.replay_cpu_micros =
                    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
                self.delivery.maximum_replay_cpu_micros = self
                    .delivery
                    .maximum_replay_cpu_micros
                    .max(self.delivery.replay_cpu_micros);
                result.map_err(err)?;
            }
        }
        self.events.publish(&self.predictor)?;
        // Binding is source-owned: require the owner checkpoint's exact game
        // projectile ID and an admitted public projectile scope for that ID.
        let bindings = self
            .predictor
            .state(OWNER_GROUP)
            .into_iter()
            .flat_map(|s| s.0.starfall_flights())
            .filter_map(|flight| {
                let id = flight.authority_id?;
                self.entities.iter().find_map(|entity| {
                    let state = self.scopes.state(*entity)?;
                    match decode_replica(&state.payload, None).ok()? {
                        ReplicaPayload::Actor(
                            dreamwake_sim::replication::PublicReplica::Projectile(p),
                        ) if p.id == id && p.owner == Some(self.model.welcome.player.get()) => {
                            Some((flight.key, *entity))
                        }
                        _ => None,
                    }
                })
            })
            .collect::<Vec<_>>();
        for (key, entity) in bindings {
            self.events.bind(key, entity)?;
        }
        if !self.active
            && let Some((snapshot, through)) = self.acknowledged_active
        {
            self.active = self
                .decoded_groups
                .get(&snapshot)
                .is_some_and(|tick| through >= *tick);
        }
        Ok(())
    }
    /// Read the committed owner checkpoint without depending on render readiness
    /// or on whether prediction has reconciled its newer publication yet.
    pub(crate) fn authoritative_owner_pose(
        &self,
    ) -> Result<Option<(ServerTick, [f32; 2])>, String> {
        let Some(state) = self.scopes.state(self.model.welcome.owner_entity) else {
            return Ok(None);
        };
        let ReplicaPayload::Owner(owner) =
            decode_replica(&state.payload, Some(self.model.expectation())).map_err(err)?
        else {
            return Err("owner scope type".into());
        };
        if owner.stamp().server_tick != state.end_tick.0 {
            return Err("owner checkpoint time".into());
        }
        Ok(Some((state.end_tick, owner.hero_view().position)))
    }
    pub(crate) fn presentation(
        &self,
        predicted: bool,
    ) -> Result<Option<DreamPresentation>, String> {
        let Some(global) = self.scopes.state(self.model.welcome.global_entity) else {
            return Ok(None);
        };
        let ReplicaPayload::Global(global) =
            decode_replica(&global.payload, Some(self.model.expectation())).map_err(err)?
        else {
            return Err("global scope type".into());
        };
        let owner = if predicted {
            self.predictor.state(OWNER_GROUP).map(|v| v.0.clone())
        } else {
            self.scopes
                .state(self.model.welcome.owner_entity)
                .and_then(|s| {
                    match decode_replica(&s.payload, Some(self.model.expectation())).ok()? {
                        ReplicaPayload::Owner(owner) => {
                            dreamwake_sim::replication::OwnerPredictionState::from_checkpoint(
                                &owner,
                                self.model.expectation(),
                            )
                            .ok()
                        }
                        _ => None,
                    }
                })
        };
        let Some(owner) = owner else {
            return Ok(None);
        };
        let mut actors = Vec::new();
        let mut cover_markers = Vec::new();
        for entity in &self.entities {
            let Some(state) = self.scopes.state(*entity) else {
                continue;
            };
            if let ReplicaPayload::Actor(mut actor) =
                decode_replica(&state.payload, Some(self.model.expectation())).map_err(err)?
            {
                if let dreamwake_sim::replication::PublicReplica::CoverMarker(marker) = &actor {
                    let attachment = marker.attachment();
                    if let Ok(engine_net::replication::attachments::AttachmentResolution::Parent {
                        local,
                        parent,
                    }) = attachment.resolve(state.scope, &self.scopes)
                        && let Ok(ReplicaPayload::Actor(mut parent)) = decode_replica(parent, None)
                        && matches!(&parent, dreamwake_sim::replication::PublicReplica::Cover(cover) if cover.present)
                    {
                        if predicted {
                            self.poses.apply(marker.parent, &mut parent);
                        }
                        if let dreamwake_sim::replication::PublicReplica::Cover(cover) = parent {
                            cover_markers.push(
                                dreamwake_sim::replication::CoverMarkerPresentation {
                                    id: marker.id,
                                    position: std::array::from_fn(|axis| {
                                        cover.position[axis] + local[axis]
                                    }),
                                    open: cover.open,
                                },
                            );
                        }
                    }
                }
                if predicted
                    && matches!(
                        actor,
                        dreamwake_sim::replication::PublicReplica::Projectile(_)
                    )
                    && self.events.suppress_public(*entity)
                {
                    continue;
                }
                if predicted && !self.poses.apply(state.scope, &mut actor) {
                    continue;
                }
                actors.push(actor);
            }
        }
        // Unordered public scopes may arrive before an independently entitled
        // source scope. Retain the replica, but wait to render its reference.
        let mut sources = BTreeSet::from([owner.owner()]);
        for actor in &actors {
            match actor {
                dreamwake_sim::replication::PublicReplica::Hero(v) => {
                    sources.insert(v.id);
                }
                dreamwake_sim::replication::PublicReplica::Enemy(v) => {
                    sources.insert(v.id);
                }
                _ => {}
            }
        }
        actors.retain(|actor| match actor {
            dreamwake_sim::replication::PublicReplica::Projectile(v) => {
                v.owner.is_none_or(|id| sources.contains(&id))
            }
            dreamwake_sim::replication::PublicReplica::Wisp(v) => {
                v.owner.is_none_or(|id| sources.contains(&id))
            }
            dreamwake_sim::replication::PublicReplica::Effect(v) => {
                v.id.owner == 0 || sources.contains(&v.id.owner)
            }
            _ => true,
        });
        let Some(tick) = self.predictor.predicted_tick(OWNER_GROUP) else {
            return Ok(None);
        };
        let collision_entity = self.model.welcome.collision_entity;
        let Some(current) = self.scopes.state(collision_entity) else {
            return Ok(None);
        };
        let Some(record) = self
            .predictor
            .dependency_history(OWNER_GROUP, tick)
            .and_then(|frame| frame.records.get(&collision_entity))
        else {
            return Ok(None);
        };
        // A newer publication in the same entered scope does not revoke a
        // retained prediction dependency. Finalization may arrive later on the
        // reliable lane. The owner hash and scene below bind the exact geometry.
        if record.scope != current.scope {
            return Ok(None);
        }
        let DependencyValue::Known(CollisionDependency::StaticCollision(collision)) = &record.value
        else {
            return Ok(None);
        };
        let mut presentation =
            DreamPresentation::from_replicas(&global, &owner, &actors).map_err(err)?;
        presentation.cover_markers = cover_markers;
        if predicted {
            presentation.elapsed = self.poses.presentation_elapsed(&global);
            // Retain already-proven retirement fences from the existing action
            // binding lane while an older owner checkpoint is still in flight.
            // Entity creation and advancement come only from shared simulation.
            let retired = owner
                .starfall_flights()
                .iter()
                .filter(|flight| !self.events.represented(flight.key))
                .filter_map(|flight| owner.projectile_visual_id(flight.key))
                .collect::<BTreeSet<_>>();
            presentation
                .projectiles
                .retain(|shot| !retired.contains(&shot.id));
            if let Some(base) = owner.base_attachment() {
                for platform in &mut presentation.platforms {
                    if platform.collider == base.collider
                        && platform.scene_revision == base.scene_revision
                        && let Ok(pose) = platform.pose_at(owner.gameplay_tick())
                    {
                        // A local rider and its declared base share the predicted
                        // display time. Remote presentation remains at common R.
                        platform.pose = pose;
                        platform.gameplay_tick = owner.gameplay_tick();
                        platform.phase = ((platform.gameplay_tick - platform.origin_gameplay_tick)
                            % u32::from(dreamwake_sim::platform::PLATFORM_PERIOD_TICKS))
                            as u16;
                    }
                }
            }
        }
        // Retain the admitted immutable manifest already used by prediction.
        // Presentation never rebuilds or substitutes current renderer geometry.
        presentation
            .attach_collision(collision, &owner)
            .map_err(err)?;
        Ok(Some(presentation))
    }
    pub(super) fn recover(&mut self) {
        self.pending_reset_group = None;
        self.active = false;
        self.recovering = true;
        if self.resync_request == ResyncRequest::Idle {
            self.resync_request = ResyncRequest::PendingEnqueue;
        }
        self.last_input_retry_committed = None;
        self.issued_commands.clear();
        self.retired_issuance_through = self.retired_issuance_through.max(self.sequence);
        self.lead_policy_head = None;
        self.journal.reset(TickInput::default());
        self.action_backlog.clear();
        self.charge_episode = None;
        self.input_deadline = None;
    }
}
fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// A new, fully authorized base closure may replace only the topology. Stable
/// control binding and mandatory scopes were checked before entering this path.
/// Nothing becomes visible until every retained input frame replays successfully.
fn replace_topology(
    old: &PredictionManager<OwnerModel>,
    model: &OwnerModel,
    proof: &PublishedGroup<'_, Vec<u8>>,
    frame: u64,
    unstarted: bool,
) -> Result<PredictionManager<OwnerModel>, String> {
    let predicted = old
        .predicted_tick(OWNER_GROUP)
        .ok_or("missing topology history")?;
    let replay = predicted.0.saturating_sub(proof.end_tick().0);
    let work = old.work();
    if replay > 32
        || work.replay_ticks != 0
        || work.predicted_ticks != 0
        // A pure authoritative checkpoint has no input to replay. A newer
        // complete closure can replace it when no replayable transition remains.
        || (!unstarted && proof.end_tick().0 > predicted.0.saturating_add(12))
    {
        return Err(format!(
            "topology replay exceeds frame work budget: predicted={} incoming={} revision={} replay={} work_replay={} work_predicted={} unstarted={unstarted}",
            predicted.0,
            proof.end_tick().0,
            proof.publication().revision.0,
            replay,
            work.replay_ticks,
            work.predicted_ticks,
        ));
    }
    // Retain both managers until commit, including worst-case checkpoint,
    // dependency and command storage plus map/event bookkeeping for each frame.
    let staging = (replay as usize + 1) * (80 * 1024 + COLLISION_HISTORY_BYTES + 8192);
    if old.retained_bytes().saturating_add(staging) > 64 * 1024 * 1024 {
        return Err("topology replay exceeds aggregate history bytes".into());
    }
    let mut next = new_predictor(model.welcome.stream.epoch)?;
    next.begin_frame(ReplicationFrame(frame)).map_err(err)?;
    next.initialize(proof, model.binding(), model)
        .map_err(err)?;
    let prior = old
        .history_state(OWNER_GROUP, proof.end_tick())
        .or_else(|| old.state(OWNER_GROUP))
        .ok_or("missing source identity at topology boundary")?;
    if next
        .state(OWNER_GROUP)
        .is_none_or(|state| state.0.combat_generation() != prior.0.combat_generation())
    {
        return Err("changed owner source requires fresh admission".into());
    }
    for tick in proof.end_tick().0.saturating_add(1)..=predicted.0 {
        let input = old
            .replay_input_history(OWNER_GROUP, ServerTick(tick))
            .ok_or("missing topology replay input")?
            .clone();
        let dependencies = next
            .dependency_history(OWNER_GROUP, ServerTick(tick - 1))
            .ok_or("missing topology replay dependency")?
            .clone();
        match input {
            ReplayInput::Command(command) => {
                next.predict(OWNER_GROUP, command, dependencies, model)
            }
            ReplayInput::Substitute => {
                next.predict_substitute(OWNER_GROUP, ServerTick(tick), dependencies, model)
            }
        }
        .map_err(err)?;
    }
    Ok(next)
}

fn new_predictor(epoch: ConnectionEpoch) -> Result<PredictionManager<OwnerModel>, String> {
    PredictionManager::new(
        epoch,
        PredictionLimits {
            // Includes the inline state and map metadata in addition to the
            // owner's bounded 64 KiB decoded allocation envelope.
            checkpoint_bytes: 80 * 1024,
            dependency_bytes: COLLISION_HISTORY_BYTES,
            ..Default::default()
        },
    )
    .map_err(err)
}

fn reference_for_view(
    references: &std::collections::VecDeque<(StateReceipt, ServerTick)>,
    viewed_at: ServerTick,
) -> Option<StateReceipt> {
    references
        .iter()
        .rev()
        .find(|(_, tick)| *tick <= viewed_at)
        .map(|(receipt, _)| *receipt)
}
#[cfg(test)]
mod combat_reference_tests {
    use super::*;
    #[test]
    fn new_owner_checkpoint_cannot_move_reference_past_frozen_render_tick() {
        let old = StateReceipt {
            scope: ScopeIdentity {
                connection: ConnectionEpoch(1),
                entity: EntityId {
                    index: 1,
                    generation: 1,
                },
                scope: ScopeEpoch(1),
                representation: RepresentationRevision(1),
            },
            baseline_generation: BaselineGeneration(1),
            snapshot: SnapshotId(1),
            version: StateVersion(1),
        };
        let mut references = [(old, ServerTick(90))].into();
        let frozen_render_tick = ServerTick(92);
        assert_eq!(
            reference_for_view(&references, frozen_render_tick),
            Some(old)
        );
        for tick in 93..110 {
            references.push_back((
                StateReceipt {
                    snapshot: SnapshotId(tick),
                    ..old
                },
                ServerTick(tick),
            ));
            assert_eq!(
                reference_for_view(&references, frozen_render_tick),
                Some(old)
            );
        }
        assert_eq!(reference_for_view(&references, ServerTick(89)), None);
    }
}
