//! The connection's live state, independent of Bevy and hardware polling.
//!
//! State packets arrive unreliably. Each entry is decoded against the state
//! this client acknowledged, the newest owner state replaces prediction from
//! its tick, and nothing waits for anything to be resent: a lost packet is
//! covered by the next one.
use super::model::*;
use dreamwake_protocol::{DreamAction, live::*};
use dreamwake_sim::replication::{
    DreamPresentation, OwnerCheckpoint, OwnerPredictionState, PublicReplica, Replica, ReplicaKind,
    ReplicaState,
};
use engine_net::{
    codec::{BoundedVec, FrameLimit},
    input::InputJournal,
    prediction::rewind::Correction,
    replication::stream::{Base, ClientStream},
    synchronization::{
        ClockConfig, ClockEstimator, ClockSample, CommittedClock, LeadController, ResyncReason,
        SyncError, TimeExchange,
    },
    types::*,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

/// Entities this client can hold at once.
const MAX_ENTITIES: usize = 4096;

#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub(crate) struct DeliveryMetrics {
    pub state_packets: u64,
    pub rejected_packets: u64,
    pub replayed: u64,
    pub rebased: u64,
    pub stale: u64,
    pub replay_cpu_micros: u64,
    pub maximum_replay_cpu_micros: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionMetrics {
    pub server_instance: u64,
    /// Server transport peer identity; stream epochs may change independently.
    pub transport_connection: u64,
    pub active: bool,
    pub connection_epoch: ConnectionEpoch,
    pub match_epoch: u32,
    pub estimated_server_tick: Option<ServerTick>,
    pub estimated_simulation_tick: Option<ServerTick>,
    pub committed_server_tick: ServerTick,
    pub clock_anchor_tick: ServerTick,
    pub owner_tick: Option<ServerTick>,
    pub finalized_tick: ServerTick,
    pub predicted_tick: Option<ServerTick>,
    pub gameplay_tick: Option<u32>,
    pub combat_view_tick: Option<ServerTick>,
    pub entities: usize,
    pub stream_latest: u32,
    pub lead_ticks: u8,
    pub arrival_slack: Option<u8>,
    pub arrival_feedback: ArrivalFeedback,
    pub arrival_feedback_applied: u64,
    pub arrival_feedback_ignored: u64,
    pub lead_policy: u64,
    pub delivery: DeliveryMetrics,
    pub replay_ticks: usize,
    pub predicted_ticks_this_frame: usize,
    pub interpolation: super::RemotePoseMetrics,
}

#[derive(Clone, Copy)]
struct CommandIssuance {
    sequence: CommandSeq,
    target: TargetTick,
    policy: Option<u64>,
    feedback_seen: bool,
}

/// One entity's newest state, and when the server captured it.
struct Entry {
    tick: ServerTick,
    replica: Replica,
}

pub(crate) struct Session {
    pub(super) welcome: Welcome,
    pub(super) predictor: Predictor,
    stream: ClientStream<EntityId, ReplicaState>,
    replicas: BTreeMap<EntityId, Entry>,
    /// How many times each entity has entered this connection's view.
    incarnations: BTreeMap<EntityId, u64>,
    delivery: DeliveryMetrics,
    pub(super) poses: super::presentation::RemotePoses,
    pub(crate) events: super::predicted_events::PredictedEvents,
    pub(super) journal: InputJournal<TickInput, DreamAction>,
    pub(super) input_deadline: Option<Duration>,
    pub(super) clock: ClockEstimator,
    pub(super) simulation_clock: CommittedClock,
    pub(super) lead: LeadController,
    pub(super) status: OwnerStatus,
    status_seq: u32,
    pub(super) outcomes_ack_pending: Option<u64>,
    /// The newest owner state received and not yet applied to prediction.
    pending_owner: Option<(u32, ServerTick, OwnerCheckpoint)>,
    /// State packets that carried owner state, newest last, for combat stamps.
    combat_references: VecDeque<(u32, ServerTick)>,
    pub(super) estimated_server_tick: Option<ServerTick>,
    pub(super) estimated_simulation_tick: Option<ServerTick>,
    pub(super) arrival_slack: Option<u8>,
    arrival_feedback: ArrivalFeedback,
    arrival_feedback_applied: u64,
    arrival_feedback_ignored: u64,
    lead_policy: u64,
    lead_policy_head: Option<TargetTick>,
    issued_commands: VecDeque<CommandIssuance>,
    retired_issuance_through: CommandSeq,
    pub(super) active: bool,
    pub(super) sequence: CommandSeq,
    pub(super) charge_episode: Option<u64>,
    pub(super) action_backlog: VecDeque<DreamAction>,
    pub(super) autoplay_edges: [bool; 5],
    pub(super) control_sequence: u32,
    pub(super) input_sequence: u32,
    /// The last tick sent an input packet; this one may wait for the next.
    pub(super) input_sent_last_tick: bool,
    pub(super) last_probe: Duration,
}
impl Session {
    fn scope_for(&self, entity: EntityId) -> ScopeIdentity {
        ScopeIdentity {
            connection: self.welcome.stream.epoch,
            entity,
            scope: ScopeEpoch(self.incarnations.get(&entity).copied().unwrap_or(1)),
            representation: RepresentationRevision(1),
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn public_hero_scopes(&self) -> Vec<(u64, ScopeIdentity, ServerTick, [f32; 2])> {
        self.replicas
            .iter()
            .filter_map(|(entity, entry)| match &entry.replica {
                Replica::Actor(PublicReplica::Hero(hero)) => {
                    Some((hero.id, self.scope_for(*entity), entry.tick, hero.position))
                }
                _ => None,
            })
            .collect()
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn public_enemy_scopes(&self) -> Vec<(u64, ScopeIdentity, ServerTick, [f32; 2])> {
        self.replicas
            .iter()
            .filter_map(|(entity, entry)| match &entry.replica {
                Replica::Actor(PublicReplica::Enemy(enemy)) => Some((
                    enemy.id,
                    self.scope_for(*entity),
                    entry.tick,
                    enemy.position,
                )),
                _ => None,
            })
            .collect()
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn scope_identities(&self) -> Vec<ScopeIdentity> {
        self.replicas
            .iter()
            .filter(|(_, entry)| matches!(entry.replica, Replica::Actor(_)))
            .map(|(entity, _)| self.scope_for(*entity))
            .collect()
    }
    pub(crate) fn metrics(&self) -> SessionMetrics {
        let work = self.predictor.work();
        SessionMetrics {
            server_instance: self.welcome.server_instance,
            transport_connection: self.welcome.client.0,
            active: self.active,
            connection_epoch: self.welcome.stream.epoch,
            match_epoch: self.welcome.match_epoch,
            estimated_server_tick: self.estimated_server_tick,
            estimated_simulation_tick: self.estimated_simulation_tick,
            committed_server_tick: self.simulation_clock.committed(),
            clock_anchor_tick: self.simulation_clock.anchor().0,
            owner_tick: self
                .replicas
                .get(&self.welcome.owner_entity)
                .map(|e| e.tick),
            finalized_tick: self.status.finalized,
            predicted_tick: self.predictor.predicted_tick(),
            gameplay_tick: self
                .predictor
                .state()
                .map(|state| state.stamp().gameplay_tick),
            combat_view_tick: self.poses.combat_view_tick(),
            entities: self.replicas.len(),
            stream_latest: self.stream.ack().latest,
            lead_ticks: self.lead.lead(),
            arrival_slack: self.arrival_slack,
            arrival_feedback: self.arrival_feedback,
            arrival_feedback_applied: self.arrival_feedback_applied,
            arrival_feedback_ignored: self.arrival_feedback_ignored,
            lead_policy: self.lead_policy,
            delivery: self.delivery,
            replay_ticks: work.replayed_ticks,
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
            predictor: Predictor::new(OwnerModel::new(welcome.clone())?, HISTORY_TICKS),
            stream: ClientStream::new(MAX_ENTITIES),
            replicas: BTreeMap::new(),
            incarnations: BTreeMap::new(),
            delivery: DeliveryMetrics::default(),
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
                // Speculation is bounded by retained history. A tighter ceiling
                // freezes the owner after every hitch until fresh authority
                // arrives, then releases the backlog in one frame.
                HISTORY_TICKS as u32,
            )
            .map_err(err)?,
            lead: LeadController::new(3).map_err(err)?,
            status: OwnerStatus {
                finalized: welcome.tick,
                ..Default::default()
            },
            status_seq: 0,
            outcomes_ack_pending: None,
            pending_owner: None,
            combat_references: VecDeque::new(),
            estimated_server_tick: None,
            estimated_simulation_tick: None,
            arrival_slack: None,
            arrival_feedback: ArrivalFeedback::default(),
            arrival_feedback_applied: 0,
            arrival_feedback_ignored: 0,
            lead_policy: 0,
            lead_policy_head: None,
            issued_commands: VecDeque::new(),
            retired_issuance_through: CommandSeq(0),
            active: false,
            sequence: CommandSeq(0),
            charge_episode: None,
            action_backlog: VecDeque::new(),
            autoplay_edges: [false; 5],
            control_sequence: 0,
            input_sequence: 0,
            input_sent_last_tick: false,
            last_probe: Duration::ZERO,
            poses: super::presentation::RemotePoses::new(epoch, welcome.content.scene_revision)
                .map_err(|e| format!("remote presentation: {e:?}"))?,
            welcome,
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
                // A local stall delays this reply after the server sent it, so
                // its round trip is the stall, not the network. Wait for the
                // next probe rather than treating it as a fault.
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
    /// Choose the fresh command head. Forecast ticks before that head use
    /// checkpointed missing-input state; authored ticks consume hardware
    /// samples at their own capture deadlines. This never edits an already
    /// assigned command or its target.
    pub(super) fn assign_command_target(
        &mut self,
        estimated: ServerTick,
    ) -> Result<Option<TargetTick>, String> {
        self.lead.assign_target(estimated).map_err(err)
    }
    pub(super) fn forecast_gap(&self, estimated: ServerTick) -> bool {
        self.sequence == CommandSeq(0)
            || self
                .predictor
                .predicted_tick()
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
    /// Arrival feedback rides every state packet. Only a sample the server
    /// has not reported before, about a command issued under the current lead
    /// policy, tunes the lead.
    pub(super) fn observe_arrival_feedback(&mut self, feedback: ArrivalFeedback) {
        if feedback.observed_commands < self.arrival_feedback.observed_commands
            || feedback.late_commands < self.arrival_feedback.late_commands
        {
            return;
        }
        let Some(sample) = feedback.sample else {
            self.arrival_feedback = feedback;
            return;
        };
        if feedback.observed_commands == self.arrival_feedback.observed_commands
            || sample.sequence > self.sequence
        {
            return;
        }
        self.arrival_feedback = feedback;
        self.arrival_slack = Some(sample.slack);
        let issued = self
            .issued_commands
            .iter_mut()
            .find(|issued| issued.sequence == sample.sequence && issued.target == sample.target);
        let eligible = match issued {
            Some(issued) if !issued.feedback_seen => {
                issued.feedback_seen = true;
                issued.policy == Some(self.lead_policy)
            }
            _ => false,
        };
        if !eligible {
            self.arrival_feedback_ignored = self.arrival_feedback_ignored.saturating_add(1);
            return;
        }
        self.arrival_feedback_applied = self.arrival_feedback_applied.saturating_add(1);
        let previous = self.lead.lead();
        if self
            .lead
            .observe_arrival_slack(i32::from(sample.slack))
            .is_ok()
            && self.lead.lead() != previous
        {
            self.lead_policy = self.lead_policy.saturating_add(1);
            self.lead_policy_head = None;
        }
    }
    pub(super) fn capture_combat_view(&mut self, at: Duration) -> Option<CombatViewStamp> {
        if !self.active {
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
        if let Some(Entry {
            replica: Replica::Global(global),
            ..
        }) = self.replicas.get(&self.welcome.global_entity)
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
    /// What a tick at `at` reads: the scene, the platforms the client knows,
    /// and the nearest disclosed enemies sampled no later than `at`.
    pub(super) fn prediction_dependencies(&self, at: ServerTick) -> Dependencies {
        let owner = self
            .predictor
            .state()
            .map(|state| state.hero_view().position)
            .unwrap_or_default();
        let mut platforms = Vec::new();
        let mut targets = Vec::new();
        for entry in self.replicas.values() {
            match &entry.replica {
                Replica::Actor(PublicReplica::Platform(view)) => platforms.push(view.clone()),
                Replica::Actor(PublicReplica::Enemy(enemy))
                    if entry.tick <= at && enemy.hp > 0.0 =>
                {
                    targets.push((
                        engine_core::spatial::distance(enemy.position, owner),
                        *enemy,
                    ));
                }
                _ => {}
            }
        }
        targets.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.id.cmp(&b.1.id)));
        Dependencies {
            collision: self.predictor.model().collision.clone(),
            platforms,
            targets: targets
                .into_iter()
                .take(MAX_PRESENTATION_TARGETS)
                .map(|(_, enemy)| enemy)
                .collect(),
        }
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
    pub(super) fn begin_frame(&mut self) {
        self.predictor.begin_frame();
    }
    pub(super) fn update_connection(
        &self,
        connection: &mut crate::DreamConnection,
        rtt_ms: f64,
        party_size: usize,
    ) {
        connection.connected = self.active;
        connection.client_id = self.welcome.client.0;
        connection.player_id = self.welcome.player.get();
        connection.party_size = party_size;
        connection.rtt_ms = rtt_ms;
        connection.status = if connection.connected {
            "Connected to the shared dream"
        } else {
            "Joining the shared dream…"
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
            self.welcome.stream.epoch,
            self.control_sequence,
            self.welcome.frame_limit().map_err(err)?,
        )
        .map_err(err)
    }
    /// One input packet: the stream acknowledgement and the given records.
    /// One input packet: the stream acknowledgement and as many of the given
    /// records, newest first, as fit one application frame.
    pub(super) fn input(&mut self, mut records: Vec<InputRecord>) -> Result<Vec<u8>, String> {
        self.input_sequence = self.input_sequence.wrapping_add(1);
        let frame = usize::from(self.welcome.application_frame_bytes);
        loop {
            let encoded = encode_input(
                &InputPacket {
                    ack: self.stream.ack(),
                    records: BoundedVec::new(records.clone()).map_err(err)?,
                },
                self.welcome.stream.epoch,
                self.input_sequence,
                input_limit(),
            );
            match encoded {
                Ok(bytes) if bytes.len() <= frame => return Ok(bytes),
                Ok(_) | Err(engine_net::codec::CodecError::Oversized) if records.len() > 1 => {
                    records.pop();
                }
                Ok(_) => return Err("input record exceeds the application frame".into()),
                Err(error) => return Err(err(error)),
            }
        }
    }
    #[cfg(test)]
    pub(super) fn stream_ack(&self) -> engine_net::replication::stream::StreamAck {
        self.stream.ack()
    }
    #[cfg(test)]
    pub(super) fn replica(&self, entity: EntityId) -> Option<&Replica> {
        self.replicas.get(&entity).map(|entry| &entry.replica)
    }
    /// Whether any state packet has arrived, so there is something to acknowledge.
    pub(super) fn has_state(&self) -> bool {
        self.stream.ack().latest != 0
    }
    /// Decode and keep one state packet. A packet that cannot be decoded is
    /// not acknowledged, and the server covers it in a later packet.
    pub(super) fn receive_state(&mut self, bytes: &[u8]) -> Result<bool, super::ReceiveRejection> {
        let limit = FrameLimit::new(MAX_STATE_FRAME_BYTES).map_err(err)?;
        let packet = decode_state(bytes, self.welcome.stream.epoch, limit).map_err(err)?;
        if !self.stream.accepts(packet.seq) {
            return Ok(false);
        }
        let result = self.decode_entries(&packet);
        let (decoded, restored) = match result {
            Ok(entries) => entries,
            Err(error) => {
                self.delivery.rejected_packets = self.delivery.rejected_packets.saturating_add(1);
                return Err(error);
            }
        };
        let changes = self
            .stream
            .accept(packet.seq, decoded, &packet.removed)
            .map_err(err)?;
        self.delivery.state_packets = self.delivery.state_packets.saturating_add(1);
        for (entity, replica) in restored {
            if !changes.updated.contains(&entity) {
                continue;
            }
            if !self.replicas.contains_key(&entity) {
                *self.incarnations.entry(entity).or_insert(0) += 1;
            }
            match &replica {
                Replica::Actor(actor) => {
                    if let Err(error) =
                        self.poses
                            .receive(self.scope_for(entity), packet.tick, actor)
                    {
                        log::debug!("remote pose {entity:?}: {error:?}");
                    }
                }
                Replica::Owner(owner) => {
                    self.combat_references.push_back((packet.seq, packet.tick));
                    while self.combat_references.len() > 64 {
                        self.combat_references.pop_front();
                    }
                    self.simulation_clock.commit(packet.tick);
                    if self
                        .pending_owner
                        .as_ref()
                        .is_none_or(|(seq, _, _)| *seq < packet.seq)
                    {
                        self.pending_owner = Some((packet.seq, packet.tick, owner.clone()));
                    }
                }
                Replica::Global(_) => {}
            }
            self.replicas.insert(
                entity,
                Entry {
                    tick: packet.tick,
                    replica,
                },
            );
        }
        for entity in changes.removed {
            self.replicas.remove(&entity);
            self.poses.remove(entity);
        }
        if packet.seq > self.status_seq {
            self.status_seq = packet.seq;
            self.status = packet.status;
            self.simulation_clock.commit(packet.status.finalized);
            self.observe_arrival_feedback(packet.status.arrival);
        }
        Ok(true)
    }
    #[allow(clippy::type_complexity)]
    fn decode_entries(
        &self,
        packet: &StatePacket,
    ) -> Result<(Vec<(EntityId, ReplicaState)>, Vec<(EntityId, Replica)>), super::ReceiveRejection>
    {
        let expected = self.predictor.model().expectation();
        let mut decoded = Vec::with_capacity(packet.entries.len());
        let mut restored = Vec::with_capacity(packet.entries.len());
        for entry in &packet.entries {
            let base = match self
                .stream
                .base(entry.entity, entry.base, packet.seq)
                .map_err(err)?
            {
                Base::Superseded => continue,
                Base::Whole => None,
                Base::Delta(base) => Some(base),
            };
            let state = ReplicaState::decode(&entry.body, base).map_err(err)?;
            let expected_kind = if entry.entity == self.welcome.owner_entity {
                ReplicaKind::Owner
            } else if entry.entity == self.welcome.global_entity {
                ReplicaKind::Global
            } else {
                ReplicaKind::Actor
            };
            if state.kind() != expected_kind {
                return Err("state kind does not match its entity".into());
            }
            let replica = state
                .restore((expected_kind == ReplicaKind::Owner).then_some(expected))
                .map_err(err)?;
            match &replica {
                Replica::Owner(owner)
                    if owner.stamp().server_tick != packet.tick.0
                        || owner.collision_identity() != self.welcome.content.scene =>
                {
                    return Err("owner state does not match its packet".into());
                }
                Replica::Global(global)
                    if global.stamp.server_tick != packet.tick.0
                        || global.stamp.match_epoch != self.welcome.match_epoch =>
                {
                    return Err("match summary does not match its packet".into());
                }
                _ => {}
            }
            decoded.push((entry.entity, state));
            restored.push((entry.entity, replica));
        }
        Ok((decoded, restored))
    }
    /// Apply the newest owner state: restore it at its tick and replay local
    /// input after it. A state outside the retained history becomes the new
    /// base. Nothing here ends or resets the session.
    pub(super) fn reconcile(&mut self) -> Result<(), String> {
        let Some((_, tick, checkpoint)) = self.pending_owner.take() else {
            return Ok(());
        };
        let state = OwnerPredictionState::from_checkpoint(
            &checkpoint,
            self.predictor.model().expectation(),
        )
        .map_err(err)?;
        let dependencies = self.prediction_dependencies(tick);
        let started = bevy::platform::time::Instant::now();
        let correction = self
            .predictor
            .correct(tick, state.clone(), dependencies.clone());
        let micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        self.delivery.replay_cpu_micros = micros;
        self.delivery.maximum_replay_cpu_micros =
            self.delivery.maximum_replay_cpu_micros.max(micros);
        match correction {
            Ok(Correction::Replayed(_)) => self.delivery.replayed += 1,
            Ok(Correction::Rebased) => self.delivery.rebased += 1,
            Ok(Correction::Stale) => self.delivery.stale += 1,
            Err(error) => {
                // Local input after the corrected tick could not be replayed;
                // continue from the server's state and forecast forward.
                log::warn!("Owner replay failed; continuing from authority: {error:?}");
                self.predictor.reset(tick, state, dependencies);
                self.delivery.rebased += 1;
            }
        }
        self.active = checkpoint.is_active();
        self.events.publish(&self.predictor)?;
        // Binding is source-owned: require the owner state's exact game
        // projectile ID and a public projectile entity carrying that ID.
        let bindings = self
            .predictor
            .state()
            .into_iter()
            .flat_map(|state| state.starfall_flights())
            .filter_map(|flight| {
                let id = flight.authority_id?;
                self.replicas
                    .iter()
                    .find_map(|(entity, entry)| match &entry.replica {
                        Replica::Actor(PublicReplica::Projectile(p))
                            if p.id == id && p.owner == Some(self.welcome.player.get()) =>
                        {
                            Some((flight.key, *entity))
                        }
                        _ => None,
                    })
            })
            .collect::<Vec<_>>();
        for (key, entity) in bindings {
            self.events.bind(key, entity)?;
        }
        Ok(())
    }
    /// The newest owner state received, whether or not prediction has used it.
    pub(crate) fn authoritative_owner_pose(
        &self,
    ) -> Result<Option<(ServerTick, [f32; 2])>, String> {
        let Some(entry) = self.replicas.get(&self.welcome.owner_entity) else {
            return Ok(None);
        };
        let Replica::Owner(owner) = &entry.replica else {
            return Err("owner entity type".into());
        };
        Ok(Some((entry.tick, owner.hero_view().position)))
    }
    pub(crate) fn presentation(
        &self,
        predicted: bool,
    ) -> Result<Option<DreamPresentation>, String> {
        let Some(Entry {
            replica: Replica::Global(global),
            ..
        }) = self.replicas.get(&self.welcome.global_entity)
        else {
            return Ok(None);
        };
        let owner = if predicted {
            self.predictor.state().cloned()
        } else {
            match self.replicas.get(&self.welcome.owner_entity) {
                Some(Entry {
                    replica: Replica::Owner(owner),
                    ..
                }) => OwnerPredictionState::from_checkpoint(
                    owner,
                    self.predictor.model().expectation(),
                )
                .ok(),
                _ => None,
            }
        };
        let Some(owner) = owner else {
            return Ok(None);
        };
        let mut actors = Vec::new();
        let mut cover_markers = Vec::new();
        for (entity, entry) in &self.replicas {
            let Replica::Actor(actor) = &entry.replica else {
                continue;
            };
            let mut actor = actor.clone();
            if let PublicReplica::CoverMarker(marker) = &actor
                && let Some(Entry {
                    replica: Replica::Actor(PublicReplica::Cover(cover)),
                    ..
                }) = self.replicas.get(&marker.parent)
                && cover.present
            {
                let mut parent = PublicReplica::Cover(*cover);
                if predicted {
                    self.poses.apply(self.scope_for(marker.parent), &mut parent);
                }
                if let PublicReplica::Cover(cover) = parent {
                    cover_markers.push(dreamwake_sim::replication::CoverMarkerPresentation {
                        id: marker.id,
                        position: std::array::from_fn(|axis| {
                            cover.position[axis] + marker.local_offset[axis]
                        }),
                        open: cover.open,
                    });
                }
            }
            if predicted
                && matches!(actor, PublicReplica::Projectile(_))
                && self.events.suppress_public(*entity)
            {
                continue;
            }
            if predicted && !self.poses.apply(self.scope_for(*entity), &mut actor) {
                continue;
            }
            actors.push(actor);
        }
        // Public entities may arrive before an independently disclosed source.
        // Keep them, but wait to render a reference to a source not yet shown.
        let mut sources = BTreeSet::from([owner.owner()]);
        for actor in &actors {
            match actor {
                PublicReplica::Hero(v) => {
                    sources.insert(v.id);
                }
                PublicReplica::Enemy(v) => {
                    sources.insert(v.id);
                }
                _ => {}
            }
        }
        actors.retain(|actor| match actor {
            PublicReplica::Projectile(v) => v.owner.is_none_or(|id| sources.contains(&id)),
            PublicReplica::Wisp(v) => v.owner.is_none_or(|id| sources.contains(&id)),
            PublicReplica::Effect(v) => v.id.owner == 0 || sources.contains(&v.id.owner),
            _ => true,
        });
        let mut presentation =
            DreamPresentation::from_replicas(global, &owner, &actors).map_err(err)?;
        presentation.cover_markers = cover_markers;
        if predicted {
            presentation.elapsed = self.poses.presentation_elapsed(global);
            // Retain already-proven retirement fences from the existing action
            // binding lane while an older owner state is still in flight.
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
                        // A local rider and its base share the predicted display
                        // time. Remote presentation remains at the common time.
                        platform.pose = pose;
                        platform.gameplay_tick = owner.gameplay_tick();
                        platform.phase = ((platform.gameplay_tick - platform.origin_gameplay_tick)
                            % u32::from(dreamwake_sim::platform::PLATFORM_PERIOD_TICKS))
                            as u16;
                    }
                }
            }
        }
        presentation
            .attach_collision(&self.predictor.model().collision, &owner)
            .map_err(err)?;
        Ok(Some(presentation))
    }
    /// Drop unsent intent after a long pause in local input, keeping the
    /// session: the next owner state and forecast continue from authority.
    pub(super) fn reset_input(&mut self) {
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

/// The newest owner state the client held at a displayed tick.
fn reference_for_view(
    references: &VecDeque<(u32, ServerTick)>,
    viewed_at: ServerTick,
) -> Option<u32> {
    references
        .iter()
        .rev()
        .find(|(_, tick)| *tick <= viewed_at)
        .map(|(seq, _)| *seq)
}
#[cfg(test)]
mod combat_reference_tests {
    use super::*;
    #[test]
    fn new_owner_state_cannot_move_reference_past_frozen_render_tick() {
        let mut references = VecDeque::from([(4, ServerTick(90))]);
        let frozen_render_tick = ServerTick(92);
        assert_eq!(reference_for_view(&references, frozen_render_tick), Some(4));
        for tick in 93..110 {
            references.push_back((tick as u32, ServerTick(tick)));
            assert_eq!(reference_for_view(&references, frozen_render_tick), Some(4));
        }
        assert_eq!(reference_for_view(&references, ServerTick(89)), None);
    }
}
