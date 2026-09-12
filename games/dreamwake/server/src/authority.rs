//! Authenticated Dreamwake admission, fixed-tick commands and scoped publication.
#[cfg(test)]
mod arrival_tests;
mod baselines;
mod combat;
#[cfg(test)]
mod finalization_tests;
#[cfg(test)]
mod health_tests;
#[cfg(test)]
mod outcome_tests;
mod outcomes;
mod ownership;
mod peer;
mod publish;
#[cfg(test)]
mod tests;

use crate::replication::DreamReplication;
use dreamwake_protocol::{self as wire, DreamAction, live, player::PlayerId};
#[cfg(test)]
use dreamwake_sim::DreamInput;
use dreamwake_sim::{DreamSimulation, RunPhase};
use engine_net::{
    codec::BoundedVec,
    commands::{Admission, OwnerStream},
    types::*,
};
use peer::{Peer, Rules};
use renet::RenetServer;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    time::Duration,
};
const MAX_PENDING_ACTIONS: usize = 32;
const MAX_MESSAGES_PER_TICK: usize = 64;
const APPLICATION_FRAME_BYTES: u16 = 1100;
#[cfg(test)]
const OWNER_GROUP_MEMBERS: usize = live::baselines::MIN_MEMBERS;
const SCENE: SceneRevision = SceneRevision(1);
fn failure(error: impl std::fmt::Debug) -> String {
    format!("{error:?}")
}
fn nanos(time: Duration) -> u64 {
    u64::try_from(time.as_nanos()).unwrap_or(u64::MAX)
}

pub(crate) struct DreamAuthority {
    sim: DreamSimulation,
    server_instance: u64,
    pub(crate) action_trace: crate::ActionTrace,
    peers: BTreeMap<u64, Peer>,
    replication: DreamReplication,
    epoch: u32,
    next_connection: ConnectionEpoch,
    tick: ServerTick,
    now: Duration,
    revision: u64,
    max_clients: usize,
    seed: u64,
    lucid: bool,
    replication_distance: f32,
    compression: engine_net::CompressionPolicy,
    notices: VecDeque<(u64, &'static str)>,
    failed: BTreeSet<u64>,
    pending_admissions: BTreeSet<u64>,
    ownership: BTreeMap<u64, OwnershipEpoch>,
    handoffs: BTreeMap<u64, ownership::PendingHandoff>,
    transport_diagnostics: Option<engine_server::SharedNet>,
    combat_clock: combat::GameplayClock,
    outcomes: outcomes::Outcomes,
    overloaded: bool,
}
/// Control is emitted before fresh unreliable state when the transport has a
/// bounded egress allowance. Channel identities and receive configuration remain
/// identical to the shared protocol configuration.
pub(crate) fn connection_config() -> renet::ConnectionConfig {
    let mut config = wire::connection_config();
    config
        .server_channels_config
        .sort_by_key(|channel| channel.channel_id != wire::CONTROL_CHANNEL);
    config
}

impl DreamAuthority {
    pub(crate) fn observe_transport(&mut self, shared: engine_server::SharedNet) {
        self.transport_diagnostics = Some(shared);
    }

    #[cfg(test)]
    pub(crate) fn new(seed: u64, lucid: bool, max_clients: usize) -> Self {
        Self::with_replication_distance(seed, lucid, max_clients, 80.0)
    }
    #[cfg(test)]
    pub(crate) fn with_replication_distance(
        seed: u64,
        lucid: bool,
        max_clients: usize,
        replication_distance: f32,
    ) -> Self {
        Self::with_replication_policy(
            seed,
            lucid,
            max_clients,
            replication_distance,
            engine_net::CompressionPolicy::default(),
        )
    }
    pub(crate) fn with_replication_policy(
        seed: u64,
        lucid: bool,
        max_clients: usize,
        replication_distance: f32,
        compression: engine_net::CompressionPolicy,
    ) -> Self {
        assert!(replication_distance.is_finite() && (4.0..=80.0).contains(&replication_distance));
        let mut sim = DreamSimulation::new(seed, lucid);
        sim.remove_player(1);
        Self {
            sim,
            action_trace: Default::default(),
            server_instance: loop {
                let instance = u64::from_le_bytes(renet_cross::generate_random_bytes());
                if instance != 0 {
                    break instance;
                }
            },
            peers: BTreeMap::new(),
            replication: Self::graph(1, replication_distance, compression),
            epoch: 1,
            next_connection: ConnectionEpoch(1),
            tick: ServerTick(0),
            now: Duration::ZERO,
            revision: 0,
            max_clients: max_clients.min(dreamwake_sim::MAX_HEROES),
            seed,
            lucid,
            replication_distance,
            compression,
            notices: VecDeque::new(),
            failed: BTreeSet::new(),
            pending_admissions: BTreeSet::new(),
            ownership: BTreeMap::new(),
            handoffs: BTreeMap::new(),
            transport_diagnostics: None,
            combat_clock: Default::default(),
            outcomes: Default::default(),
            overloaded: false,
        }
    }
    fn graph(
        epoch: u32,
        distance: f32,
        compression: engine_net::CompressionPolicy,
    ) -> DreamReplication {
        DreamReplication::with_compression(
            epoch,
            f64::from(distance),
            engine_net::interest::Limits {
                max_connections: wire::MAX_PLAYERS,
                max_entities: 8192,
                max_observers: 1,
                prefetch: 0.0,
                ..Default::default()
            },
            compression,
        )
        .expect("fixed Dreamwake graph limits")
    }
    fn make_peer(&mut self, id: u64, player: PlayerId) -> Result<Peer, String> {
        let epoch = self.next_connection;
        self.next_connection = epoch.checked_next().ok_or("Connection epoch exhausted")?;
        let ownership = match self.ownership.get(&player.get()) {
            Some(epoch) => epoch.checked_next().ok_or("Ownership epoch exhausted")?,
            None => OwnershipEpoch(1),
        };
        let (global_entity, owner_entity, collision_entity) = self
            .replication
            .reserve_owner(player.get())
            .map_err(failure)?;
        let peer = Peer::new(
            live::Welcome {
                server_instance: self.server_instance,
                protocol: wire::PROTOCOL_ID,
                client: ConnectionId(id),
                player,
                stream: OwnerStream {
                    connection: ConnectionId(id),
                    epoch,
                    stream: CommandStream(1),
                    owner: owner_entity,
                    ownership,
                },
                match_epoch: self.epoch,
                tick: self.tick,
                server_time_nanos: nanos(self.now),
                content: live::ContentIdentity::current(SCENE),
                global_entity,
                owner_entity,
                collision_entity,
                application_frame_bytes: APPLICATION_FRAME_BYTES,
            },
            self.now,
        )?;
        self.ownership.insert(player.get(), ownership);
        Ok(peer)
    }
    #[cfg(test)]
    fn admit(&mut self, id: u64) -> bool {
        let Ok(player) = PlayerId::new(id) else {
            return false;
        };
        self.admit_player(id, player)
    }
    fn admit_player(&mut self, id: u64, player: PlayerId) -> bool {
        if self.peers.contains_key(&id) {
            return self.peers[&id].welcome.player == player;
        }
        if id == 0
            || self.peers.len() >= self.max_clients
            || self
                .peers
                .values()
                .any(|peer| peer.welcome.player == player)
            || (!self.ownership.contains_key(&player.get())
                && self.ownership.len() >= dreamwake_sim::MAX_HEROES)
        {
            return false;
        }
        match self.make_peer(id, player) {
            Ok(peer) => {
                self.peers.insert(id, peer);
                self.pending_admissions.insert(id);
                true
            }
            Err(reason) => {
                log::warn!("Dreamwake admission: {reason}");
                false
            }
        }
    }
    fn disconnect(&mut self, id: u64) {
        self.cancel_handoffs_for(id);
        if let Some(peer) = self.peers.remove(&id) {
            self.outcomes.close(
                peer.welcome.player.get(),
                peer.welcome.stream.epoch,
                self.tick,
            );
            self.sim.set_player_active(peer.welcome.player.get(), false);
        }
        self.pending_admissions.remove(&id);
        self.notices.retain(|(recipient, _)| *recipient != id);
        let _ = self.replication.disconnect(id, self.tick);
        self.failed.remove(&id);
        if self.peers.is_empty() {
            self.sim.set_paused(false);
        }
    }
    fn resync(&mut self, id: u64) -> Result<(), String> {
        self.cancel_handoffs_for(id);
        let peer = self.peers.get(&id).ok_or("Unknown peer")?;
        let mut attempts = peer.resync_attempts.clone();
        attempts.retain(|attempt| self.now.saturating_sub(*attempt) < Duration::from_secs(30));
        if attempts.len() >= 3 {
            return Err("Repeated resync limit".into());
        }
        attempts.push_back(self.now);
        let count = peer.resyncs.saturating_add(1);
        if let Some(peer) = self.peers.get(&id) {
            self.outcomes.close(
                peer.welcome.player.get(),
                peer.welcome.stream.epoch,
                self.tick,
            );
        }
        self.replication
            .disconnect(id, self.tick)
            .map_err(failure)?;
        let player = self.peers[&id].welcome.player;
        let initial_deadline =
            (!self.peers[&id].activation_committed).then_some(self.peers[&id].initial_deadline);
        let mut peer = self.make_peer(id, player)?;
        if let Some(deadline) = initial_deadline {
            peer.initial_deadline = deadline;
        }
        peer.interrupt_charge_pending = true;
        peer.resyncs = count;
        peer.resync_attempts = attempts;
        peer.activation_committed = self.sim.player_is_active(player.get());
        self.peers.insert(id, peer);
        Ok(())
    }
    fn receive_control(
        &mut self,
        id: u64,
        control: live::ClientControl,
        server: &mut RenetServer,
    ) -> Result<(), String> {
        match control {
            live::ClientControl::FinalizedAck { through } => {
                self.peers
                    .get_mut(&id)
                    .ok_or("Unknown peer")?
                    .finalized_batches
                    .acknowledge(through)
                    .map_err(failure)?;
            }
            live::ClientControl::OutcomesAck { through } => {
                self.peers
                    .get_mut(&id)
                    .ok_or("Unknown peer")?
                    .outcome_batches
                    .acknowledge(through)
                    .map_err(failure)?;
            }
            live::ClientControl::OutcomeLookup { keys } => {
                let peer = self.peers.get_mut(&id).ok_or("Unknown peer")?;
                if self.now < peer.next_outcome_lookup {
                    return Ok(());
                }
                peer.next_outcome_lookup = self.now + Duration::from_millis(250);
                for key in keys.into_vec() {
                    if peer.outcome_lookups.len() >= 8 {
                        break;
                    }
                    peer.outcome_lookups.insert(key);
                }
            }
            live::ClientControl::Ready { content } => {
                let peer = self.peers.get_mut(&id).ok_or("Unknown peer")?;
                if content != peer.welcome.content {
                    return Err("Incompatible ruleset or scene".into());
                }
                peer.ready = true;
                self.replication
                    .set_player_peer(
                        id,
                        peer.welcome.player.get(),
                        peer.welcome.stream.ownership,
                        peer.welcome.stream.epoch,
                        content.scene_revision,
                    )
                    .map_err(failure)?;
            }
            live::ClientControl::Decoded { receipts } => {
                let peer = self.peers.get(&id).ok_or("Unknown peer")?;
                if let Some(scopes) = self.replication.scopes_mut(id) {
                    for receipt in receipts.as_slice() {
                        if !peer.owner_group_entities().contains(&receipt.scope.entity) {
                            let _ = scopes.acknowledge(*receipt);
                        }
                    }
                }
            }
            live::ClientControl::OwnerDecoded { proofs } => {
                let peer = self.peers.get_mut(&id).ok_or("Unknown peer")?;
                if !peer.ready {
                    return Ok(());
                }
                let Some(scopes) = self.replication.scopes_mut(id) else {
                    return Ok(());
                };
                if !peer.baselines.validate_proofs(proofs.as_slice())?
                    || proofs
                        .as_slice()
                        .iter()
                        .any(|proof| scopes.validate_acknowledgment(proof.state).is_err())
                {
                    return Ok(());
                }
                peer.baselines.acknowledge(proofs.as_slice())?;
                for proof in proofs.as_slice() {
                    let receipt = &proof.state;
                    if scopes.acknowledge(*receipt).is_ok() {
                        if receipt.scope.entity == peer.welcome.owner_entity {
                            peer.combat.decoded(*receipt);
                        }
                        if let Some(group) = &mut peer.frozen {
                            if group
                                .members
                                .iter()
                                .any(|state| state.receipt() == *receipt)
                            {
                                group.decoded.insert(receipt.scope.entity);
                            }
                        }
                    }
                }
                if peer
                    .frozen
                    .as_ref()
                    .is_some_and(|group| group.decoded.len() == group.members.len())
                {
                    let group = peer.frozen.take().expect("complete frozen group");
                    if !peer.active {
                        if !group.active_state {
                            peer.activate_pending = true;
                            return Ok(());
                        }
                        peer.active = true;
                        let owner_snapshot = group
                            .members
                            .iter()
                            .find(|state| state.scope.entity == peer.welcome.owner_entity)
                            .ok_or("Missing owner proof")?
                            .snapshot;
                        publish::control(
                            server,
                            id,
                            peer,
                            &live::ServerControl::Active {
                                owner_snapshot,
                                through: self.tick,
                            },
                        )?;
                    }
                }
            }
            live::ClientControl::BaselineRetire { requests } => {
                let peer = self.peers.get_mut(&id).ok_or("Unknown peer")?;
                if let Some(fences) = peer.baselines.retire(requests.as_slice())? {
                    // A duplicate fence, or a fence unrelated to the retained
                    // transfer's bases, must not restart that transfer's deadline.
                    if peer.frozen.as_ref().is_some_and(|group| {
                        group.baseline_packets.iter().any(|packet| {
                            if let engine_net::replication::baselines::BaselineEncoding::Delta {
                                base,
                            } = packet.encoding
                            {
                                fences.iter().any(|fence| {
                                    fence.context == base.context
                                        && fence.generation == base.generation
                                        && base.snapshot <= fence.through
                                })
                            } else {
                                false
                            }
                        })
                    }) {
                        peer.frozen = None;
                    }
                    publish::control(
                        server,
                        id,
                        peer,
                        &live::ServerControl::BaselineRetired {
                            fences: BoundedVec::new(fences).map_err(failure)?,
                        },
                    )?;
                }
            }
            live::ClientControl::BaselineRepair { requests } => {
                let peer = self.peers.get_mut(&id).ok_or("Unknown peer")?;
                let reset_was_pending = peer.baselines.reset_pending.is_some();
                peer.baselines.repair(requests.as_slice())?;
                if !reset_was_pending && peer.baselines.reset_pending.is_some() {
                    peer.frozen = None;
                }
            }
            live::ClientControl::Exited(exit) => {
                if let Some(scopes) = self.replication.scopes_mut(id) {
                    let _ = scopes.acknowledge_exit(exit);
                }
            }
            live::ClientControl::Destroyed(destroy) => {
                if let Some(scopes) = self.replication.scopes_mut(id) {
                    let _ = scopes.acknowledge_destroy(destroy);
                }
            }
            live::ClientControl::Menu { sequence, action } => {
                let peer = self.peers.get_mut(&id).ok_or("Unknown peer")?;
                if peer.active {
                    peer.queue_menu(sequence, action)?;
                }
            }
            live::ClientControl::ClockProbe { client_send_nanos } => {
                let peer = self.peers.get_mut(&id).ok_or("Unknown peer")?;
                publish::control(
                    server,
                    id,
                    peer,
                    &live::ServerControl::ClockReply {
                        client_send_nanos,
                        server_receive_nanos: nanos(self.now),
                        server_send_nanos: nanos(self.now),
                        tick: self.tick,
                    },
                )?;
            }
            live::ClientControl::Resync { .. } => {
                self.resync(id)?;
            }
        }
        Ok(())
    }
    fn receive_bundle(&mut self, id: u64, bundle: live::CommandBundle) -> Result<(), String> {
        let peer = self.peers.get_mut(&id).ok_or("Unknown peer")?;
        if !peer.active {
            return Ok(());
        }
        // Receive and simulation are ordered phases. Never report a prepared
        // future cutoff as though it were committed arrival time.
        if peer.inbox.prepared_tick().is_some() {
            return Err("Input received during prepared simulation tick".into());
        }
        let rules = Rules(peer.welcome.stream);
        for command in bundle.records.as_slice() {
            // GraphicsId keeps the exact existing u32 command identity. A new
            // stream is required before exhaustion, never a truncating cast.
            if command.sequence.0 > u64::from(u32::MAX) {
                return Err("Action identity exhausted; reconnect required".into());
            }
            let keys: Vec<_> = command
                .actions
                .as_slice()
                .iter()
                .map(|edge| command.action_key(edge.slot))
                .collect();
            let owner = peer.welcome.player.get();
            let (overload, reservations) = if keys.is_empty() {
                (false, Vec::new())
            } else {
                match self.outcomes.normal.reserve(owner, &keys) {
                    Ok(keys) => (false, keys),
                    Err(engine_net::outcomes::OutcomeError::Capacity) => (
                        true,
                        self.outcomes
                            .overload
                            .reserve(owner, &keys)
                            .map_err(failure)?,
                    ),
                    Err(error) => return Err(failure(error)),
                }
            };
            let ledger = if overload {
                &mut self.outcomes.overload
            } else {
                &mut self.outcomes.normal
            };
            let previous_admission = peer.inbox.retained_admission(command.sequence);
            let admission = match peer.inbox.admit(ConnectionId(id), command.clone(), &rules) {
                Ok(admission) => admission,
                Err(error) => {
                    ledger.cancel_reservations(&reservations);
                    return Err(failure(error));
                }
            };
            match admission {
                Admission::Accepted { .. } if overload => {
                    peer.overload_commands.insert(command.sequence);
                }
                Admission::Accepted { .. } => {}
                Admission::Late | Admission::TargetOccupied => {
                    for key in reservations {
                        let status = if admission == Admission::Late {
                            live::TerminalStatus::Expired
                        } else {
                            live::TerminalStatus::Rejected
                        };
                        let reason = if admission == Admission::Late {
                            live::OutcomeReason::Expired
                        } else {
                            live::OutcomeReason::Game(dreamwake_sim::combat::ActionReason::Conflict)
                        };
                        ledger
                            .finish(
                                owner,
                                key,
                                ServerTick(command.target.0),
                                outcomes::rejection(status, reason),
                            )
                            .map_err(failure)?;
                    }
                }
                _ => ledger.cancel_reservations(&reservations),
            }
            if matches!(admission, Admission::Accepted { .. }) {
                // Arrival, execution and retained combat history share committed
                // simulation ticks, even while the driver owes wall-clock work.
                let arrival = self.tick;
                peer.combat.admit(command.sequence, arrival)?;
                self.action_trace
                    .command(self.server_instance, owner, command, Some(arrival));
            } else if matches!(admission, Admission::Late | Admission::TargetOccupied) {
                self.action_trace
                    .command(self.server_instance, owner, command, None);
            }
            // An out-of-order first delivery can miss its deadline even after a
            // newer command arrives. Count each effective observation once using
            // the existing bounded audit; redundant Late records cannot inflate
            // feedback, and Future retries count only when they become effective.
            if !matches!(
                previous_admission,
                Some(Admission::Accepted { .. } | Admission::Late)
            ) {
                let slack = match admission {
                    Admission::Accepted { arrival_slack } => Some(arrival_slack.min(12) as u8),
                    Admission::Late => Some(0),
                    _ => None,
                };
                if let Some(slack) = slack {
                    let sample = live::ArrivalSample {
                        sequence: command.sequence,
                        target: command.target,
                        arrival_tick: peer.inbox.finalized_through(),
                        slack,
                    };
                    peer.arrival.observed_commands = peer
                        .arrival
                        .observed_commands
                        .checked_add(1)
                        .ok_or("arrival observation counter exhausted")?;
                    if admission == Admission::Late {
                        peer.arrival.late_commands = peer
                            .arrival
                            .late_commands
                            .checked_add(1)
                            .ok_or("late observation counter exhausted")?;
                    }
                    if peer.arrival.sample.is_none_or(|old| {
                        sample.slack < old.slack
                            || (sample.slack == old.slack && sample.sequence > old.sequence)
                    }) {
                        peer.arrival.sample = Some(sample);
                    }
                }
            }
        }
        Ok(())
    }
    fn notice(&mut self, id: u64, text: &'static str) {
        if self.notices.len() < self.max_clients * 8 {
            self.notices.push_back((id, text));
        }
    }
    fn restart(&mut self, lucid: bool, new_seed: bool) {
        self.cancel_all_handoffs();
        if new_seed {
            self.seed = self
                .seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
        }
        let Some(epoch) = self.epoch.checked_add(1) else {
            self.failed.extend(self.peers.keys());
            return;
        };
        for peer in self.peers.values() {
            self.outcomes.close(
                peer.welcome.player.get(),
                peer.welcome.stream.epoch,
                self.tick,
            );
        }
        self.epoch = epoch;
        self.lucid = lucid;
        let connected: BTreeSet<_> = self
            .peers
            .values()
            .map(|peer| peer.welcome.player.get())
            .collect();
        for player in self
            .ownership
            .keys()
            .copied()
            .filter(|id| !connected.contains(id))
            .collect::<Vec<_>>()
        {
            self.sim.remove_player(player);
        }
        self.ownership.retain(|id, _| connected.contains(id));
        self.sim.restart_party(self.seed, lucid);
        self.replication = Self::graph(epoch, self.replication_distance, self.compression);
        self.revision = 0;
        let ids: Vec<_> = self.peers.keys().copied().collect();
        for id in ids {
            let player = self.peers[&id].welcome.player;
            self.sim.set_player_active(player.get(), false);
            match self.make_peer(id, player) {
                Ok(peer) => {
                    self.peers.insert(id, peer);
                }
                Err(_) => {
                    self.failed.insert(id);
                }
            }
        }
        self.notices.clear();
    }
    fn begin_run(&mut self) {
        if self.sim.phase() == RunPhase::Intro {
            for player in self.peers.values().map(|peer| peer.welcome.player.get()) {
                self.sim.continue_run_for(player);
            }
        }
    }
    fn menu(&mut self, id: u64, action: DreamAction) {
        let Some(player) = self.peers.get(&id).map(|peer| peer.welcome.player.get()) else {
            return;
        };
        if self.overloaded
            && (matches!(action, DreamAction::Start { .. } | DreamAction::Restart)
                || matches!(action, DreamAction::Continue) && self.sim.phase() == RunPhase::Intro)
        {
            self.notice(
                id,
                "The server is catching up. Try starting the dream again shortly.",
            );
            return;
        }
        let mut accepted = true;
        match action {
            DreamAction::Start { lucid } => {
                if matches!(
                    self.sim.phase(),
                    RunPhase::Intro | RunPhase::Victory | RunPhase::Defeat
                ) {
                    if self.lucid != lucid || self.sim.phase() != RunPhase::Intro {
                        self.restart(lucid, false);
                    }
                    self.begin_run();
                } else {
                    self.notice(id, "This dream has already begun.");
                }
            }
            DreamAction::Choose { choice, slot } => {
                accepted = self
                    .sim
                    .choose_reward_for(player, choice as usize, slot as usize)
            }
            DreamAction::Continue => {
                if self.sim.phase() == RunPhase::Intro {
                    self.begin_run();
                } else {
                    accepted = self.sim.continue_run_for(player);
                }
            }
            DreamAction::Swap { a, b } => {
                if self.sim.phase() == RunPhase::Combat
                    && !(self.sim.is_paused() && self.peers.len() == 1)
                {
                    self.notice(id, "Rearrange Memories between encounters.");
                } else {
                    accepted = self.sim.swap_memories_for(player, a as usize, b as usize);
                }
            }
            DreamAction::BuyMemoryUpgrade { slot } => {
                accepted = self.sim.buy_memory_upgrade_for(player, slot as usize)
            }
            DreamAction::Restart => {
                if self.peers.len() == 1
                    || matches!(self.sim.phase(), RunPhase::Victory | RunPhase::Defeat)
                {
                    self.restart(self.lucid, true);
                    self.begin_run();
                } else {
                    self.notice(id, "Finish the shared run before starting another.");
                }
            }
            DreamAction::Pause { paused } => {
                if self.peers.len() == 1 {
                    self.sim.set_paused(paused);
                } else {
                    self.notice(
                        id,
                        "The shared dream keeps moving while party members use menus.",
                    );
                }
            }
            DreamAction::Cast { .. }
            | DreamAction::Dash { .. }
            | DreamAction::Dreamlance { .. }
            | DreamAction::BeamBegin { .. }
            | DreamAction::BeamStop
            | DreamAction::ChargeBegin
            | DreamAction::ChargeRelease { .. }
            | DreamAction::ChargeCancel { .. } => accepted = false,
        }
        if !accepted {
            self.notice(id, "That choice is not available in the current encounter.");
        }
    }
    fn step_committed(&mut self, context: engine_net::clock::TickContext) {
        self.now = self.now.max(context.observed_now);
        if self.tick.checked_next() != Some(context.tick) {
            self.failed.extend(self.peers.keys());
            return;
        }
        self.apply_handoffs(context.tick);
        for id in std::mem::take(&mut self.pending_admissions) {
            let Some(player) = self.peers.get(&id).map(|peer| peer.welcome.player.get()) else {
                continue;
            };
            if !self.sim.add_pending_player(player) {
                self.failed.insert(id);
            }
        }
        if self.peers.len() > 1 {
            self.sim.set_paused(false);
        }
        for peer in self.peers.values_mut() {
            if peer.interrupt_charge_pending {
                self.sim.interrupt_player_charge(peer.welcome.player.get());
                peer.interrupt_charge_pending = false;
            }
            if peer.activate_pending {
                if peer.welcome.stream.ownership.0 == 1 {
                    self.sim.set_player_active(peer.welcome.player.get(), true);
                } else {
                    self.sim.resume_player(peer.welcome.player.get());
                }
                peer.activation_committed = true;
                peer.activate_pending = false;
            }
        }
        let original_epoch = self.epoch;
        let menus: Vec<_> = self
            .peers
            .iter_mut()
            .filter_map(|(&id, peer)| {
                peer.menus
                    .pop_front()
                    .map(|(seq, action)| (id, seq, action))
            })
            .collect();
        for (id, seq, action) in menus {
            if self.epoch != original_epoch {
                break;
            }
            self.menu(id, action);
            if self.epoch == original_epoch {
                if let Some(peer) = self.peers.get_mut(&id) {
                    peer.menu_applied = Some(seq);
                }
            }
        }
        if context
            .tick
            .0
            .is_multiple_of(u64::from(dreamwake_sim::TICK_HZ))
        {
            self.outcomes.expire(context.tick);
        }
        let mut explicit = Vec::with_capacity(64);
        let mut rejected = Vec::new();
        let mut inputs = Vec::with_capacity(self.peers.len());
        for (&id, peer) in &mut self.peers {
            peer.messages = 0;
            peer.input_messages = 0;
            match peer.inbox.prepare_next(&Rules(peer.welcome.stream)) {
                Ok(prepared) if prepared.tick() == context.tick => {
                    let arrival = prepared
                        .sequence()
                        .and_then(|sequence| peer.combat.take_arrival(sequence));
                    let input = *prepared.input();
                    if let Some(sequence) = prepared.sequence() {
                        let overloaded = peer.overload_commands.remove(&sequence);
                        let owner = peer.welcome.player.get();
                        let generation = self.sim.combat_generation(owner);
                        for edge in prepared.actions() {
                            let action_key = ActionKey {
                                connection: peer.welcome.stream.epoch,
                                stream: peer.welcome.stream.stream,
                                command: sequence,
                                slot: edge.slot,
                            };
                            let reason = if overloaded {
                                Some(live::OutcomeReason::Capacity)
                            } else if !peer.active {
                                Some(live::OutcomeReason::Game(
                                    dreamwake_sim::combat::ActionReason::Inactive,
                                ))
                            } else if generation.is_none() {
                                Some(live::OutcomeReason::Game(
                                    dreamwake_sim::combat::ActionReason::InvalidAction,
                                ))
                            } else if explicit.len() >= 64 {
                                Some(live::OutcomeReason::Game(
                                    dreamwake_sim::combat::ActionReason::Budget,
                                ))
                            } else {
                                None
                            };
                            if let Some(reason) = reason {
                                rejected.push((owner, action_key, overloaded, reason));
                                continue;
                            }
                            let key = dreamwake_sim::combat::RayActionKey {
                                match_epoch: self.epoch,
                                connection_epoch: peer.welcome.stream.epoch.0,
                                command_stream: peer.welcome.stream.stream.0,
                                ownership_epoch: peer.welcome.stream.ownership.0,
                                actor: owner,
                                actor_generation: generation.unwrap(),
                                command_sequence: sequence.0,
                                action_slot: edge.slot,
                            };
                            use dreamwake_sim::combat::{CombatAction, ValidatedRay};
                            let action = match edge.action {
                                DreamAction::Dash { direction } => CombatAction::Dash {
                                    key,
                                    execution_server_tick: context.tick.0,
                                    direction,
                                },
                                DreamAction::Cast { slot, aim } => CombatAction::Cast {
                                    key,
                                    execution_server_tick: context.tick.0,
                                    slot,
                                    aim,
                                },
                                DreamAction::ChargeBegin
                                | DreamAction::ChargeRelease { .. }
                                | DreamAction::ChargeCancel { .. } => CombatAction::Charge {
                                    key,
                                    execution_server_tick: context.tick.0,
                                    command: match edge.action {
                                        DreamAction::ChargeBegin => {
                                            dreamwake_sim::ChargeCommand::Begin {
                                                episode: sequence.0,
                                            }
                                        }
                                        DreamAction::ChargeRelease { episode } => {
                                            dreamwake_sim::ChargeCommand::Release { episode }
                                        }
                                        DreamAction::ChargeCancel { episode } => {
                                            dreamwake_sim::ChargeCommand::Cancel { episode }
                                        }
                                        _ => unreachable!(),
                                    },
                                    aim: input.held.aim,
                                },
                                DreamAction::BeamStop => CombatAction::BeamStop {
                                    key,
                                    execution_server_tick: context.tick.0,
                                },
                                DreamAction::Dreamlance { aim, view }
                                | DreamAction::BeamBegin { aim, view } => {
                                    let timing = arrival
                                        .ok_or(
                                            engine_net::combat_timing::TimingError::InvalidContext,
                                        )
                                        .and_then(|arrival| {
                                            peer.combat.validate(
                                                view,
                                                arrival,
                                                context.tick,
                                                &self.combat_clock,
                                                self.epoch,
                                            )
                                        });
                                    let Ok((query, gameplay, fraction, _clamped)) = timing else {
                                        rejected.push((
                                            owner,
                                            action_key,
                                            false,
                                            live::OutcomeReason::Timing,
                                        ));
                                        continue;
                                    };
                                    let ray = ValidatedRay {
                                        command_fraction: Some(view.sampled_fraction),
                                        query_fraction: fraction,
                                        key,
                                        execution_server_tick: context.tick.0,
                                        query_server_tick: query.0,
                                        query_gameplay_tick: gameplay,
                                        aim,
                                    };
                                    if matches!(edge.action, DreamAction::BeamBegin { .. }) {
                                        CombatAction::BeamBegin(ray)
                                    } else {
                                        CombatAction::Ray(ray)
                                    }
                                }
                                _ => {
                                    rejected.push((
                                        owner,
                                        action_key,
                                        false,
                                        live::OutcomeReason::Game(
                                            dreamwake_sim::combat::ActionReason::InvalidAction,
                                        ),
                                    ));
                                    continue;
                                }
                            };
                            explicit.push(action);
                        }
                        if let (Some(sample), Some(arrival), Some(generation)) =
                            (input.beam, arrival, generation)
                        {
                            if !overloaded && peer.active && explicit.len() < 64 {
                                if let Ok((query, gameplay, fraction, _)) = peer.combat.validate(
                                    sample.view,
                                    arrival,
                                    context.tick,
                                    &self.combat_clock,
                                    self.epoch,
                                ) {
                                    explicit.push(dreamwake_sim::combat::CombatAction::BeamAim(
                                        dreamwake_sim::combat::ValidatedRay {
                                            command_fraction: Some(sample.view.sampled_fraction),
                                            query_fraction: fraction,
                                            key: dreamwake_sim::combat::RayActionKey {
                                                match_epoch: self.epoch,
                                                connection_epoch: peer.welcome.stream.epoch.0,
                                                command_stream: peer.welcome.stream.stream.0,
                                                ownership_epoch: peer.welcome.stream.ownership.0,
                                                actor: owner,
                                                actor_generation: generation,
                                                command_sequence: sequence.0,
                                                action_slot: 0,
                                            },
                                            execution_server_tick: context.tick.0,
                                            query_server_tick: query.0,
                                            query_gameplay_tick: gameplay,
                                            aim: sample.aim,
                                        },
                                    ));
                                }
                            }
                        }
                    }
                    if peer.active {
                        inputs.push((peer.welcome.player.get(), input.held));
                    }
                }
                _ => {
                    self.failed.insert(id);
                }
            }
        }
        self.sim
            .set_combat_trace_enabled(self.action_trace.enabled());
        match self.sim.step_multiplayer_with_actions(&inputs, &explicit) {
            Ok(verdicts) => {
                for verdict in verdicts {
                    let owner = verdict.key.actor;
                    let bindings = self
                        .sim
                        .starfall_bindings(verdict.key)
                        .into_iter()
                        .map(|(ordinal, id)| {
                            self.replication
                                .reserve_projectile_identity(id)
                                .map(|entity| (ordinal, entity))
                        })
                        .collect::<Result<Vec<_>, _>>();
                    let result = bindings.map_err(|e| e.to_string()).and_then(|bindings| {
                        self.outcomes
                            .verdict_with_bindings(owner, verdict, bindings)
                    });
                    if let Err(error) = result {
                        log::error!("Dreamwake terminal result: {error}");
                        self.failed
                            .extend(self.peers.iter().filter_map(|(&id, peer)| {
                                (peer.welcome.player.get() == owner).then_some(id)
                            }));
                    }
                }
            }
            Err(error) => {
                // The game rejects structural overflow before advancing. Preserve
                // one authoritative tick and surface every reservation as Budget.
                self.sim.step_multiplayer(&inputs);
                log::error!("Dreamwake action batch rejected: {error:?}");
                for (&id, peer) in &self.peers {
                    for key in self
                        .outcomes
                        .normal
                        .pending(peer.welcome.player.get(), peer.welcome.stream.epoch)
                    {
                        rejected.push((
                            peer.welcome.player.get(),
                            key,
                            false,
                            live::OutcomeReason::Game(dreamwake_sim::combat::ActionReason::Budget),
                        ));
                    }
                    self.failed.insert(id);
                }
            }
        }
        for (owner, key, overload, reason) in rejected {
            let ledger = if overload {
                &mut self.outcomes.overload
            } else {
                &mut self.outcomes.normal
            };
            if let Err(error) = ledger.finish(
                owner,
                key,
                context.tick,
                outcomes::rejection(live::TerminalStatus::Rejected, reason),
            ) {
                log::error!("Dreamwake terminal rejection: {error:?}");
            }
        }
        self.tick = context.tick;
        self.combat_clock
            .record(self.epoch, self.tick, self.sim.gameplay_tick());
        self.action_trace
            .complete(|owner, key| self.outcomes.lookup(owner, key));
        self.action_trace
            .combat(self.server_instance, self.sim.take_combat_traces());
        for (&id, peer) in &mut self.peers {
            if peer.inbox.commit(context.tick).is_err() {
                self.failed.insert(id);
            }
        }
    }
    #[cfg(test)]
    fn step(&mut self, now: Duration) {
        self.step_committed(engine_net::clock::TickContext {
            tick: self.tick.checked_next().unwrap(),
            deadline: now,
            observed_now: now,
        });
    }
}
impl engine_server::Authority for DreamAuthority {
    fn update_health(&mut self, health: engine_net::clock::TickHealth) {
        self.overloaded = health.overloaded;
    }
    fn admit(&mut self, _: u64, _: &mut RenetServer) -> bool {
        false
    }
    fn admit_authenticated(
        &mut self,
        id: u64,
        grant: Option<&renet_cross::SessionGrant>,
        server: &mut RenetServer,
    ) -> bool {
        let Some(player) = grant.and_then(crate::admission::grant_player) else {
            return false;
        };
        if self
            .peers
            .values()
            .any(|peer| peer.welcome.player == player && peer.welcome.client.0 != id)
        {
            let Some(grant) = grant else {
                return false;
            };
            let Ok(unix) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            else {
                return false;
            };
            let Some(remaining) = Duration::from_secs(grant.expires_at).checked_sub(unix) else {
                return false;
            };
            let Some(deadline) = self.now.checked_add(remaining) else {
                return false;
            };
            return self.stage_handoff(id, player, deadline);
        }
        if !self.admit_player(id, player) {
            return false;
        }
        publish::welcome(server, id, self.peers.get_mut(&id).unwrap()).is_ok()
    }
    fn disconnect(&mut self, id: u64) {
        DreamAuthority::disconnect(self, id);
    }
    fn receive(&mut self, server: &mut RenetServer, elapsed: Duration) {
        self.now = self.now.max(elapsed);
        let ids: Vec<_> = self.peers.keys().copied().collect();
        for id in ids {
            for channel in [wire::CONTROL_CHANNEL, wire::INPUT_CHANNEL] {
                for _ in 0..MAX_MESSAGES_PER_TICK {
                    // A legal compact publication can return many reliable
                    // decode proofs in one network wakeup. Preserve their
                    // bounded queue until the next committed tick's work budget.
                    if self.peers[&id].messages >= MAX_MESSAGES_PER_TICK {
                        break;
                    }
                    // Reserve work for a full bounded bundle before dequeueing.
                    // Network wakeups may accumulate legal redundant inputs while
                    // simulation is stalled; drain them over committed ticks.
                    if channel == wire::INPUT_CHANNEL
                        && self.peers[&id].input_messages
                            >= engine_net::commands::CommandLimits::default().submissions_per_tick
                                / 8
                    {
                        break;
                    }
                    if channel == wire::INPUT_CHANNEL
                        && !self
                            .outcomes
                            .can_consume_bundle(self.peers[&id].welcome.player.get())
                    {
                        break;
                    }
                    let Some(bytes) = server.receive_message(id, channel) else {
                        break;
                    };
                    let peer = self.peers.get_mut(&id).unwrap();
                    peer.messages += 1;
                    if channel == wire::INPUT_CHANNEL {
                        peer.input_messages += 1;
                    }
                    let epoch = peer.welcome.stream.epoch;
                    let limit = peer.limit();
                    let result = if channel == wire::CONTROL_CHANNEL {
                        match live::decode_control(&bytes, epoch, limit) {
                            Ok(message) => self.receive_control(id, message, server),
                            Err(engine_net::codec::CodecError::WrongEpoch) => Ok(()),
                            Err(error) => Err(failure(error)),
                        }
                    } else {
                        match live::decode_commands(&bytes, epoch, limit) {
                            Ok(message) => self.receive_bundle(id, message),
                            Err(engine_net::codec::CodecError::WrongEpoch) => Ok(()),
                            Err(error) => Err(failure(error)),
                        }
                    };
                    if let Err(reason) = result {
                        log::warn!("Disconnect Dreamwake peer {id}: {reason}");
                        self.failed.insert(id);
                        break;
                    }
                }
            }
            if let Some(peer) = self.peers.get_mut(&id) {
                if peer.welcome_pending && publish::welcome(server, id, peer).is_err() {
                    self.failed.insert(id);
                }
            }
        }
        publish::disconnect_failed(server, self);
    }
    fn step(&mut self, elapsed: Duration) {
        let Some(tick) = self.tick.checked_next() else {
            self.failed.extend(self.peers.keys());
            return;
        };
        self.step_committed(engine_net::clock::TickContext {
            tick,
            deadline: elapsed,
            observed_now: elapsed,
        });
    }
    fn step_tick(&mut self, context: engine_net::clock::TickContext) {
        self.step_committed(context);
    }
    fn publish(&mut self, server: &mut RenetServer) {
        publish::states(server, self);
    }
}
