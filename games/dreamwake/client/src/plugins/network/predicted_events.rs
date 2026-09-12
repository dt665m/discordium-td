//! One bounded journal for committed owner prediction effects and cosmetic cues.
use super::model::OwnerModel;
use dreamwake_protocol::live::{ActionOutcome, OWNER_GROUP, TerminalStatus};
use dreamwake_sim::starfall::{SpawnKey, StarfallFlight};
use engine_net::{events::*, prediction::PredictionManager, replication::Payload, types::*};
use std::collections::{BTreeMap, BTreeSet};

const PROJECTILE: EventKind = EventKind(1);
const CAST_SOUND: EventKind = EventKind(2);
const CAST_CAMERA: EventKind = EventKind(3);
const SCHEMA: SchemaId = SchemaId(0x1301);
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PresentationEvent {
    Projectile(StarfallFlight),
    Reserved(SpawnKey),
    CastSound,
    CastCamera,
}
impl Payload for PresentationEvent {
    fn retained_bytes(&self) -> usize {
        0
    }
}
pub(crate) struct PredictedEvents {
    journal: EventJournal<PresentationEvent>,
    pub(crate) sounds: u16,
    pub(crate) camera_cues: u16,
}
fn action(key: SpawnKey) -> ActionKey {
    ActionKey {
        connection: ConnectionEpoch(key.action.connection_epoch),
        stream: CommandStream(key.action.command_stream),
        command: CommandSeq(key.action.command_sequence),
        slot: key.action.action_slot,
    }
}
fn event(
    key: SpawnKey,
    kind: EventKind,
    origin: ServerTick,
    payload: PresentationEvent,
) -> EventRecord<PresentationEvent> {
    EventRecord {
        identity: EventIdentity {
            key: EventKey {
                action: action(key),
                kind,
                spawn_ordinal: u32::from(key.ordinal),
            },
            origin_tick: origin,
            schema: SCHEMA,
        },
        expires_at: ServerTick(origin.0 + 154),
        delivery: if kind == PROJECTILE {
            Delivery::SimulationOwned
        } else {
            Delivery::OneShot
        },
        payload,
    }
}
impl PredictedEvents {
    pub(crate) fn new(scope: JournalScope) -> Result<Self, String> {
        Ok(Self {
            journal: EventJournal::new(scope, Limits::default()).map_err(err)?,
            sounds: 0,
            camera_cues: 0,
        })
    }
    fn consume(&mut self, changes: Changes<PresentationEvent>) -> Result<(), String> {
        for change in changes.into_vec() {
            if let Change::Create { event, .. } = change {
                let count = match event.payload {
                    PresentationEvent::CastSound => Some(&mut self.sounds),
                    PresentationEvent::CastCamera => Some(&mut self.camera_cues),
                    _ => None,
                };
                if let Some(count) = count {
                    *count = count
                        .checked_add(1)
                        .filter(|n| *n <= 256)
                        .ok_or("presentation cue capacity")?;
                }
            }
            // BindRetired is deliberately identity-only. Rendering reads live
            // simulation state, so this transition cannot recreate a ghost.
        }
        Ok(())
    }
    /// Called only after the predictor published its entire successful update.
    pub(super) fn publish(
        &mut self,
        predictor: &PredictionManager<OwnerModel>,
    ) -> Result<(), String> {
        let Some(now) = predictor.predicted_tick(OWNER_GROUP) else {
            return Ok(());
        };
        let Some(current) = predictor.state(OWNER_GROUP) else {
            return Ok(());
        };
        let first = ServerTick(now.0.saturating_sub(255));
        let mut records = BTreeMap::new();
        let mut retired_keys = BTreeSet::new();
        let mut retained_command_head = None::<CommandSeq>;
        let current_keys = current
            .0
            .starfall_flights()
            .iter()
            .map(|f| f.key)
            .chain(current.0.starfall_repeats().iter().map(|r| r.key))
            .map(|key| EventKey {
                action: action(key),
                kind: PROJECTILE,
                spawn_ordinal: u32::from(key.ordinal),
            })
            .collect::<BTreeSet<_>>();
        for tick in first.0..=now.0 {
            if let Some(command) = predictor.command_history(OWNER_GROUP, ServerTick(tick)) {
                retained_command_head = Some(
                    retained_command_head
                        .map_or(command.0.sequence, |head| head.min(command.0.sequence)),
                );
            }
            if let Some(state) = predictor.history_state(OWNER_GROUP, ServerTick(tick)) {
                retired_keys.extend(state.0.starfall_retired().iter().map(|key| EventKey {
                    action: action(*key),
                    kind: PROJECTILE,
                    spawn_ordinal: u32::from(key.ordinal),
                }));
                for &key in state.0.starfall_cues() {
                    for (kind, payload) in [
                        (CAST_SOUND, PresentationEvent::CastSound),
                        (CAST_CAMERA, PresentationEvent::CastCamera),
                    ] {
                        let record = event(key, kind, ServerTick(tick), payload);
                        records.insert(record.identity.key, record);
                    }
                }
            }
        }
        let objects = current
            .0
            .starfall_flights()
            .iter()
            .map(|f| (f.key, PresentationEvent::Projectile(f.clone())))
            .chain(
                current
                    .0
                    .starfall_repeats()
                    .iter()
                    .map(|r| (r.key, PresentationEvent::Reserved(r.key))),
            );
        for (key, payload) in objects {
            let identity_key = EventKey {
                action: action(key),
                kind: PROJECTILE,
                spawn_ordinal: u32::from(key.ordinal),
            };
            let cue_key = EventKey {
                action: action(key),
                kind: CAST_SOUND,
                spawn_ordinal: 0,
            };
            // A newly observed checkpoint object is attributed to this admitted
            // baseline. This is not a reconstructed authoritative shot time.
            let origin = self
                .journal
                .get(identity_key)
                .map(|v| v.identity.origin_tick)
                .or_else(|| records.get(&cue_key).map(|r| r.identity.origin_tick))
                .unwrap_or(now);
            if origin >= first {
                let record = event(key, PROJECTILE, origin, payload);
                records.insert(record.identity.key, record);
            }
        }
        let live = self
            .journal
            .iter()
            .filter(|v| {
                v.phase == Phase::Live
                    && v.identity.key.kind == PROJECTILE
                    && !current_keys.contains(&v.identity.key)
                    && (v.decision == Decision::Accepted || retired_keys.contains(&v.identity.key))
            })
            .map(|v| v.identity)
            .collect::<Vec<_>>();
        for identity in live {
            let changes = self.journal.expire(now, identity).map_err(err)?;
            self.consume(changes)?;
        }
        let records = records.into_values().collect::<Vec<_>>();
        let changes = self
            .journal
            .reconcile(now, ReplayRange { first, last: now }, &records, |_| true)
            .map_err(err)?;
        self.consume(changes)?;
        // Completed cue identities have no ongoing presentation lifetime.
        let old = self
            .journal
            .iter()
            .filter(|v| {
                v.phase == Phase::Live
                    && v.identity.key.kind != PROJECTILE
                    && v.identity.origin_tick.0 + 154 <= now.0
            })
            .map(|v| v.identity)
            .collect::<Vec<_>>();
        for identity in old {
            let changes = self.journal.expire(now, identity).map_err(err)?;
            self.consume(changes)?;
        }
        let live_head = self
            .journal
            .iter()
            .filter(|v| v.phase == Phase::Live)
            .map(|v| v.identity.key.action.command)
            .min();
        let retired = self
            .journal
            .iter()
            .filter(|v| {
                v.identity.origin_tick < first
                    && v.phase != Phase::Live
                    && live_head.is_none_or(|head| v.identity.key.action.command < head)
            })
            .map(|v| v.identity.key.action.command)
            .max();
        let command_floor = retained_command_head
            .map(|head| CommandSeq(head.0.saturating_sub(1)))
            .map(|floor| {
                live_head.map_or(floor, |head| {
                    floor.min(CommandSeq(head.0.saturating_sub(1)))
                })
            });
        let retired = retired
            .into_iter()
            .chain(command_floor)
            .max()
            .filter(|value| *value > self.journal.retired_through());
        if let Some(retired) = retired {
            self.journal
                .retire_through(retired)
                .map_err(|error| format!("{error}: retire_through={retired:?}"))?;
        }
        Ok(())
    }
    pub(crate) fn outcome(&mut self, outcome: &ActionOutcome) -> Result<(), String> {
        let now = self.journal.now();
        if outcome.key.command <= self.journal.retired_through() {
            return Ok(());
        }
        if outcome.data.status != TerminalStatus::Accepted {
            let changes = self.journal.reject_action(now, outcome.key).map_err(err)?;
            return self.consume(changes);
        }
        let origin = self
            .journal
            .iter()
            .find(|v| v.identity.key.action == outcome.key)
            .map(|v| v.identity.origin_tick)
            .unwrap_or(now);
        for &(ordinal, binding) in outcome.data.spawn_bindings.as_slice() {
            let key = EventKey {
                action: outcome.key,
                kind: PROJECTILE,
                spawn_ordinal: u32::from(ordinal),
            };
            let identity = self
                .journal
                .get(key)
                .map(|v| v.identity)
                .unwrap_or(EventIdentity {
                    key,
                    origin_tick: origin,
                    schema: SCHEMA,
                });
            self.accept(identity, Some(binding))?;
        }
        let identities = self
            .journal
            .iter()
            .filter(|v| v.identity.key.action == outcome.key)
            .map(|v| v.identity)
            .collect::<Vec<_>>();
        for identity in identities {
            let binding = if identity.key.kind == PROJECTILE {
                outcome
                    .data
                    .spawn_bindings
                    .as_slice()
                    .iter()
                    .find(|(ordinal, _)| u32::from(*ordinal) == identity.key.spawn_ordinal)
                    .map(|(_, id)| *id)
            } else {
                None
            };
            self.accept(identity, binding)?;
        }
        Ok(())
    }
    /// Owner checkpoint identity and independently delivered public entity scope
    /// must agree before a delayed Echo receives its authoritative binding.
    pub(crate) fn bind(&mut self, key: SpawnKey, entity: EntityId) -> Result<(), String> {
        let key = EventKey {
            action: action(key),
            kind: PROJECTILE,
            spawn_ordinal: u32::from(key.ordinal),
        };
        let Some(identity) = self.journal.get(key).map(|v| v.identity) else {
            return Ok(());
        };
        self.accept(identity, Some(entity))
    }
    fn accept(&mut self, identity: EventIdentity, binding: Option<EntityId>) -> Result<(), String> {
        let changes = self
            .journal
            .resolve(self.journal.now(), identity, Outcome::Accepted { binding })
            .map_err(|error| {
                format!("{error}: accept key={:?} binding={binding:?}", identity.key)
            })?;
        self.consume(changes)
    }
    pub(crate) fn represented(&self, key: SpawnKey) -> bool {
        self.journal
            .get(EventKey {
                action: action(key),
                kind: PROJECTILE,
                spawn_ordinal: u32::from(key.ordinal),
            })
            .is_some_and(|v| v.phase == Phase::Live)
    }
    pub(crate) fn suppress_public(&self, entity: EntityId) -> bool {
        // A retired ghost's late binding also suppresses resurrection. The
        // ordinary authoritative scope will leave under its normal lifecycle.
        self.journal
            .iter()
            .any(|v| v.identity.key.kind == PROJECTILE && v.binding == Some(entity))
    }
}
fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key() -> SpawnKey {
        SpawnKey {
            action: dreamwake_sim::combat::RayActionKey {
                match_epoch: 1,
                connection_epoch: 1,
                command_stream: 1,
                ownership_epoch: 1,
                actor: 1,
                actor_generation: 1,
                command_sequence: 1,
                action_slot: 0,
            },
            ordinal: 0,
        }
    }
    fn adapter() -> PredictedEvents {
        PredictedEvents::new(JournalScope {
            connection: ConnectionEpoch(1),
            stream: CommandStream(1),
        })
        .unwrap()
    }
    fn flight() -> StarfallFlight {
        StarfallFlight {
            key: key(),
            origin_tick: 1,
            position: [0.0, 0.0],
            direction: [1.0, 0.0],
            radius: 0.34,
            remaining: 2.0,
            essence: None,
            authority_id: None,
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn newer_binding_before_removal_checkpoint_retires_old_projectile() {
        use super::super::{
            tests::{fixture, receipt_runtime},
            *,
        };
        use dreamwake_protocol::live::{ActionOutcomeData, OutcomeReason};
        let (mut session, _) = fixture();
        let first = key();
        let mut second = first;
        second.action.command_sequence = 2;
        let records = [first, second].map(|key| {
            let mut projectile = flight();
            projectile.key = key;
            event(
                key,
                PROJECTILE,
                ServerTick(1),
                PresentationEvent::Projectile(projectile),
            )
        });
        let changes = session
            .events
            .journal
            .reconcile(
                ServerTick(1),
                ReplayRange {
                    first: ServerTick(1),
                    last: ServerTick(1),
                },
                &records,
                |_| true,
            )
            .unwrap();
        session.events.consume(changes).unwrap();
        let mut runtime = receipt_runtime(session);
        let mut world = World::new();
        world.init_resource::<diagnostics::Telemetry>();
        let outcomes = [first, second].map(|key| ActionOutcome {
            sequence: key.action.command_sequence,
            key: action(key),
            execution_tick: ServerTick(1),
            data: ActionOutcomeData {
                status: TerminalStatus::Accepted,
                reason: OutcomeReason::Game(dreamwake_sim::combat::ActionReason::Accepted),
                query_tick: None,
                target: None,
                hit_region: 0,
                damage: 0.0,
                transaction: None,
                binding: None,
                spawn_bindings: BoundedVec::new(vec![(
                    0,
                    EntityId {
                        index: 100,
                        generation: key.action.command_sequence as u32,
                    },
                )])
                .unwrap(),
            },
        });
        // The removal checkpoint is delayed on the state lane. Both predicted
        // projectiles are still live when the next reliable binding arrives.
        for outcome in &outcomes {
            let session = runtime.prediction.as_ref().unwrap();
            let bytes = encode_control(
                &ServerControl::ActionOutcomes {
                    batch: outcome.sequence,
                    records: BoundedVec::new(vec![outcome.clone()]).unwrap(),
                },
                session.model.welcome.stream.epoch,
                0,
                session.model.welcome.frame_limit().unwrap(),
            )
            .unwrap();
            receive_message(
                &mut runtime,
                CONTROL_CHANNEL,
                &bytes,
                Duration::from_secs(1),
                &mut world,
            )
            .unwrap();
        }
        let session = runtime.prediction.as_mut().unwrap();
        assert_eq!(session.outcomes_ack_pending, Some(2));
        assert!(!session.events.represented(first));
        assert!(session.events.represented(second));
        for outcome in &outcomes {
            session.events.outcome(outcome).unwrap();
            assert!(
                session
                    .events
                    .suppress_public(outcome.data.spawn_bindings.as_slice()[0].1)
            );
        }
        // A delayed replay of the older checkpoint cannot recreate that visual.
        let changes = session
            .events
            .journal
            .reconcile(
                ServerTick(1),
                ReplayRange {
                    first: ServerTick(1),
                    last: ServerTick(1),
                },
                &records,
                |_| true,
            )
            .unwrap();
        assert!(changes.as_slice().is_empty());
        assert!(!session.events.represented(first));
        assert!(session.events.represented(second));
        assert_eq!((session.events.sounds, session.events.camera_cues), (0, 0));
    }
    #[test]
    fn replay_delivers_sound_camera_once_and_omission_cancels_projectile() {
        let mut events = adapter();
        let records = vec![
            event(
                key(),
                CAST_SOUND,
                ServerTick(1),
                PresentationEvent::CastSound,
            ),
            event(
                key(),
                CAST_CAMERA,
                ServerTick(1),
                PresentationEvent::CastCamera,
            ),
            event(
                key(),
                PROJECTILE,
                ServerTick(1),
                PresentationEvent::Projectile(flight()),
            ),
        ];
        for tick in 1..=4 {
            let changes = events
                .journal
                .reconcile(
                    ServerTick(tick),
                    ReplayRange {
                        first: ServerTick(1),
                        last: ServerTick(tick),
                    },
                    &records,
                    |_| true,
                )
                .unwrap();
            events.consume(changes).unwrap();
        }
        assert_eq!((events.sounds, events.camera_cues), (1, 1));
        assert!(events.represented(key()));
        let changes = events
            .journal
            .reconcile(
                ServerTick(5),
                ReplayRange {
                    first: ServerTick(1),
                    last: ServerTick(5),
                },
                &[],
                |_| true,
            )
            .unwrap();
        events.consume(changes).unwrap();
        assert!(!events.represented(key()));
        assert_eq!((events.sounds, events.camera_cues), (1, 1));
    }
    #[test]
    fn late_binding_of_expired_ghost_never_recreates_it_and_suppresses_public_duplicate() {
        let mut events = adapter();
        let record = event(
            key(),
            PROJECTILE,
            ServerTick(1),
            PresentationEvent::Projectile(flight()),
        );
        let changes = events
            .journal
            .reconcile(
                ServerTick(1),
                ReplayRange {
                    first: ServerTick(1),
                    last: ServerTick(1),
                },
                &[record.clone()],
                |_| true,
            )
            .unwrap();
        events.consume(changes).unwrap();
        let changes = events
            .journal
            .expire(ServerTick(2), record.identity)
            .unwrap();
        events.consume(changes).unwrap();
        let entity = EntityId {
            index: 100,
            generation: 1,
        };
        events.bind(key(), entity).unwrap();
        events.bind(key(), entity).unwrap();
        assert!(!events.represented(key()));
        assert!(events.suppress_public(entity));
        assert_eq!(events.sounds, 0);
    }
    #[test]
    fn rejection_and_scope_change_do_not_leave_prediction_or_replay_cues() {
        let mut events = adapter();
        let record = event(
            key(),
            PROJECTILE,
            ServerTick(1),
            PresentationEvent::Projectile(flight()),
        );
        let changes = events
            .journal
            .reconcile(
                ServerTick(1),
                ReplayRange {
                    first: ServerTick(1),
                    last: ServerTick(1),
                },
                &[record.clone()],
                |_| true,
            )
            .unwrap();
        events.consume(changes).unwrap();
        let changes = events
            .journal
            .reject_action(ServerTick(2), action(key()))
            .unwrap();
        events.consume(changes).unwrap();
        assert!(!events.represented(key()));
        assert!(
            events
                .bind(
                    key(),
                    EntityId {
                        index: 100,
                        generation: 1
                    }
                )
                .is_err()
        );
        let changes = events
            .journal
            .reset(JournalScope {
                connection: ConnectionEpoch(2),
                stream: CommandStream(1),
            })
            .unwrap();
        events.consume(changes).unwrap();
        assert!(!events.represented(key()));
    }
}

#[cfg(test)]
mod paused_object_tests {
    use super::*;
    #[test]
    fn paused_simulation_object_survives_4096_server_ticks_and_late_binding_stays_retired() {
        let key = SpawnKey {
            action: dreamwake_sim::combat::RayActionKey {
                match_epoch: 1,
                connection_epoch: 1,
                command_stream: 1,
                ownership_epoch: 1,
                actor: 1,
                actor_generation: 1,
                command_sequence: 1,
                action_slot: 0,
            },
            ordinal: 3,
        };
        let flight = StarfallFlight {
            key,
            origin_tick: 1,
            position: [0.0; 2],
            direction: [1.0, 0.0],
            radius: 0.34,
            remaining: 2.0,
            essence: None,
            authority_id: None,
        };
        let mut events = PredictedEvents::new(JournalScope {
            connection: ConnectionEpoch(1),
            stream: CommandStream(1),
        })
        .unwrap();
        let projectile = event(
            key,
            PROJECTILE,
            ServerTick(1),
            PresentationEvent::Projectile(flight),
        );
        let sound = event(key, CAST_SOUND, ServerTick(1), PresentationEvent::CastSound);
        let changes = events
            .journal
            .reconcile(
                ServerTick(1),
                ReplayRange {
                    first: ServerTick(1),
                    last: ServerTick(1),
                },
                &[projectile.clone(), sound],
                |_| true,
            )
            .unwrap();
        events.consume(changes).unwrap();
        for now in [256, 4097, 8193] {
            let changes = events
                .journal
                .reconcile(
                    ServerTick(now),
                    ReplayRange {
                        first: ServerTick(now - 255),
                        last: ServerTick(now),
                    },
                    if now == 256 {
                        std::slice::from_ref(&projectile)
                    } else {
                        &[]
                    },
                    |_| true,
                )
                .unwrap();
            events.consume(changes).unwrap();
            assert!(events.represented(key));
            assert_eq!(events.sounds, 1);
            assert!(matches!(
                events.journal.retire_through(CommandSeq(1)),
                Err(EventError::LiveHistory)
            ));
        }
        let changes = events
            .journal
            .expire(ServerTick(8194), projectile.identity)
            .unwrap();
        events.consume(changes).unwrap();
        events
            .bind(
                key,
                EntityId {
                    index: 99,
                    generation: 1,
                },
            )
            .unwrap();
        assert!(!events.represented(key));
        assert!(events.suppress_public(EntityId {
            index: 99,
            generation: 1
        }));
        events.journal.retire_through(CommandSeq(1)).unwrap();
        assert!(events.journal.retained_bytes() <= Limits::default().resident_bytes);
    }
}
