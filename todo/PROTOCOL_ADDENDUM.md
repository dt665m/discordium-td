# Protocol Completion Contracts

This addendum supplies concrete implementation choices that complement the engineering specification. All wire identities and state machines are proposed library contracts, not undocumented claims about commercial games. Spatial routing follows the selected Epic Replication Graph foundation; see SPATIAL_REPLICATION.md. This revision adds scope identity and is not wire-compatible with an implementation that omitted it.

## 1. First connection sequence

```text
CLIENT                                        SERVER
Connect secure transport -------------------> validate service/match admission
AuthRequest(token, build, capabilities) ------> validate identity, expiry, replay policy
                         <------------------- AuthAccepted(connection_epoch)
                         <------------------- SessionConfig(protocol, tick epoch,
                                                ruleset/map/schema hashes,
                                                rates, spatial limits, ownership,
                                                approved observer grants)
ReadyForState(verified asset/hash set) -------> verify readiness, owner and observer grants
                                              prepare graph; gather permitted scopes
                                              close authorized prediction dependencies
                         <------------------- ScopeEnter + InitialCheckpoint(S[K], scope/group manifests,
                                                baseline generation, server time)
Decode to staging; validate entire scope
Publish complete state and retain baseline
ScopeReadyAck(scope, snapshot, generation) ----> mark this scope baseline decoded/usable
TimeProbe -----------------------------------> timestamp receive/send
                         <------------------- TimeReply + authoritative tick
Establish command lead, predict locally
InputBundle(U[K+1]...future targets) ---------> queue only eligible future ticks
                         <------------------- Snapshot + finalized ticks + outcomes
```

Commands for ticks already committed during synchronization are rejected rather than silently shifted. A newly connected player may be held non-interactive until an explicit future activation tick, giving both peers time to establish a complete state and usable lead. Activation is server-authoritative and part of the lifecycle journal. Synchronization never grants permission to burst a backlog of stale control samples into the world.

Time probes continue during normal play. Authentication is not repeated every tick, but connection epoch, ownership and protocol identity remain unambiguous for every application message. The client cannot begin controlling an entity merely because a render replica exists.

## 2. Reconciliation trace with a lost command

Assume both peers agree on `S[100]`, the client predicts through tick 108, and command 105 is lost until after its server deadline. The server executes real inputs for 101–104, substitutes input for 105, then executes real inputs for 106. It sends a complete checkpoint `S_server[106]`, a finalized-through watermark of 106, and an exception marking command 105 expired/substituted.

The client decodes and retains the checkpoint. It compares **predicted state at 106**, not predicted state at 108. It restores `S_server[106]` and replays exactly inputs 107 and 108 with their dependency histories. It does not replay 106, and it does not try to execute expired input 105 again. A fire action that existed only in command 105 receives an expired result and has its provisional effect canceled or retired.

The next datagram may redundantly contain command 105. That does not alter history: the server returns the same terminal late/expired decision. Commands 107–108 remain pending until their authoritative execution/checkpoint makes them obsolete. A transport ACK for the earlier datagram never replaces this logic.

## 3. Terminal action outcome state machine

Client-side action state is `Pending -> Accepted | Rejected | Expired`. An accepted result may separately be `AwaitingEntityBinding` for a spawn, but acceptance itself is terminal. The server must not send acceptance and later silently reinterpret the same request as rejection. A later game event can destroy an accepted projectile; that is a new authoritative lifecycle event, not a retroactive rejection of its original spawn request.

Use an outcome record containing action key, authoritative status, execution tick, reason class, optional entity binding, optional authoritative transaction ID and outcome sequence. Repeat until the client acknowledges outcome decoding, or include it in a reliable bounded control/result stream. Also retain enough outcome history for the declared reconnect window.

When a connection restarts, an old action key may be submitted to an **outcome lookup** endpoint, not re-executed as a fresh gameplay request. For durable non-gameplay operations, use a persistent transaction ID independent of transport epoch. Transport reconnection must never regenerate an irreversible operation under a new transaction identity.

A complete checkpoint can finalize all ticks through K without listing every cosmetic event, but pending local actions through K still need a terminal result or a deterministic, documented expiration rule. Do not infer whether a shot happened merely from current ammo: reloads or other resource changes can make that inference ambiguous.

## 4. Baseline retention and retirement fencing

A replication scope is qualified by connection epoch, entity generation, scope epoch and representation revision (and group revision/member scope manifest for grouped checkpoints). Each scope has a `baseline_generation`, a monotonically advancing snapshot ID, and a bounded allowed-baseline set. A decoded ACK promises that a named state is retained. The server records that promise before using the state as a base.

To retire a baseline, the client proposes retirement. The server first stops creating new deltas against that baseline, establishes at least one usable replacement or keyframe path, and acknowledges a retirement fence. The client may then discard the retired state according to the negotiated rule. Old datagrams referencing it can still arrive out of order after the reliable retirement ACK; therefore the client must recognize them as retired-generation/stale-base traffic rather than assuming cross-lane ordering.

A stale retired-base delta older than already published state may be discarded. A useful newer delta that cannot be decoded triggers a bounded keyframe request or waits for the already-scheduled replacement. It is never partially applied and never acknowledged as decoded. Limit repeated keyframe requests so one lost baseline does not create an amplification loop.

A scope re-entry or field-representation change requires full state under a new scope identity; no old-scope ACK authorizes a new delta. A baseline-generation reset invalidates all previous baselines in that scope and starts from a complete keyframe. Snapshot identity alone is insufficient across a generation or connection reset. Baseline cache size includes state and metadata; the client may refuse a configuration that exceeds its negotiated memory budget.

## 5. Resource-budget contract

The base specification defines several rates and caps. Add the following independent ceilings for the first implementation, then tune only with measurements:

| Resource | Proposed initial ceiling | Overflow behavior |
|---|---:|---|
| Client predicted groups | 8 | Refuse escalation or downgrade optional prediction |
| Entities in one prediction group | 32 | Require compact grouping or authoritative-only interaction |
| Total client predicted entities | 128 | Do not silently exceed global replay cost |
| Decoded checkpoint bytes per group | 16,384 | Reject oversized checkpoint/configuration |
| Aggregate client prediction-history memory | 64 MiB | Reduce admitted prediction scope or resynchronize; never unbounded allocation |
| Replay entity-steps per rendered frame | 2,048 | Cancel unpublished replay and use bounded recovery |
| Snapshot chunks per group publication | 16 | Reject malformed/excessive assembly; require another representation |
| Concurrent incomplete group assemblies | 8 | Evict oldest obsolete staging assembly, never a published baseline |
| Queued reliable application bytes per peer | 256 KiB | Backpressure low-priority work; disconnect persistently abusive peers |
| Future input slots per owner | Negotiated lead window, initially 12 | Reject outside the window |
| Action edges per command | 8 | Reject excessive input rather than executing partial arbitrary edges |
| Server-approved observers / connection | 2 | Reject excess or unapproved camera grants |
| Cells / indexed entity | 256 | Transactionally reject unsupported footprint or use an approved coarse node |
| Gathered candidates / connection | 4,096 | Signal overload; never silently omit required state |
| Required dependency entities / connection | 128 | Deny prediction admission before allocating excess work |
| Known scope records / connection | 8,192 | Bound cached/dormant/pending entries; admission or controlled reset on pressure |
| Pending scope transitions / connection | 1,024 | Coalesce obsolete incarnations; preserve required revocations; shed admission |
| Active grid cells / match | 65,536 | Reject scene/routing admission before allocation |
| Total grid memberships / match | 2,000,000 | Reject/partition unsupported workload; report occupancy |

These are independent limits. Passing a per-group cap does not permit eight groups to exceed the total entity-step or memory cap. Count replay operations across all groups in the frame. The wall-clock replay target remains a measured objective because one entity-step can have different cost depending on collision and abilities.

Checkpoint bytes, history metadata, event journals, tombstones, staged corrections and baseline storage must all be charged to their appropriate memory budgets. Reject an internally inconsistent configuration at startup. A 16 KiB group snapshot at 256 ticks costs 4 MiB before metadata; eight such groups cost 32 MiB, leaving headroom under the illustrative 64 MiB prediction-history ceiling. Replication baseline and diagnostic rings need separate explicitly bounded accounting.

Do not require every legal maximum-sized input bundle to fit after optional redundancy. Pack the newest complete input first, then as many older independently decodable records as fit within the actual transport payload budget. If even the newest legal input does not fit, the schema/configuration is invalid; do not silently truncate its action list.

## 6. Public API completion checklist

Expose engine-neutral operations for creating/destroying worlds; receiving bounded transport events; advancing the server to a monotonic deadline; capturing hardware input into a command journal; advancing prediction; applying decoded authoritative updates; extracting presentation data; querying action outcomes; registering graph routes; publishing committed bounds/routing/dormancy changes; granting/revoking observers; preparing/gathering the persistent graph; closing authorized dependencies; applying scope-ready/exit/repair receipts; changing ownership under server authority; requesting resynchronization; and collecting telemetry.

Every method specifies thread ownership, allocation behavior, error type and whether it can mutate the world. `apply_snapshot` is transactional. `render_extract` is read-only. `receive_transport_event` cannot directly mutate physics. `advance_server` owns the authoritative tick barrier. `advance_prediction` may perform bounded replay but does not emit irreversible external effects.

Gameplay registration requires a stable schema ID, canonical codec, default state, capture/restore behavior, dependency declaration and interpolation policy. Registering a field as predicted without providing restoration must fail at initialization. Dynamic component changes require an explicit lifecycle path; ordinary engine reflection is not sufficient.

A published adapter must supply executable examples for normal connection, two spatially separated clients, interest exit/re-entry, dropped dormancy wake, hidden-dependency rejection, prediction correction, rejected action, missing baseline, reconnect, spawn rejection, moving platform, historical hit query and graceful disconnect. API documentation must show the complete lifecycle, not only an attractive three-line connect example.

## 7. Scene revisions, large worlds and server AI

Use authoritative world-space or cell-plus-local coordinates in history. A floating-origin shift changes the rendering/working origin, not the historical identity of a location. Record the origin/cell revision with cached local-space query data, and transform query origins/targets into a common historical coordinate frame before testing. A shift must not appear as a teleport or invalidate shot distances accidentally.

For scene streaming, collision data has a content hash and revision. A server-approved entry boundary determines when an entity can interact with a region. Clients lacking the required collision revision cannot predict through it as empty space. Dynamic destruction is a ticked topology change represented in the historical blocker world.

Server AI is authoritative. Its decisions and random inputs must be included in replay logs or be reproducible from recorded initial conditions and ticked inputs. Clients normally interpolate AI actors or predict only a limited motion proxy. Do not run unsynchronized client AI and assume it is a trustworthy dependency for owner prediction.

## 8. Completion evidence

The production release report must attach a dependency/version manifest, source/license register, serialized protocol golden vectors, compatibility matrix, deterministic replay corpus, real-process networking logs, fuzz results, network-profile reports, hardware/CPU/bandwidth measurements, visual correction recordings, hit-policy boundary traces and a soak report.

The Python movement and spatial models in this handoff validate only their stated subsets. The spatial model is an in-memory contract demonstration, not a live replication backend or proof of the production scaling envelope. In particular, its simplified baseline store refuses unnegotiated eviction; the retirement state machine above still needs production implementation. Its command model is single-owner and does not replace the full multi-owner sequence/ownership admission policy. Its group assembler has one staged group and must be extended with bounded revision supersession and expiry. Do not mistake passing the model's tests for completing the release evidence list.

## 9. Scope transition and delivery contract

The normative ordering is `prepare graph -> gather candidates -> authorized dependency closure -> final field/scene policy -> scope diff -> atomic units -> due/priority/budget -> serialize`. No encoder takes an unrestricted entity list from an adapter. Every selected unit carries the world/policy revision it was authorized under; an entitlement revocation fences any unsent older-revision work.

Logical per-entity scope identity is `(connection_epoch, entity_index, entity_generation, scope_epoch, representation_revision)`. State adds `state_version`, `end_tick`, snapshot/baseline identity and optional group/member manifest. `scope_epoch` is server-issued, monotonically increasing within its entity/connection incarnation and does not wrap within an active session. A full group keyframe is required when any member scope identity changes; bump the group revision rather than mixing old and new members. Scope counters use a bounded serial/wrap contract or force a session reset before exhaustion.

| Record | Sender | Receiver behavior |
|---|---|---|
| `ScopeEnter(full state)` | Server | Stage authorized schema/current state; validate dependencies; publish atomically; return matching `ScopeReadyAck` |
| `ScopeReadyAck` | Client | Mark only the exact decoded entry baseline usable; ignore stale/unknown incarnations |
| `ScopedDelta` | Server | Require matching active scope, representation and retained baseline; otherwise discard or request bounded repair |
| `ScopeExit` | Server | Retire only the named incarnation; no authoritative death effect; acknowledge idempotently |
| `ScopeExitAck` | Client | Retire server delivery bookkeeping for that incarnation subject to in-flight/repair fencing |
| `StateVersionAck` | Client | Advance only the matching scope's known decoded version; enable per-connection dormancy completion |
| `ScopeRepairRequest` | Client | Validate entitlement and rate limit; return full currently permitted state or safe retirement |
| `Destroy` | Server | End that entity generation only for entitled audiences; do not leak a global hidden-entity death event |

**Reordering trace:** entity E enters at scope 12, leaves, then re-enters at scope 13. A delayed `ScopeExit(E,12)` cannot remove scope 13. A delayed delta or entry ACK for scope 12 cannot update scope 13 or establish its baseline. A new owner/field projection establishes a new representation and scope epoch; a cached broader payload is never reused. Unknown/new deltas without the full entry remain unapplied.

**Dormancy trace:** entity E has state version 7. Connection A decoded version 7; B has version 6. A may stop receiving unchanged state while B's version 7 remains pending. A dropped wake notification is repaired by versioned state retry. Later C enters for the first time and gets full version 7, not an assumption that map defaults or old events reconstruct it. Changing E to version 8 creates new work for every eligible connection independently.

**Revocation trace:** a team reveal or spectator grant expires while an old delta is queued. Remove the grant at the committed policy barrier, purge unsent prohibited fields, invalidate affected client prediction, and issue safe scope retirement or a narrower full representation. Do not promise to retract bytes that have already reached the peer or transport. Spatial hysteresis and required dependencies do not override this rule.

**Mandatory exit recovery:** repeated scope exits, scoped repairs, or a bounded current-scope manifest reconcile lost transitions. A connection-known scope may not live forever because its exit packet was lost. Tombstones and scope counters are retained only within bounded safe horizons; a controlled connection/scope-generation reset is preferable to unbounded per-entity history. Preserve separately budgeted immutable dependency samples while replay still requires them.
