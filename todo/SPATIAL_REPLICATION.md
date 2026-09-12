# Fortnite / Unreal Replication Graph Spatial Foundation

This is the normative spatial-replication design for the portable library. It is part of the core architecture and the M1–M3 deliverables, not an optional M6 optimization. The foundation is **Epic's publicly documented Replication Graph model associated with Fortnite**. It is not a claim about the undisclosed implementation or configuration of the current Fortnite service. Unity Ghost relevancy/importance remains a supporting comparison, not the architecture to implement.

## 1. Evidence and adaptation boundaries

**Observed Epic pattern:** persistent nodes hold reusable actor lists; each connection gathers candidate lists from the graph rather than requiring every actor to evaluate every connection. Epic illustrates this with a historical Fortnite workload of 100 connections and roughly 50,000 replicated actors. That population is not an on-wire update count. [S09](ENGINEERING_SPEC.md#s09)

The API documents node preparation and per-connection gathering, separate static/dynamic/dormancy-driven spatial registration, global and per-connection actor information, connection-specific dormancy, and distance-dependent replication frequency. These are the named mechanisms to follow. [S51](ENGINEERING_SPEC.md#s51) [S52](ENGINEERING_SPEC.md#s52) [S55](ENGINEERING_SPEC.md#s55) [S56](ENGINEERING_SPEC.md#s56) [S57](ENGINEERING_SPEC.md#s57) [S61](ENGINEERING_SPEC.md#s61) [S62](ENGINEERING_SPEC.md#s62)

**Derived library contracts:** Rust types, sparse storage, the exact cell footprint algorithm, authorization masks, prediction-dependency closure, scope epochs, checkpoint receipts, recovery rules, numerical settings and tests below are this library's implementation choices. They are not copied Epic APIs or verified Fortnite internals. An Epic dependent-actor list is not evidence that Unreal guarantees this library's atomic rollback checkpoint semantics. [S61](ENGINEERING_SPEC.md#s61)

**Alternative, not an extra layer:** Epic documents Iris and Replication Graph as separate replication systems; a network driver uses one or the other. Do not describe the native Unreal integration as “Replication Graph plus Iris.” A portable implementation may borrow general principles, but must not claim wire compatibility or native interoperability. [S60](ENGINEERING_SPEC.md#s60)

| Epic reference | Portable library responsibility | Evidence boundary |
|---|---|---|
| `UReplicationGraphNode` [S51](ENGINEERING_SPEC.md#s51) | Persistent node storage; prepare once, gather per connection | Lifecycle pattern observed; Rust API derived |
| `GridSpatialization2D` [S52](ENGINEERING_SPEC.md#s52) | XY broad-phase lists with static, dynamic and dormancy-driven routing | Node modes observed; sparse influence-cell implementation below derived |
| `AlwaysRelevant` [S53](ENGINEERING_SPEC.md#s53) | Small globally eligible actor list | Never a blanket override of field authorization |
| `AlwaysRelevant_ForConnection` [S54](ENGINEERING_SPEC.md#s54) | Owner/view-target list and explicit per-connection extensions | Documented node includes controller/view target; arbitrary teams/items need application routing |
| `DormancyNode` / `ConnectionDormancyNode` [S55](ENGINEERING_SPEC.md#s55) [S56](ENGINEERING_SPEC.md#s56) | Shared dormant membership plus per-connection delivery state | Our ACK/version protocol is not Unreal's actor-channel protocol |
| `DynamicSpatialFrequency` [S57](ENGINEERING_SPEC.md#s57) | Optional distance-based due times inside eligible candidate sets | Frequency is scheduling, not permission or prediction completeness |
| Global / connection actor information [S61](ENGINEERING_SPEC.md#s61) [S62](ENGINEERING_SPEC.md#s62) | Separate shared entity metadata from connection-local tracking | Our entity IDs, revision fields and memory limits are derived |
| Game-specific nodes [S09](ENGINEERING_SPEC.md#s09) | Teams, rooms/portals, objectives, audible events, approved spectator views | Public extension pattern; actual game policies must be specified |

## 2. Three different relevance decisions

**Simulation relevance** decides which authoritative systems run. **Replication relevance** decides what a connection is eligible to learn. **Prediction relevance** decides what an admitted client predictor needs to reproduce its own gameplay. These are separate library concepts. Removing a client replica MUST NOT despawn or stop simulating the authoritative entity.

Replication has a mandatory two-stage distinction: **eligibility** determines permitted state; **scheduling** determines which permitted updates fit this frame. A gathered candidate is neither a disclosure authorization nor a promise of immediate transmission. Never use a low priority as a privacy filter.

Prediction requirements can bypass distance and optional update-frequency reduction, but not authorization. A permitted supporting platform outside the camera may be required. A hidden enemy with prohibited coordinates may not be included simply because it would improve collision prediction. Use an approved proxy, a server-authored interaction correction, or disable the affected prediction path. A proxy requires its own disclosure review; a renamed copy of the hidden transform is still a leak.

## 3. Persistent graph and state ownership

```text
Committed authoritative world + class routing + ticked change feed
                              |
                   ReplicationGraph.prepare(k)
                              |
        +---------------------+--------------------------+
        |                     |                          |
   Global actor lists    GridSpatialization2D       Connection nodes
   always relevant       static / dynamic /        owner / view target /
   approved objectives   dormancy-driven cells      team / semantic grants
        |                     |                          |
        +---------------------+--------------------------+
                              |
                 gather(connection, observers)
                              |
        bounded candidates + authorized dependency closure
                              |
       hard disclosure/field/scene checks + scope transitions
                              |
             atomic checkpoint units / due-time filtering
                              |
          priority + age + reserved budget + congestion limit
                              |
             serialize permitted state -> paced transport
```

Shared metadata contains entity/generation, route class, authoritative bounds, cull policy, dirty version, dormancy intent, dependencies and scene revision. Connection-local metadata contains observer grants, scope incarnation, disclosure/representation revision, latest decoded state version, retained baselines, lifecycle receipts and last/due transmission frames. Store connection-local records sparsely; do not allocate a full entity-by-connection matrix.

Maintain a reverse route index for removal and rerouting. Spawn, authoritative destruction, ownership/team changes, teleport, bounds/cull changes, attachment changes, scene migration and dormancy transitions generate tick-stamped changes. Apply them before gathering from the same committed world revision. Worker threads may gather concurrently from a frozen graph view; they cannot mutate routing while other connections iterate it.

Do not require every node to be purely event-driven. Epic exposes a once-per-frame prepare hook, and its per-connection always-relevant node documents a rebuilt view-dependent list. Updating all dynamic actor poses once per replication frame is an acceptable fallback when an engine lacks reliable dirty notifications; it is not an actors-times-connections scan. [S51](ENGINEERING_SPEC.md#s51) [S54](ENGINEERING_SPEC.md#s54)

The shipped core MUST include functional spatial and connection nodes, not only a user-supplied `is_relevant(entity, connection)` callback over the entire world. Custom nodes receive bounded candidate lists and documented change feeds. An opt-in brute-force oracle is allowed in tests and debugging only; production configuration must not silently fall back to world-wide broadcasting.

## 4. Class routing and first-release node set

| Route | Example | Required behavior |
|---|---|---|
| Not replicated | Server AI planner, private anti-cheat record | Never enters replication graph or client debug exports |
| Global | Public match phase or scoreboard summary | Small bounded list; only approved public fields |
| Owner-only | Private inventory, input receipts | Explicit authenticated connection route; no spatial fallback |
| Spatial static | Networked door or immobile pickup | Register influence cells; update only on explicit bounds/policy changes |
| Spatial dynamic | Player, moving projectile or vehicle | Refresh authoritative footprint once per graph frame |
| Spatial dormancy-driven | Destructible prop that may start moving | Static while dormant, dynamic while awake; versioned wake/flush |
| Dependent | Carried item or supporting platform proxy | Ticked parent/dependency relation; preserve field policy and checkpoint completeness |
| Semantic/custom | Team status, objective, authorized spectator | Application-defined shared or connection list; revoke explicitly |

Routing selects storage, not exclusive permission. An entity may have several eligibility reasons, but the gather accumulator deduplicates its identity. Removing one reason MUST NOT remove a still-valid owner/team/dependency reason. Do not conflate the carrying parent's update relationship with ownership of private item fields.

The Unreal BasicReplicationGraph is an example, not a complete solution: Epic documents limited class-based cull/always-relevant/owner-only support and restrictions on changing those values per actor. The portable implementation here must support explicit ticked runtime rerouting. [S59](ENGINEERING_SPEC.md#s59)

Static collision geometry that is already verified by map/content hash does not need a separately replicated entity for every triangle or prop. Replicate interactive state and authoritative topology revisions. This avoids making “50,000 actors” a target for needlessly fragmenting the game's state.

## 5. Spatial algorithm and observer admission

Use a sparse uniform XY grid as the first implementation, retaining exact Z/3D and room/portal tests after broad-phase gathering. This is a derived storage implementation of the selected 2D spatialization model. It is not a claim about Fortnite's cell dimensions or container type. For deeply vertical games, use layered or hierarchical custom nodes while retaining the graph/gather contract.

**Index actor influence, not only actor centers.** For entity `e`, conservatively cover cells intersecting its bounds expanded by its maximum permitted cull distance, leave hysteresis and configured prefetch allowance. A connection gathers the cell containing each server-approved observer, then performs exact policy checks on that bounded candidate set. This matches Epic's conceptual example of persistent lists for locations from which an actor can matter; the precise expansion below is our design. [S09](ENGINEERING_SPEC.md#s09)

```text
R_cover(e) = cull_radius(e) + bound_radius(e) + leave_margin + prefetch_cap
min_cell_x = floor((x(e) - R_cover(e) - grid_origin_x) / cell_size)
max_cell_x = floor((x(e) + R_cover(e) - grid_origin_x) / cell_size)
# Repeat for Y; cover this conservative rectangle.
# During gather, test exact distance/shape and authorization.
```

Use floor, not truncation, for negative coordinates. Cell coverage is a broad-phase superset: false-positive candidates are allowed; missing an eligible actor is not. Entities are updated whenever their **influence footprint** changes, even if their center remains in the same cell. Radius changes, speed-class changes, topology/room changes and teleports all invalidate the relevant cached data. Distance, reveal and field-policy decisions may change without any cell crossing, so cached candidate membership never replaces final checks.

A production registration must reject non-finite bounds and unreasonable radii before computing or allocating cell ranges. Bound cells per entity, total memberships and grid nodes. Route oversized legitimate objects to a configured coarse grid/custom large-object node, or reject the unsupported scene at admission. Do not put arbitrarily large objects in an unbounded always-relevant list and call that optimization. The first implementation may reject oversized registrations transactionally until the approved coarse node exists.

Observers come from the authoritative pawn, a bounded third-person camera envelope, or an explicit spectator/remote-camera grant. A client cannot send arbitrary observer coordinates and subscribe itself to the whole map. Validate identity, allowed offset, scene readiness, number of observers, update rate and entitlement. Deduplicate the union for split-screen or multiple allowed viewpoints. A spectator mode change revokes old visibility before new-scope publication. A delayed spectator requires data and policy at the spectator's permitted timeline, not unrestricted current state.

For ordinary spatial entry use the configured enter radius; for an already eligible entity use the larger leave radius. A bounded prefetch expansion can derive from maximum relative approach speed times a conservative delivery budget, plus a margin. Example proposal: 20 m/s × 0.25 s + 2 m = 7 m. This does not predict exact one-way delay or guarantee readiness. Teleports and very fast projectiles need explicit swept/event interest or activation gates, not an ever-larger global radius. Lookahead may not expand secret-state disclosure.

## 6. Gather transaction and dependency closure

For each connection and graph frame:

1. Read the frozen world/routing/observer-policy revision. Gather global, spatial and approved semantic lists. Include connection-local pending lifecycle/repair work, without scanning unrelated entities.
2. Deduplicate `(entity index, generation)` and retain reason bits. Apply cheap permission checks before expensive geometry or dependency work.
3. Expand the server-declared prediction dependency graph from admitted local prediction roots. Validate every target, dependency edge, allowed representation and budget. Use a visited set; cycles do not cause repeated work.
4. Apply the final entitlement, field visibility, scene and exact spatial checks. Required permitted dependencies bypass distance; prohibited dependencies generate a prediction-unavailable verdict rather than leaked payload.
5. Diff the resulting eligible set against connection state. Generate versioned entry, exit, representation-change, dormancy and repair work. Recheck policy immediately before enqueueing serialized data if workers can race a newer revocation barrier.
6. Form atomic checkpoint units with their dependency manifests. Only then compute due times, prioritize, fit to bandwidth and serialize. Advance lifecycle/baseline state on explicit receipts, not candidate selection.

The closure is all-or-nothing for a prediction group. Missing, unauthorized or over-budget dependencies prevent that group from being admitted to prediction. Never truncate a dependency list to fit a packet while reporting the group complete. An independent presentation replica may still be sent under its own permitted representation; prediction stays disabled until its complete checkpoint is available.

Keep delivery scope and replay retention separate. After dismounting a platform, current spatial delivery may end, but immutable historical samples still needed by buffered replay remain pinned within the history budget. Releasing a current network replica cannot invalidate a replay pointer. On privacy revocation, invalidate affected local prediction and use a safe correction path; the server cannot make an untrusted client forget data already delivered.

## 7. Protocol identity and interest lifecycle

Every per-connection entity representation has a monotonically increasing `scope_epoch`, separate from entity generation, ownership epoch and baseline generation. A scope epoch changes when an entity re-enters after exit or its permitted representation changes. Qualify state/delta/ACK identity by connection epoch, entity generation, scope epoch, representation revision, and the existing stream/group baseline identity. A prediction group also carries its group revision and each member's scope identity.

Visibility and delivery are distinct axes:

```text
Eligibility:  Absent -> Eligible -> Absent      (authoritative entity may remain alive)
Delivery:     Entering -> Active -> DormantKnown -> Active
                          |              |
                          +-> Leaving <--+
                                |
                            Forgotten
Authority:    Alive -------------------------> Destroyed
```

Dormant is not a synonym for out of interest. Dormancy suppresses repeated state work for a known unchanged actor. Interest exit retires that connection's current replica. Destruction is an authoritative gameplay fact. A server can retain bounded replay-only data after the visible replica is forgotten.

`ScopeEnter` includes scope identity, schema/representation, authoritative tick, current semantic state, scene revision and dependency requirements. Send a full baseline for every new scope incarnation; never rely on a previous incarnation's retained delta base. The client stages the full state, validates it, publishes the complete unit and acknowledges `ScopeReady`. Only that receipt makes this new baseline eligible for subsequent deltas. An entry is not merely a visual spawn instruction.

`ScopeExit` carries the retiring scope identity and a safe reason class. It removes that representation without playing gameplay death effects or creating an authoritative death tombstone. Retry or repair until acknowledged, or supersede it with a newer complete scope transition. A lost exit must not leave an indefinitely live local replica. Handle a later re-entry arriving before an old exit by discarding the old scope message, not deleting the new object.

An explicit authoritative `Destroy` ends the entity generation for clients entitled to that fact and rejects late state for that generation. Out-of-scope clients need not receive a revealing global death broadcast. Map-placed objects require current absence/destruction state when their region later becomes eligible so cached map defaults do not resurrect destroyed props. This can be a regional revision or approved tombstone representation, not a replay of every old event.

Control-lane order does not imply order relative to state datagrams. All transitions and chunks validate scope identities independently. Old ACKs cannot activate a new scope; old deltas cannot revive a retired replica; a newer representation cannot reuse a broader old field baseline. Connection reset invalidates all connection-local scope state.

## 8. Dormancy, flush, destruction and repair

Epic's dormancy documentation distinguishes dormant actors, which remain known locally, from dynamic actors losing relevance. It also describes waking/flushing before changing replicated properties and completing unacknowledged updates before becoming fully dormant. Its grid handles dormancy-driven actors as static while dormant and dynamic while awake. [S58](ENGINEERING_SPEC.md#s58)

Implement the equivalent library responsibility through `begin_replication_mutation(entity, tick)` or an atomic `mark_changed_and_publish` transaction. It increments the authoritative state version and makes the actor's changed state eligible for relevant connections. `flush_once` schedules the new version without promising perpetual awake status; `wake` resumes normal change/frequency processing. Every mutation that matters to clients must use this path, including subobjects and containers.

Shared dormancy intent does not prove delivery to any specific client. Track the last decoded state version per connection. Connection A can be dormant at version 7 while connection B still needs version 7 or its initial baseline. Retrying B's update must not cause A to receive all dormant state again. Re-entry still receives complete current state even when the server's entity is dormant.

Do not periodically broadcast all dormant actors. Maintain bounded pending-wake/dirty versions, scope repair requests and due timers. Retry changes until decoded or replaced by a newer equivalent complete state. If a full repair is needed, schedule it only for currently entitled connections. Dropped wake, last-state, destroy and exit messages all require repair paths.

The portable default retires a replica after ordinary spatial exit, including dormant replicas, subject to explicit cache/history policy. This is our lifecycle choice, not a claim that every Unreal configuration destroys out-of-range dormant actors. A hard entitlement revocation bypasses normal distance hysteresis and cancels unsent state for the revoked representation.

## 9. Frequency, priority and bounded bandwidth

Distance-dependent frequency is a scheduling option after eligibility. Implement it with per-connection last/due transmission ticks. The optional dynamic-frequency node may group candidates by due time; it must not become a second world-wide scan or hide a missing dependency. The documented Epic node derives update frequency from distance to the connection's view. [S57](ENGINEERING_SPEC.md#s57)

Prioritize lifecycle/repair and owner prediction checkpoints, then threats and interactive state, then lower-value remote presentation. Use capped age accumulation or a fair queue with deterministic tie-breaking. Under a feasible offered load, eligible lower-priority state must have a tested maximum update age. Under sustained overload, no algorithm can guarantee every deadline; expose violations, lower optional quality or stop admitting additional scope. “Required” is not unlimited bandwidth.

Reserve a configurable share of each connection's transport-derived budget for critical units. Unused reserve may serve ordinary state. Atomically required groups are either scheduled completely (possibly as bounded staged chunks) or not published. Staged chunk transfers need progress deadlines and cannot be perpetually restarted by every new snapshot. Bound queued entries separately from ordinary deltas to handle spawn storms and teleports.

A keyframe is paid for once its bytes are actually sent; retransmission, headers and chunks also count. Never count a selected actor as successfully updated before transmission or treat transmission as decode acknowledgement. Encoder reuse across connections is permitted only for an identical authorized representation and canonical state; baselines, ownership, private fields and scope envelopes remain connection-specific. Never reuse authenticated encrypted datagrams across peers.

## 10. Event, attachment and information filtering

Route effects and event audiences through the same authority policy, but do not equate event scope with entity scope. A player may hear an approved sound while not being entitled to its source's precise hidden transform. Send an authorized sound proxy or coarse event representation; do not include a hidden entity ID, target lock, direction or exact coordinates unless the rules permit it. An RPC addressed to a missing replica cannot be the only representation of persistent gameplay state.

A charging weapon, healing beam, opened door or looping effect must be reconstructible from its current semantic state on entry: phase, start tick/duration where permitted, attachment and cancellation state. This requirement applies even when the original start event occurred outside the connection's interest.

Resolve attachments by generation and scope-aware dependency identity. Unknown parents remain staged or use an explicitly allowed independent transform; never bind to the nearest entity. Owner-only inventory must not become visible to all observers of its carrier. Revoking a team reveal removes only that grant and reevaluates remaining legitimate reasons and allowed fields.

The spatial grid is not a line-of-sight or anti-wallhack solution by itself. A game that promises hidden-state protection must supply authoritative disclosure rules and test all serialized fields, events, debug views and derived proxies. Compare this requirement with Riot's published selective-disclosure work rather than attributing the exact policy to Fortnite. [S41](ENGINEERING_SPEC.md#s41)

## 11. Engine-neutral API and integration order

The following signatures express the proposed contract, not drop-in compiling Rust. The implementation must supply concrete bounded buffers, lifetimes and error types.

```rust
trait ReplicationGraph {
    fn apply_changes(&mut self, changes: &CommittedChangeBatch)
        -> Result<(), GraphError>;
    fn prepare(&mut self, frame: ReplicationFrame, world: &CommittedWorldView)
        -> Result<PreparedGraphRevision, GraphError>;
    fn gather(&self, connection: &AuthorizedConnectionView,
              revision: PreparedGraphRevision, out: &mut BoundedCandidateSet)
        -> Result<(), GatherError>;
}

trait DisclosurePolicy {
    fn allowed_representation(&self, connection: ConnectionId, entity: EntityId,
                              tick: ServerTick) -> Option<RepresentationGrant>;
}

// These are separate stages with separate budgets and observability.
fn close_prediction_dependencies(/* candidates, roots, grants, limits */)
    -> Result<EligibleReplicationSet, PredictionAdmissionError>;
fn reconcile_scope(/* eligible set, connection delivery state */)
    -> Result<ScopedReplicationWork, ScopeError>;
fn schedule_and_encode(/* scoped work, transport budget, baselines */)
    -> Result<OutgoingBatch, ReplicationError>;
```

The portable core owns node storage and gathering. Adapters publish committed transforms/bounds and route changes, not ad hoc prefiltered world snapshots that bypass policy. A native Unreal path may configure Replication Graph directly and implement missing game nodes. A portable-core Unreal path uses the portable graph and prevents native actor replication from sending a second uncontrolled copy of the same state. A Unity or Bevy adapter implements the same portable graph contracts; substituting another native relevance backend requires the same conformance suite and an explicit ADR.

## 12. Cost model, budgets and diagnostics

A defensible cost model is shared metadata/index maintenance plus candidate work:

```text
prepare_cost ~ changed routing + dynamic pose refresh + changed cell memberships
frame_cost   ~ prepare_cost + sum_connection(candidate visits + dependency edges
                + pending lifecycle work + scheduling + bytes encoded)
memory       ~ shared actors + cell memberships + sparse connection-known records
                + bounded baselines + staged scope/checkpoint state
```

It is not universally `O(changed entities)`. Dense fights where every player genuinely needs every other player still incur large per-connection work. Measure actual candidate visits, precise tests, node occupancy and transmitted counts, not just number of dirty entities. Spatial partitioning avoids irrelevant pair tests; it cannot remove required communication.

Proposed initial settings are a 32 m cell, 80 m base cull radius, 10 m leave margin and at most 8 m spatial prefetch. These are our test defaults, not Fortnite values. Class bounds/cull radii override only within configured coverage budgets. Start with at most 256 cells per indexed entity, two approved observers per connection, 4,096 gathered candidates and 128 required dependency entities per connection. Existing per-group and global prediction limits still apply. Validate budgets together and admit a smaller workload when required traffic cannot fit.

Instrument prepare/gather/filter/schedule/encode durations; cells and memberships changed; candidates by node; deduplicated and rejected counts; permission reason; known/entering/dormant/retiring scope counts; baseline resets; dependency pins and failures; selected versus transmitted bytes; critical deadline misses; and actual update-age percentiles. Keep server-only hidden-state detail out of ordinary client overlays.

Ship an interest inspector with authoritative observer cells, per-entity eligibility reasons, field grants, scope epochs, last decoded version and schedule decisions. A developer must be able to answer both “why did this client receive this?” and “why is this required platform missing?” without a packet capture.

## 13. Milestone and qualification gates

M0 approves the Replication Graph foundation, route table, disclosure policy, source boundaries and budgets. M1 implements persistent nodes, prepare/gather, the brute-force oracle and in-memory dependency-admission tests. M2 adds real per-connection scope entry/exit, full baselines, decoded-ready receipts and bounded lifecycle repair. M3 cannot pass until two separated clients receive different spatial entity sets, converge after loss/re-entry and retain complete authorized prediction dependencies. M4 adds advanced attachments/abilities to that existing path. M6 expands capacity and tunes compression/frequency; it does not introduce spatial interest for the first time.

Qualify sparse and dense layouts separately. The scale scenario is a proposed 100-connection / 50,000-potential-actor benchmark inspired by Epic's published motivating scale, not a claim of Fortnite parity. In the sparse case, unrelated distant actor growth must not cause per-connection world-wide visits. In the dense case, report saturation and degradation without discarding required groups or disclosing hidden state.

Test negative coordinates; coverage while the center stays in one cell; bounds/cull changes; multi-observer deduplication; authorization overrides; wake/flush loss; late joins to dormant entities; re-entry and stale epochs; leaving versus destruction; hidden dependencies; dependency cycles and capacity overflow; predictive-history retention; teleport/entry storms; scene revision changes; field-policy changes while deltas are queued; and spatial event audiences. The planning matrix contains production pass criteria. The executable model checks a deliberately narrower subset and is not a production graph implementation.
