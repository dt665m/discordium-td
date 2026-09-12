//! Persistent production baseline and event journals; only explicit retirement frees history.
use engine_net::{events, replication::baselines::*, types::*};

pub struct Retirement {
    server: ServerBaselines<Vec<u8>>,
    client: ClientBaselines<Vec<u8>>,
    context: BaselineContext,
    pending: Option<BaselineRetirement>,
    retired_through: u64,
    journal: events::EventJournal<Vec<u8>>,
    pub retired: u64,
    pub lost: u64,
}
impl Retirement {
    pub fn new(owner: EntityId) -> Self {
        let context = BaselineContext {
            scope: ScopeIdentity {
                connection: ConnectionEpoch(1),
                entity: owner,
                scope: ScopeEpoch(1),
                representation: RepresentationRevision(1),
            },
            schema: SchemaId(1),
            group: None,
        };
        Self {
            server: ServerBaselines::new(context, BaselineGeneration(1), BaselineLimits::default())
                .unwrap(),
            client: ClientBaselines::new(context, BaselineGeneration(1), BaselineLimits::default())
                .unwrap(),
            context,
            pending: None,
            retired_through: 0,
            journal: events::EventJournal::new(
                events::JournalScope {
                    connection: ConnectionEpoch(1),
                    stream: CommandStream(1),
                },
                events::Limits {
                    records: 64,
                    bindings: 8,
                    action_fences: 64,
                    transitions: 8,
                    payload_bytes: 64,
                    resident_bytes: 128 * 1024,
                    transition_bytes: 4096,
                    peak_bytes: 512 * 1024,
                    history_ticks: 32,
                    lifetime_ticks: 4,
                },
            )
            .unwrap(),
            retired: 0,
            lost: 0,
        }
    }
    pub fn step(&mut self, tick: u64) {
        if let Some(request) = self.pending.take() {
            let fence = self.server.retire(request).unwrap();
            self.client.acknowledge_retirement(fence).unwrap();
            self.retired += 1;
        }
        let mut payload = vec![0; 64];
        payload[..8].copy_from_slice(&tick.to_le_bytes());
        let target = BaselineState {
            receipt: BaselineReceipt {
                context: self.context,
                generation: BaselineGeneration(1),
                snapshot: SnapshotId(tick),
                version: StateVersion(tick),
                end_tick: ServerTick(tick),
            },
            payload,
        };
        let mut packet = None;
        self.server
            .transmit(target, self.server.newest_decoded(), &BytePatch, |p| {
                packet = Some(p.clone());
                true
            })
            .unwrap();
        if tick % 11 != 0 {
            let proof = self
                .client
                .receive(&packet.unwrap(), &BytePatch, |_| true)
                .unwrap();
            self.server.acknowledge(proof).unwrap();
        } else {
            self.lost += 1;
        }
        if let Some(current) = self.client.current() {
            if current.receipt.snapshot.0 > self.retired_through + 2 {
                let request = self
                    .client
                    .propose_retirement(SnapshotId(current.receipt.snapshot.0 - 2))
                    .unwrap();
                self.retired_through = request.through.0;
                let fence = self.server.retire(request).unwrap();
                // One lost retirement acknowledgment; retry the exact request next tick.
                if tick % 13 == 0 {
                    self.pending = Some(request);
                    self.lost += 1;
                } else {
                    self.client.acknowledge_retirement(fence).unwrap();
                    self.retired += 1;
                }
            }
        }
        assert!(self.server.retained_states() <= 8 && self.client.retained_states() <= 8);
        assert!(self.server.retained_bytes() <= BaselineLimits::default().resident_bytes);
        assert!(self.client.retained_bytes() <= BaselineLimits::default().resident_bytes);
        let key = events::EventKey {
            action: ActionKey {
                connection: ConnectionEpoch(1),
                stream: CommandStream(1),
                command: CommandSeq(tick),
                slot: 0,
            },
            kind: events::EventKind(1),
            spawn_ordinal: 0,
        };
        let event = events::EventRecord {
            identity: events::EventIdentity {
                key,
                origin_tick: ServerTick(tick),
                schema: SchemaId(1),
            },
            expires_at: ServerTick(tick + 2),
            delivery: events::Delivery::Reversible,
            payload: vec![0; 32],
        };
        self.journal
            .reconcile(
                ServerTick(tick),
                events::ReplayRange {
                    first: ServerTick(tick),
                    last: ServerTick(tick),
                },
                &[event],
                |_| true,
            )
            .unwrap();
        if tick > 32 {
            self.journal.retire_through(CommandSeq(tick - 32)).unwrap();
        }
        assert!(self.journal.records() <= 33);
        assert!(self.journal.retained_bytes() <= events::Limits::default().resident_bytes);
    }
    pub fn bytes(&self) -> usize {
        self.server.retained_bytes() + self.client.retained_bytes() + self.journal.retained_bytes()
    }
    pub fn states(&self) -> usize {
        self.server.retained_states() + self.client.retained_states()
    }
    pub fn events(&self) -> usize {
        self.journal.records()
    }
}
