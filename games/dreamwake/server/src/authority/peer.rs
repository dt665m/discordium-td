use super::*;
use engine_net::{
    commands::{CommandInbox, CommandLimits, CommandRules, OwnerStream},
    scheduling::{Scheduler, SchedulerLimits},
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

pub(super) struct FrozenGroup {
    pub members: Vec<engine_net::replication::FullState<Vec<u8>>>,
    pub fragments: Vec<engine_net::replication::ByteFragment>,
    /// Cursor in the sole bounded immutable transfer. A complete pass is needed
    /// before any member's sent receipt or baseline is committed.
    pub next_fragment: usize,
    pub sent_once: bool,
    pub decoded: BTreeSet<EntityId>,
    pub started: Duration,
    pub active_state: bool,
    pub baseline_packets: Vec<engine_net::replication::baselines::BaselinePacket>,
}
#[derive(Default)]
pub(super) struct EgressCounters {
    pub control_messages: u64,
    pub control_bytes: u64,
    pub finalized_messages: u64,
    pub finalized_bytes: u64,
    pub finalized_receipts: u64,
    pub finalized_deferred: u64,
    pub state_messages: u64,
    pub state_bytes: u64,
    pub group_passes: u64,
    pub group_fragments: u64,
    pub group_age_ticks: u64,
    pub maximum_group_age_ticks: u64,
    pub publication_credit: usize,
    pub publication_prefix: usize,
    pub control_due_messages: usize,
    pub control_due_payload_bytes: usize,
}

pub(super) struct Peer {
    pub combat: super::combat::PeerCombat,
    pub overload_commands: BTreeSet<CommandSeq>,
    pub baselines: super::baselines::OwnerBaselines,
    pub welcome: live::Welcome,
    pub welcome_pending: bool,
    pub ready: bool,
    pub active: bool,
    pub activate_pending: bool,
    pub interrupt_charge_pending: bool,
    pub activation_committed: bool,
    pub resyncs: u64,
    /// At most three recoveries within the sliding admission window. Carried
    /// across stream replacement so requesting a new epoch cannot reset it.
    pub resync_attempts: VecDeque<Duration>,
    pub next_outcome_lookup: Duration,
    pub finalized_batches: engine_net::delivery::ReceiptWindow<ServerTick>,
    pub outcome_batches: engine_net::delivery::ReceiptWindow<u64>,
    pub next_outcome_batch: u64,
    pub outcome_lookups: BTreeSet<ActionKey>,
    pub inbox: CommandInbox<live::TickInput, DreamAction>,
    pub scheduler: Scheduler,
    /// Frozen through decode during bootstrap; after activation only the newest
    /// complete publication is retained, independent of older decode receipts.
    pub frozen: Option<FrozenGroup>,
    pub group_revision: GroupRevision,
    pub group_manifest: Vec<ScopeIdentity>,
    pub owner_bases: Vec<EntityId>,
    pub frame_sequence: u32,
    /// Queued on ReliableOrdered, not decoded or simulation acknowledgement.
    pub finalized_sent_through: ServerTick,
    pub egress: EgressCounters,
    pub next_egress_report: Duration,
    pub last_pacing_drops: u64,
    pub menu_received: CommandSeq,
    pub menu_applied: Option<CommandSeq>,
    pub menus: VecDeque<(CommandSeq, DreamAction)>,
    pub messages: usize,
    pub input_messages: usize,
    pub arrival: live::ArrivalFeedback,
    pub initial_deadline: Duration,
}
impl Peer {
    pub fn new(welcome: live::Welcome, now: Duration) -> Result<Self, String> {
        let finalized_sent_through = welcome.tick;
        Ok(Self {
            combat: Default::default(),
            overload_commands: Default::default(),
            baselines: Default::default(),
            inbox: CommandInbox::new(
                welcome.stream,
                welcome.tick,
                CommandLimits {
                    held_grace_ticks: dreamwake_sim::HELD_INPUT_GRACE_TICKS,
                    ..Default::default()
                },
            )
            .map_err(failure)?,
            scheduler: Scheduler::new(
                welcome.client,
                SchedulerLimits {
                    bytes_per_second: crate::DEFAULT_EGRESS_BYTES_PER_SECOND,
                    ..SchedulerLimits::default()
                },
            )
            .map_err(failure)?,
            welcome,
            welcome_pending: true,
            ready: false,
            active: false,
            activate_pending: false,
            interrupt_charge_pending: false,
            activation_committed: false,
            resyncs: 0,
            resync_attempts: VecDeque::new(),
            next_outcome_lookup: Duration::ZERO,
            finalized_batches: engine_net::delivery::ReceiptWindow::new(finalized_sent_through, 4)
                .map_err(failure)?,
            outcome_batches: engine_net::delivery::ReceiptWindow::new(0, 4).map_err(failure)?,
            next_outcome_batch: 0,
            outcome_lookups: BTreeSet::new(),
            frozen: None,
            group_revision: GroupRevision(0),
            group_manifest: Vec::new(),
            owner_bases: Vec::new(),
            frame_sequence: 0,
            finalized_sent_through,
            egress: EgressCounters::default(),
            next_egress_report: now + Duration::from_secs(1),
            last_pacing_drops: 0,
            menu_received: CommandSeq(0),
            menu_applied: None,
            menus: VecDeque::new(),
            messages: 0,
            input_messages: 0,
            arrival: live::ArrivalFeedback::default(),
            initial_deadline: now.saturating_add(Duration::from_secs(10)),
        })
    }
    pub fn owner_group_entities(&self) -> Vec<EntityId> {
        let mut members = vec![
            self.welcome.global_entity,
            self.welcome.owner_entity,
            self.welcome.collision_entity,
        ];
        members.extend_from_slice(&self.owner_bases);
        members
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
}
