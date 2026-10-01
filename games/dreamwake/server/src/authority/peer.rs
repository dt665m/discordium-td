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
    pub menus: VecDeque<(CommandSeq, DreamAction)>,
    pub messages: usize,
    pub input_messages: usize,
    pub arrival: live::ArrivalFeedback,
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
            menus: VecDeque::new(),
            messages: 0,
            input_messages: 0,
            arrival: live::ArrivalFeedback::default(),
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
