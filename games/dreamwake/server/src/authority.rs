//! Authenticated Dreamwake admission, fixed-tick commands and scoped publication.
#[cfg(test)]
mod arrival_tests;
mod combat;
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
const APPLICATION_FRAME_BYTES: u16 = live::APPLICATION_FRAME_BYTES as u16;
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
pub(crate) fn connection_config() -> renet::ConnectionConfig {
    wire::connection_config()
}

impl DreamAuthority {
    pub(crate) fn observe_transport(&mut self, shared: engine_server::SharedNet) {
        self.transport_diagnostics = Some(shared);
    }

    #[cfg(test)]
    pub(crate) fn new(seed: u64, lucid: bool, max_clients: usize) -> Self {
        Self::with_replication_distance(seed, lucid, max_clients, 80.0)
    }
    pub(crate) fn with_replication_distance(
        seed: u64,
        lucid: bool,
        max_clients: usize,
        replication_distance: f32,
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
            replication: Self::graph(1, replication_distance),
            epoch: 1,
            next_connection: ConnectionEpoch(1),
            tick: ServerTick(0),
            now: Duration::ZERO,
            revision: 0,
            max_clients: max_clients.min(dreamwake_sim::MAX_HEROES),
            seed,
            lucid,
            replication_distance,
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
    fn graph(epoch: u32, distance: f32) -> DreamReplication {
        DreamReplication::new(
            epoch,
            f64::from(distance),
            engine_net::interest::Limits {
                max_connections: wire::MAX_PLAYERS,
                max_entities: 8192,
                max_observers: 1,
                prefetch: 0.0,
                ..Default::default()
            },
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
        let (global_entity, owner_entity) = self
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
    fn receive_control(
        &mut self,
        id: u64,
        control: live::ClientControl,
        server: &mut RenetServer,
    ) -> Result<(), String> {
        match control {
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
        }
        Ok(())
    }
    fn receive_input(&mut self, id: u64, packet: live::InputPacket) -> Result<(), String> {
        let peer = self.peers.get_mut(&id).ok_or("Unknown peer")?;
        // Acknowledgements count before activation: they are how it happens.
        peer.stream.acknowledge(packet.ack).map_err(failure)?;
        peer.combat.acknowledge(packet.ack);
        if !peer.active {
            return Ok(());
        }
        // Receive and simulation are ordered phases. Never report a prepared
        // future cutoff as though it were committed arrival time.
        if peer.inbox.prepared_tick().is_some() {
            return Err("Input received during prepared simulation tick".into());
        }
        let rules = Rules(peer.welcome.stream);
        for record in packet.records.as_slice() {
            let command = &record.command(peer.welcome.stream);
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
                    Err(engine_net::outcomes::OutcomeError::Capacity) => {
                        match self.outcomes.overload.reserve(owner, &keys) {
                            Ok(keys) => (true, keys),
                            // The rest of this bundle repeats in the next
                            // packet; take it then.
                            Err(engine_net::outcomes::OutcomeError::Capacity) => break,
                            Err(error) => return Err(failure(error)),
                        }
                    }
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
                // This tick's admission work is spent; the remaining records
                // repeat in the next packet.
                Err(engine_net::commands::CommandError::RateLimit) => {
                    ledger.cancel_reservations(&reservations);
                    break;
                }
                Err(error) => {
                    ledger.cancel_reservations(&reservations);
                    return Err(failure(error));
                }
            };
            // A command too far ahead must come again once it fits.
            if admission != Admission::Future {
                peer.received.mark(command.sequence);
            }
            match admission {
                Admission::Accepted { .. } if overload => {
                    peer.overload_commands.insert(command.sequence);
                }
                Admission::Accepted { .. } => {}
                // A late action still runs, on the first tick without one of
                // its own, as Unreal's character movement executes a move when
                // it arrives. Lateness then shifts the action by a few ticks
                // instead of discarding it. Rays judged against a view time
                // keep their own rewind limit and still expire.
                Admission::Late
                    if previous_admission.is_none()
                        && !command.actions.as_slice().is_empty()
                        && command.actions.as_slice().iter().all(|edge| {
                            matches!(
                                edge.action,
                                DreamAction::Dash { .. }
                                    | DreamAction::Cast { .. }
                                    | DreamAction::ChargeBegin
                                    | DreamAction::ChargeRelease { .. }
                                    | DreamAction::ChargeCancel { .. }
                            )
                        })
                        && peer.late_actions.len() + command.actions.as_slice().len()
                            <= peer::MAX_LATE_ACTIONS =>
                {
                    for edge in command.actions.as_slice() {
                        peer.late_actions
                            .push_back((command.sequence, edge.clone(), overload));
                    }
                }
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
                    // Report the slack the window can actually express. Pinning
                    // a narrower number would hide how much headroom a distant
                    // player really has from its own lead controller.
                    Admission::Accepted { arrival_slack } => Some(
                        arrival_slack.min(u64::from(engine_net::types::MAX_COMMAND_LEAD_TICKS))
                            as u8,
                    ),
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
    /// Deliver commands as one input packet carrying no new acknowledgement.
    #[cfg(test)]
    fn receive_commands(
        &mut self,
        id: u64,
        commands: Vec<engine_net::commands::Command<live::TickInput, DreamAction>>,
    ) -> Result<(), String> {
        let ack = self
            .peers
            .get(&id)
            .ok_or("Unknown peer")?
            .stream
            .acknowledged();
        self.receive_input(
            id,
            live::InputPacket {
                ack,
                records: BoundedVec::new(
                    commands
                        .iter()
                        .map(live::InputRecord::from_command)
                        .collect(),
                )
                .map_err(failure)?,
            },
        )
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
        self.replication = Self::graph(epoch, self.replication_distance);
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
            if peer.activate_pending {
                if peer.welcome.stream.ownership.0 == 1 {
                    self.sim.set_player_active(peer.welcome.player.get(), true);
                } else {
                    self.sim.resume_player(peer.welcome.player.get());
                }
                peer.active = true;
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
                    let owner = peer.welcome.player.get();
                    let generation = self.sim.combat_generation(owner);
                    let prepared_command = prepared
                        .sequence()
                        .map(|sequence| (sequence, peer.overload_commands.remove(&sequence)));
                    let mut edges: Vec<_> = prepared_command
                        .map(|(sequence, overloaded)| {
                            prepared
                                .actions()
                                .iter()
                                .map(|edge| (sequence, edge.clone(), overloaded))
                                .collect()
                        })
                        .unwrap_or_default();
                    // One action per player per tick: a late one waits for a
                    // tick without its own.
                    if edges.is_empty() {
                        edges.extend(peer.late_actions.pop_front());
                    }
                    {
                        for (sequence, edge, overloaded) in edges {
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
                                                peer.welcome.stream.epoch,
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
                        if let (
                            Some((sequence, overloaded)),
                            Some(sample),
                            Some(arrival),
                            Some(generation),
                        ) = (prepared_command, input.beam, arrival, generation)
                        {
                            if !overloaded && peer.active && explicit.len() < 64 {
                                if let Ok((query, gameplay, fraction, _)) = peer.combat.validate(
                                    peer.welcome.stream.epoch,
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
            // A peer that never activated holds no participation, health,
            // progression or readiness by contract, so preserving it protects
            // nothing. Refusing the newcomer instead strands the profile until
            // the stalled connection times out. Displace the pending peer so a
            // fresh stream can join immediately.
            let stalled: Vec<u64> = self
                .peers
                .values()
                .filter(|peer| {
                    peer.welcome.player == player && peer.welcome.client.0 != id && !peer.active
                })
                .map(|peer| peer.welcome.client.0)
                .collect();
            if !stalled.is_empty() {
                for previous in stalled {
                    log::warn!(
                        "Dreamwake admission: displacing never-activated peer {previous} for player {}",
                        player.get()
                    );
                    server.disconnect(previous);
                    DreamAuthority::disconnect(self, previous);
                }
                return self.admit_player(id, player)
                    && publish::welcome(server, id, self.peers.get_mut(&id).unwrap()).is_ok();
            }
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
                    // Bound work per tick; anything beyond waits in Renet for
                    // the next committed tick.
                    if self.peers[&id].messages >= MAX_MESSAGES_PER_TICK {
                        break;
                    }
                    // Reserve work for a full input packet before dequeueing.
                    // Network wakeups may accumulate redundant input while
                    // simulation is stalled; drain it over committed ticks.
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
                        match live::decode_input(&bytes, epoch, live::input_limit()) {
                            Ok(message) => self.receive_input(id, message),
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
