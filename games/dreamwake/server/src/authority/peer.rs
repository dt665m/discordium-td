use super::*;
use dreamwake_sim::replication::ReplicaState;
use engine_net::{
    commands::{CommandInbox, CommandLimits, CommandRules, OwnerStream},
    replication::stream::ServerStream,
};

pub(super) struct Rules(pub OwnerStream);
impl CommandRules<live::TickInput, DreamAction> for Rules {
    fn owns_at(&self, owner: OwnerStream, _: TargetTick) -> bool {
        owner == self.0
    }
    fn valid_input(&self, input: &live::TickInput) -> bool {
        live::valid_tick_input(input)
    }
    fn valid_action(&self, action: &DreamAction) -> bool {
        live::valid_tick_action(action)
    }
    fn held_input(&self, previous: &live::TickInput) -> live::TickInput {
        // Held movement and attack have a three-tick grace; action edges never do.
        previous.held.held_only().into()
    }
    fn neutral_input(&self) -> live::TickInput {
        live::TickInput::default()
    }
}

#[derive(Default)]
pub(super) struct EgressCounters {
    pub control_messages: u64,
    pub control_bytes: u64,
    pub state_packets: u64,
    pub state_bytes: u64,
    pub state_entries: u64,
    /// Publications cut short because the connection's egress allowance was
    /// spent; the remaining entities went out in a later publication.
    pub allowance_limited: u64,
}

/// Late actions a connection may have waiting at once; more expire.
pub(super) const MAX_LATE_ACTIONS: usize = 4;

pub(super) fn can_defer_action(action: &DreamAction) -> bool {
    matches!(
        action,
        DreamAction::Dash { .. }
            | DreamAction::Cast { .. }
            | DreamAction::ChargeBegin
            | DreamAction::ChargeRelease { .. }
            | DreamAction::ChargeCancel { .. }
    )
}

/// Which commands have reached the server: every sequence through a
/// watermark, plus a bounded set of later ones that arrived first.
#[derive(Debug, Default)]
pub(super) struct Received {
    through: u64,
    above: BTreeSet<u64>,
}
impl Received {
    /// Sequences tracked beyond the watermark; a gap wider than this is far
    /// outside anything the client still repeats.
    const WINDOW: u64 = 256;
    pub fn through(&self) -> CommandSeq {
        CommandSeq(self.through)
    }
    pub fn mark(&mut self, sequence: CommandSeq) {
        let sequence = sequence.0;
        if sequence <= self.through {
            return;
        }
        if sequence > self.through + Self::WINDOW {
            // The client has moved on past a gap that can never fill.
            self.through = sequence - Self::WINDOW;
            self.above.retain(|&seen| seen > sequence - Self::WINDOW);
        }
        self.above.insert(sequence);
        while self.above.remove(&(self.through + 1)) {
            self.through += 1;
        }
    }
}

/// Entities a connection can hold at once: every relevant actor plus the
/// owner and match summary.
const MAX_STREAM_ENTITIES: usize = 4096;

pub(super) struct Peer {
    pub combat: super::combat::PeerCombat,
    pub overload_commands: BTreeSet<CommandSeq>,
    pub welcome: live::Welcome,
    pub welcome_pending: bool,
    pub ready: bool,
    /// The hero participates and its commands apply.
    pub active: bool,
    pub activate_pending: bool,
    pub next_outcome_lookup: Duration,
    pub outcome_batches: engine_net::delivery::ReceiptWindow<u64>,
    pub next_outcome_batch: u64,
    pub outcome_lookups: BTreeSet<ActionKey>,
    pub inbox: CommandInbox<live::TickInput, DreamAction>,
    pub stream: ServerStream<EntityId, ReplicaState>,
    pub publications: u64,
    pub frame_sequence: u32,
    pub egress: EgressCounters,
    pub next_egress_report: Duration,
    pub last_pacing_drops: u64,
    /// Transport-refused sends, reported separately from pacing.
    pub last_backend_drops: u64,
    pub menu_received: CommandSeq,
    pub menu_applied: Option<CommandSeq>,
    pub received: Received,
    /// Deferred actions in command/slot order, with whether their outcome was
    /// reserved on the overload ledger. Newer deferrable edges cannot overtake them.
    pub late_actions: VecDeque<(
        CommandSeq,
        engine_net::commands::ActionEdge<DreamAction>,
        bool,
    )>,
    pub menus: VecDeque<(CommandSeq, DreamAction)>,
    pub messages: usize,
    pub input_messages: usize,
    pub arrival: live::ArrivalFeedback,
    /// When a message from this connection was last dequeued.
    pub last_heard: Duration,
}
impl Peer {
    pub fn new(welcome: live::Welcome, now: Duration) -> Result<Self, String> {
        Ok(Self {
            combat: Default::default(),
            overload_commands: Default::default(),
            inbox: CommandInbox::new(
                welcome.stream,
                welcome.tick,
                CommandLimits {
                    held_grace_ticks: dreamwake_sim::HELD_INPUT_GRACE_TICKS,
                    ..Default::default()
                },
            )
            .map_err(failure)?,
            welcome,
            welcome_pending: true,
            ready: false,
            active: false,
            activate_pending: false,
            next_outcome_lookup: Duration::ZERO,
            outcome_batches: engine_net::delivery::ReceiptWindow::new(0, 4).map_err(failure)?,
            next_outcome_batch: 0,
            outcome_lookups: BTreeSet::new(),
            stream: ServerStream::new(MAX_STREAM_ENTITIES),
            publications: 0,
            frame_sequence: 0,
            egress: EgressCounters::default(),
            next_egress_report: now + Duration::from_secs(1),
            last_pacing_drops: 0,
            last_backend_drops: 0,
            menu_received: CommandSeq(0),
            menu_applied: None,
            received: Received::default(),
            late_actions: VecDeque::new(),
            menus: VecDeque::new(),
            messages: 0,
            input_messages: 0,
            arrival: live::ArrivalFeedback::default(),
            last_heard: now,
        })
    }
    pub fn next_frame(&mut self) -> Result<u32, String> {
        self.frame_sequence = self
            .frame_sequence
            .checked_add(1)
            .ok_or_else(|| "Frame sequence exhausted".to_owned())?;
        Ok(self.frame_sequence)
    }
    pub fn limit(&self) -> engine_net::codec::FrameLimit {
        self.welcome
            .frame_limit()
            .expect("validated frame allowance")
    }
    pub fn queue_menu(&mut self, sequence: CommandSeq, action: DreamAction) -> Result<(), String> {
        if sequence <= self.menu_received {
            return Ok(());
        }
        if sequence.0 - self.menu_received.0 > 256 || self.menus.len() >= MAX_PENDING_ACTIONS {
            return Err("Menu transaction window exceeded".into());
        }
        self.menu_received = sequence;
        self.menus.push_back((sequence, action));
        Ok(())
    }
    /// The client holds an acknowledged copy of its own hero, so its owner
    /// state can start replacing prediction.
    pub fn holds_owner(&self) -> bool {
        self.stream.base(self.welcome.owner_entity).is_some()
    }
}

#[cfg(test)]
mod received_tests {
    use super::*;

    #[test]
    fn the_watermark_advances_only_through_contiguous_arrivals() {
        let mut received = Received::default();
        received.mark(CommandSeq(2));
        assert_eq!(received.through(), CommandSeq(0));
        received.mark(CommandSeq(1));
        assert_eq!(received.through(), CommandSeq(2));
        received.mark(CommandSeq(2));
        received.mark(CommandSeq(4));
        assert_eq!(received.through(), CommandSeq(2));
        received.mark(CommandSeq(3));
        assert_eq!(received.through(), CommandSeq(4));
        // A gap the client can no longer fill is passed rather than held.
        received.mark(CommandSeq(1_000));
        assert_eq!(received.through(), CommandSeq(1_000 - Received::WINDOW));
        assert!(received.above.len() <= Received::WINDOW as usize);
    }
}
