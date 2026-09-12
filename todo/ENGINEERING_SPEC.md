# Server-Authoritative Prediction Library

## 1. Engineering decision

Build an **engine-independent, fixed-step simulation and reconciliation kernel with a first-class Fortnite/Unreal-style Replication Graph**, with engine adapters around it. The kernel must own tick identity, buffered commands, authoritative checkpoints, prediction history, entity lifecycle, persistent spatial/connection graph state, scoped replication and deterministic gameplay events. It must not be merely a component that predicts a transform and periodically lerps it toward the server.

The first production target is an authoritative dedicated-server action game: a 12-player arena, predicted kinematic characters, moving platforms, dashes, hitscan weapons, simple projectiles, server-owned health and rollback-safe ability presentation. Spatially selective per-connection delivery is mandatory in that first arena; a 100-player / 50,000-potential-actor workload is a separate scale qualification of the same graph architecture. These are **proposed qualification workloads**, not assumptions about a particular game or measurements from Overwatch or Fortnite.

Use Rust for the reference core and headless server, exposed through ordinary Rust library APIs. Rust is a recommendation, not a dependency of the architecture. A C++ or C# implementation is acceptable when it implements the same contracts and tests. Do not rewrite a viable existing networking stack merely to own more code.

**Spatial architecture decision:** use Epic's public Replication Graph pattern as the foundation: persistent shared nodes, 2D spatialization, explicit connection nodes, dormancy and per-connection gathering. [S09](#s09) The portable routing rules, prediction integration and scope protocol are derived library contracts. Unity relevancy/importance is supporting evidence, not an alternative default to select later. Section 4 defines the core and Section 17 specifies its implementation. M1 builds the graph; M2 connects scoped delivery; M3 verifies it with prediction.

There are two reasonable product paths:

| Path | Choose it when | Required work |
|---|---|---|
| Adopt an engine-native prediction stack | The game is committed to one engine and its supported simulation model | Integrate gameplay, add missing fairness/diagnostic/security features, and run this qualification suite. |
| Build the portable kernel specified here | The simulation must run independently of a rendering engine, or an independent library is the actual product | Own the shared simulation boundary, protocol, replay history, adapters, tooling and compatibility surface. |

**Quality claim:** this specification is a design and implementation handoff, not evidence that a completed product outperforms a commercial shooter. “As good as or better” becomes a release gate: demonstrate equal or better responsiveness, correction behavior, hit consistency and operational stability in stated workloads. No library can remove propagation delay, infer unknowable remote inputs, or make every conflicting player perspective simultaneously correct.

### Research boundaries

The evidence was reviewed on September 9, 2026. Public source, public engine documentation, developer explanations and proposed design are distinguished throughout. Moving repository branches were examined but are not reproducible version pins. Milestone M0 requires immutable commit IDs and dependency lockfiles before copying, integrating or benchmarking anything.

Valve's public Source SDK contains selected Source-era game code, not Counter-Strike 2's complete networking implementation. Blizzard's verified GDC abstract establishes ECS and determinism as architectural themes; the complete presentation transcript and proprietary implementation were not available for technical verification. Epic publishes extensive engine networking documentation, but that does not identify every mechanism or configuration used by the current Fortnite service. Unity package documentation is versioned, and samples must be matched to the installed package versions. [S01](#s01) [S07](#s07) [S08](#s08) [S23](#s23)

This is a comprehensive inventory of the **publicly supportable mechanisms needed for the requested library**, not a claim to enumerate undisclosed proprietary code. Suggested numeric defaults and algorithms below are original design choices unless explicitly attributed.

## 2. What the reference systems actually contribute

| Reference | Verified mechanism or design emphasis | What to adopt conceptually | What not to conclude |
|---|---|---|---|
| Valve Source SDK | Buffered user commands, shared movement, authoritative comparison and client replay; historical hitbox/animation rewind. [S02](#s02) [S03](#s03) [S04](#s04) [S05](#s05) | Commands rather than trusted positions; restoration and replay; explicit historical hit queries. | This is the current CS2 implementation, or the SDK is permissively reusable. |
| Counter-Strike 2 | Valve advertises sub-tick timing for movement, shooting and grenade actions. [S06](#s06) | Preserve action timing within simulation intervals when the gameplay and measurement justify it. | Simulation ticks, scheduling and replication cadence cease to matter; the complete implementation is published. |
| Overwatch, GDC 2017 | Official session describes ECS, gameplay complexity and determinism for responsive, precise networking. [S07](#s07) | Make gameplay state explicit and replay-friendly; architecture must accommodate abilities and attachments. | Exact current rewind limits, tick policies, every ability rule, or proprietary source have been verified here. |
| Unreal Character Movement | Saved moves, combining/retry, authoritative movement validation, corrections, replay and smoothing. [S08](#s08) | A complete movement prediction lifecycle, including custom state and movement bases. | Its ServerMove timing model is identical to a global fixed-tick command scheduler. |
| Unreal GAS | Prediction keys associate predicted effects/cues with authoritative outcomes and scoped prediction windows. [S11](#s11) | Stable action identity, rejection and deduplication of provisional effects. | GAS automatically rolls back arbitrary game logic, every periodic effect, or an entire physics world. |
| Fortnite / Unreal Replication Graph | Persistent shared nodes, spatial and connection lists, per-connection gathering. [S09](#s09) [S51](#s51) | **Primary spatial architecture:** implement persistent graph routing before the first networked prediction slice. | Exact current Fortnite settings, or automatic atomic rollback from a graph. |
| Unreal Iris | Alternative replication backend; not compatible with Replication Graph in the same native driver. [S60](#s60) | Supporting comparison only; native Unreal must select one backend. | Stack Iris under the Replication Graph plugin as an interchangeable extra stage. |
| Unreal network physics | Different replication modes trade correction behavior, history and resimulation cost. [S12](#s12) | Separate kinematic prediction from the substantially harder dynamic-rigidbody path. | Sending transforms and velocities is sufficient to restore every physics solver. |
| Unity Netcode for Entities | Shared predicted simulation, timing feedback, ticked commands and a documented partial-snapshot interaction hazard. [S16](#s16) [S17](#s17) [S18](#s18) [S19](#s19) | Explicit prediction groups, immutable input history and coherent restoration boundaries. | Every entity can be independently rolled back while freely interacting with entities at other times. |
| Unity Netcode for GameObjects | The reviewed documentation distinguishes client anticipation from full rollback/replay. [S22](#s22) [S50](#s50) | Anticipation is useful for presentation, but insufficient as the complete shooter predictor. | All Unity networking products provide interchangeable prediction semantics. |
| Lightyear | Public Rust/Bevy prediction, interpolation and tick-input infrastructure. [S27](#s27) [S28](#s28) [S29](#s29) | Evaluate existing implementation, tests and adapter boundaries before writing a new Bevy predictor. | Repository activity or feature lists prove commercial-shooter parity. |
| Mirror | Public prediction documentation discusses history correction around Unity physics. [S43](#s43) | Evaluate the exact physics and history behavior in a workload, not just API convenience. | A generic physics predictor automatically solves gameplay authority and fairness. |
| FishNet | Public feature documentation includes prediction; its license restricts competing networking solutions. [S44](#s44) [S45](#s45) | Capability comparison only in this handoff. | Publicly readable source grants permission to derive a competing networking library. |
| Quake III | Public historical source includes snapshot-based prediction and explicit state serialization. [S37](#s37) [S38](#s38) [S39](#s39) | Learn the separation of command history, predicted state and network representation. | Historical GPL code can be pasted into an unrelated proprietary library without review. |
| VALORANT | Developer articles explain latency budgeting and selective disclosure of enemy state. [S40](#s40) [S41](#s41) | Measure the entire latency path; integrate relevance with anti-information-leak rules. | Tick rate alone determines responsiveness or fairness. |
| Apex Legends | Developer explanation connects lag-compensation tradeoffs with server profiling and fleet telemetry. [S42](#s42) | Operational observability is part of netcode quality, not a post-release extra. | A historical article establishes current service configuration. |

### Source-reading order for engineers

Start with the spatial evidence map in Section 17.1 and the graph lifecycle in [S51](#s51), then inspect the grid and connection-node APIs in [S52](#s52) and [S54](#s54). Follow with the public command and movement structures in [S04](#s04) and [S05](#s05), then read the restoration/replay lifecycle in [S02](#s02). Read [S17](#s17) before designing interactions between predicted entities. Read [S08](#s08), [S11](#s11) and [S12](#s12) as three different systems, not one interchangeable Unreal feature. Evaluate [S27](#s27) [S28](#s28) [S29](#s29) for an existing Rust/Bevy foundation. Read [S03](#s03) for the historical rewind pattern, but implement the isolated query design in this specification rather than temporarily mutating the live server world. Finish with [S40](#s40) [S41](#s41) [S42](#s42) when defining player-facing fairness and operational tests.

Source-reading does not authorize code reuse. Track licenses, authorship, exact versions and implementation provenance independently.

## 3. Normative terms and non-negotiable invariants

**MUST** denotes a release requirement. **SHOULD** denotes the default unless an architecture decision record, tests and measurements justify a deviation. **MAY** denotes an optional capability. The distinction matters: optional sub-tick support must not delay core correctness.

Define `S[k]` as the complete relevant simulation state **at the end of tick k**. Input `U[k]` advances `S[k-1]` to `S[k]`. A checkpoint stamped `K` is therefore restored before replaying `K+1, K+2, …, P`. Tick zero is an initialized state, not a command that is executed twice.

| Identity | Meaning | Must not be confused with |
|---|---|---|
| `ServerTick` | Fixed-step authoritative world interval | Render frame, packet sequence or wall clock |
| `CommandSeq` | Monotonic identity of a client input record within an epoch | Proof the server executed the input |
| `TargetTick` | Server tick at which that immutable command is intended to execute | Packet arrival tick |
| `SnapshotId` | Identity of an encoded state publication | Its simulation tick or a transport ACK |
| `BaselineId` | Exact decoded, retained state used to reconstruct a delta | Any merely received packet |
| `EntityId` | Index plus generation, scoped to a match | Pool slot, pointer or engine object handle |
| `ConnectionEpoch` | Identity of the current authenticated connection incarnation | Account ID or reusable UDP endpoint |
| `OwnershipEpoch` | Ticked revision of the right to issue commands for an entity | Permanent ownership assumption |
| `ScopeEpoch` | Connection-local incarnation of an entity representation | Entity generation, ownership or baseline generation |
| `RepresentationRevision` | Revision of the permitted field/schema projection | Permission to reuse a broader previous field set |
| `StateVersion` | Authoritative version of replicated state, tracked per connection at decode | Selection for transmission or packet receipt |
| `ReplicationFrame` | Graph prepare/gather scheduling interval stamped with a committed world tick | Necessarily the same cadence as simulation or each entity's send rate |
| `PredictionKey` | Stable identity of one predicted gameplay action | The replay pass number |
| `PresentationTime` | Remote-world time represented by the current rendered view | Client's predicted local time |

The following invariants are mandatory:

1. The server owns authoritative positions, collision outcomes, health, inventory, damage, cooldown eligibility, entity lifecycle and match results. A client supplies requests and control intent, never final truth.
2. A player contributes at most one scheduled control sample per simulation tick and bounded action edges. No packet can buy extra simulation time.
3. Every state variable read by predicted logic is restored, deterministically derived, or supplied by an explicitly versioned dependency source. “It usually does not change” is not a classification.
4. A simulation function may execute repeatedly for the same tick. It must not directly play audio, send transactions, charge an account, update analytics, allocate an irreversible identity or read live input.
5. Simulation state is corrected immediately. Visual smoothing must not feed back into simulation or hit validation.
6. An acknowledged packet, a decoded snapshot and an executed command are three different facts. Represent them separately.
7. A delta is never applied to an absent or different baseline. A partial prediction group is never treated as a complete authoritative checkpoint.
8. Lifecycle, ownership and scene revisions participate in history. Reusing an index does not resurrect an old entity.
9. Every queue, history, message, entity set, action batch and replay pass has a finite budget and an explicit overflow behavior.
10. When information is missing, use a documented fallback or resynchronize. Do not silently invent historical state and present it as verified.
11. Per-connection state delivery goes through the persistent Replication Graph. Gather eligible state before scheduling or serializing it; no production broadcast-all fallback.
12. Prediction dependencies can override distance/frequency reduction but never a hard disclosure denial. An incomplete authorized dependency closure disables that prediction path rather than leaking or guessing state.
13. Leaving interest, being dormant and being authoritatively destroyed are distinct facts. New scope incarnations start from full baselines and reject old-scope messages.
14. Prepare shared routing/index data once per graph frame. Per-connection work visits gathered candidates and known/pending scope state, not every world entity.
15. All state, events, attachments and debug exports obey the same server-approved observer/field policy. Always-relevant is not always-authorized.

## 4. Core architecture and Fortnite-style replication graph

Use a workspace with five first-class responsibilities: authoritative simulation; prediction/reconciliation; persistent spatial replication graph; scoped replication/scheduling; and presentation. Package names below are proposed; responsibilities and dependency boundaries are normative.

| Package | Responsibilities | Must not depend on |
|---|---|---|
| `net_types` | Strong tick/ID types, scope/representation revisions, serial arithmetic, schema identity | Engine, network I/O, game rules |
| `net_clock` | Clock estimator, lead control, presentation clock, timing telemetry | Gameplay or rendering transforms |
| `sim_core` | Pure fixed-step gameplay and state classification contracts | Socket API, wall clock, graphics, OS randomness |
| `net_history` | Tick-indexed checkpoints, tombstones, lifecycle history, transactional restoration | A specific ECS |
| `net_prediction` | Input journal, replay planner, dependency declarations/groups, predicted events | Concrete transport or renderer |
| `net_interest` | Persistent Replication Graph, spatial and connection nodes, authoritative observer admission, eligibility and dependency inclusion | Client rendering camera as authority; sockets; concrete predictor |
| `net_replication` | Scope entry/exit, authorized field projection, dormancy delivery state, schemas, baselines and scheduling | Client input devices; world-wide per-connection scans |
| `net_protocol` | Bounded codecs, handshake, scope receipts, command receipts, action outcomes | Game presentation |
| `net_transport` | Existing secure transport adapters and packet pacing | Simulation authority decisions |
| `net_server` | Admission, tick scheduling, graph change publication, authoritative execution | Client frame loop |
| `net_lagcomp` | Immutable collision history and historical queries | Mutation of live simulation |
| `net_presentation` | Snapshot interpolation, replica lifecycle and error-offset policies | Authoritative damage logic |
| `net_trace` | Telemetry, interest/correction inspectors, replay, network emulator | Production-only services |
| `adapter_*` | Entity mapping, committed bounds/change feed, input capture, rendering, engine hooks | Other engine adapters |
| `arena_sample` | Headless server, spatially separated clients, graph test scenes and bot mode | Undocumented editor behavior |

### 4.1 Primary model and portable adaptations

Implement the public Epic node/gather pattern, not a generic distance callback invoked for every actor/client pair. Named engine reference points are `GridSpatialization2D`, `AlwaysRelevant`, `AlwaysRelevant_ForConnection`, `DormancyNode`, `ConnectionDormancyNode` and optional `DynamicSpatialFrequency`. [S52](#s52) [S53](#s53) [S54](#s54) [S55](#s55) [S56](#s56) [S57](#s57) Our scopes, buffers, permission masks and atomic dependency rules are derived contracts detailed in Section 17.

Class routing and committed change feeds populate persistent node lists. Shared metadata is prepared once; a connection gathers candidate lists from its approved observers and semantic grants. Hard field/disclosure rules then constrain dependency inclusion and the eligible set. Only that set reaches lifecycle handling, frequency/priority scheduling and serialization. Global or owner-critical state may bypass distance, not permission.

### 4.2 End-to-end data path

```text
Client input -> immutable commands -> local prediction -> render extraction
                    |                     ^                     |
                    v                     |                     v
              input datagrams       restore + replay        presentation
                    |                     ^
Server admission -> fixed simulation -> committed S[k]
                         |                |        |
                         v                |        +-> immutable hit history
                    action verdicts       v
                               graph change feed + prepare once
                                          |
               global nodes + spatial cells + connection nodes
                                          |
                           gather authorized candidates
                                          |
                     permitted prediction dependency closure
                                          |
                          scope enter/exit + field projection
                                          |
                       atomic units + priority + bandwidth
                                          |
                          baseline encoding -> state datagrams
```

### 4.3 Ownership and scheduling boundaries

Use dependency inversion for simulation, graph policy, transport and collision queries. Prediction emits engine-neutral dependency declarations; `net_interest` consumes those declarations, not the predictor implementation. `net_replication` consumes an opaque eligible set carrying policy/world revisions, not an unrestricted `World` plus client ID. An adapter cannot bypass scope checks by sending its own raw actor transforms.

Receive/decrypt/decode into bounded mailboxes. At a tick boundary, transfer a bounded batch into the simulation thread and run without awaiting arbitrary I/O. After commit, apply ticked graph changes, prepare the shared graph, then expose immutable views for worker gathering/encoding. Revocations fence unsent work before it reaches transport. Workers may not observe half-completed ticks or partially rerouted entities.

The same kernel must run with an in-memory emulator and real transport. The headless server must not instantiate a renderer to determine interest. Shared dynamic-pose refresh may visit each moving entity once per replication frame; connection gathering must not visit every unrelated actor. Dense mutually relevant populations remain inherently expensive and must be measured.

### 4.4 First-class delivery gate

M1 implements graph storage, routing and gather correctness alongside the offline simulator. M2 establishes per-connection scopes and full-entry state over real networking. M3 must show two separated clients with different received sets, complete authorized prediction dependencies, and recovery under exit/re-entry and packet loss. M6 is capacity tuning and compression, not the first implementation of spatial relevance. No broadcast-first implementation satisfies the vertical-slice gate.

## 5. Shared simulation contract

### 5.1 State classification

Every component and field requires one classification in the schema registry:

| Class | Examples | Storage and replication rule |
|---|---|---|
| Predicted authoritative | Movement position/velocity, stance, stamina, weapon cooldown, platform attachment | In authoritative checkpoints and replay history; complete dependency closure required |
| Server-only authoritative | Hidden opponents, secret RNG, damage adjudication, match admission | Never guessed as final truth by clients; send only allowed projections/results |
| Interpolated authoritative | Remote body poses, non-owned projectiles, cosmetic physics props | Snapshot history and presentation interpolation; not automatic replay inputs |
| Deterministic derived | Broadphase built from a versioned collision set, cached movement basis | Rebuild reproducibly or checkpoint it; document which |
| Presentation-only | Camera shake, particles, mesh error offset, local sound instance | Outside authoritative hashes and simulation; event reconciliation governs lifetime |
| External deterministic input | Ticked door change, server impulse, ownership transfer, scene revision | Journal by effective tick and revision; retain over the replay horizon |

A `PlayerState` checkpoint MUST include more than position: velocity, movement mode, grounded state, ground entity and generation, ground-local position/orientation, stance/capsule size, jump buffering/coyote state if present, acceleration modifiers, stun and dash state, relevant resources, weapon selection, cooldown timers, accepted action state, deterministic random counters, parent/attachment revision and teleport segment. Omitted fields are a common route to recurring correction loops.

### 5.2 Kernel API

The interface below is language-neutral pseudocode with Rust-like types, not a compiled crate:

```rust
trait Simulation {
    type State;
    type Command;
    type ExternalEvent;
    type Snapshot;

    // No I/O, no live input polling, no wall-clock reads.
    fn step(
        &self,
        state: &mut Self::State,
        tick: ServerTick,
        dt: FixedStep,
        commands: &CommandSet<Self::Command>,
        external: &[Self::ExternalEvent],
        dependencies: &DependencyView,
        events: &mut DeterministicEventBuffer,
    ) -> Result<(), SimFault>;

    fn capture(&self, state: &Self::State, group: GroupId)
        -> Result<Self::Snapshot, CaptureFault>;

    // Transactional: either a complete valid restore succeeds or state is unchanged.
    fn restore(&self, state: &mut Self::State, snapshot: &Self::Snapshot)
        -> Result<(), RestoreFault>;

    fn canonical_digest(&self, state: &Self::State, group: GroupId) -> Digest;
}
```

Represent `ReplayMode` in the event adapter or execution context, but do not use it to change movement, damage eligibility or other deterministic results. Mode may affect delivery of presentation side effects, not the state transition itself. Identical commands, dependencies and pre-state must produce identical post-state in normal execution and replay.

### 5.3 Determinism strategy

Prefer a small shared simulation package compiled into both client and server. Use explicit iteration order, stable entity IDs, stable collision candidate tie-breakers and deterministic random streams scoped to entities/actions. No iteration-order-dependent global RNG; adding a cosmetic entity must not change weapon spread.

Do not describe floating-point arithmetic as automatically deterministic or automatically unusable. Define a supported determinism domain: compiler, build flags, physics version, platforms and initialization rules. Rapier documents additional conditions for cross-platform determinism, including its enhanced-determinism feature and deterministic initialization. Those conditions do not make unrelated gameplay code deterministic. [S49](#s49)

For the initial reference movement controller, quantized integer/fixed-point state is attractive for reproducibility, while collision queries may use a tightly controlled floating-point implementation. If using floating-point predicted state, round-trip authoritative checkpoint encoding in the server's own replay test; a compressed correction must not create a different numerical recurrence than the client can reproduce.

A canonical digest MUST serialize explicit fields in fixed order, normalize representations and include schema/scene/dependency revisions. Never hash raw struct memory, padding, pointers or unordered maps. Diagnostics should report the first differing field, not merely a hash mismatch.

### 5.4 State ordering within a tick

Freeze and test this order: apply ticked world/lifecycle changes; install valid control samples; update movement/ability intent; perform movement and attachments; generate attack intents; build authoritative combat poses; validate attacks using the selected historical policy; resolve damage and status changes; commit lifecycle changes; capture end-of-tick state and history; extract network and presentation events.

Game design may choose a different order, such as pre-movement hitscan, but it MUST be written into the ruleset hash. A client cannot predict movement-before-fire while a server validates fire-before-movement and then call the resulting error random network jitter.

## 6. Clock synchronization and the three time domains

Maintain three distinct quantities on the client: an estimate of current server time, the predicted local simulation time, and the remote-world presentation time. Unity's documented timing system explicitly distinguishes prediction and interpolation ticks and uses server command-buffer feedback to adjust timing. [S18](#s18) The controller below is a proposed implementation, not a reproduction of Unity internals.

### 6.1 Initial synchronization

Use monotonic clocks. The client sends a probe at local time `t0`; the server records receive/send times `t1` and `t2`; the client receives at `t3`. An initial estimate is:

```text
round_trip = (t3 - t0) - (t2 - t1)
server_minus_client_offset = ((t1 - t0) + (t2 - t3)) / 2
estimated_server_now = local_now + offset
```

This estimates offset under a symmetry assumption; it does not measure true one-way latency. Collect a small window, reject invalid/negative samples, prefer low-delay samples for baseline offset, and separately estimate jitter and server processing delay. The handshake supplies the server tick epoch, fixed step, last committed tick, ruleset hash and starting authoritative checkpoint.

Represent time with integer durations and a rational tick rate. For 60 Hz, avoid repeatedly adding a rounded integer `16 ms`; derive deadlines from `epoch + tick * 1 second / 60`. Internally use at least 64-bit ticks. Wire truncation is optional and requires tested serial-number unwrapping.

### 6.2 Prediction lead

Schedule a new command for a future authoritative tick. Let `L` be lead in ticks:

```text
initial_L = ceil((estimated_uplink + jitter_allowance + send_queue_allowance
                  + server_scheduling_margin) / fixed_dt)
P ≈ estimated_committed_server_tick + L
```

Start the uplink estimate at half the measured RTT only as a bootstrap. The server reports the arrival slack of accepted commands: `target_tick - server_tick_at_arrival`. Use the observed slack distribution to tune lead so commands normally arrive just before their execution deadline. Report packet arrival before application-queue delay as well as tick-boundary admission; the two reveal different bottlenecks.

Default target queue slack is one tick; the initial lead is bounded to 1–12 ticks in the proposed arena profile. A 12-tick lead cap is not a promise to support arbitrary latency. When the network exceeds the supported envelope, expose a degraded-connection state and move to controlled resynchronization rather than silently trusting ever-older actions.

### 6.3 Clock servo and discontinuities

Use a slowly adjusted logical clock to change how frequently fixed steps are scheduled, with an initial slew range of 0.98–1.02 of real time. Keep simulation `dt` constant. Target lead changes do not authorize changing the target tick of commands already sent. Normally the client gradually gains or loses phase by scheduling fixed ticks slightly faster or slower, not by rewriting command identity.

A large offset change, suspended application or exhausted history triggers resynchronization: stop issuing new gameplay commands, request a current complete checkpoint, establish an explicit new command-stream incarnation, resolve outstanding predicted actions against server receipts, and resume with fresh lead. The new stream does not cancel server-committed damage or permit replaying an already accepted action under a new key.

### 6.4 Remote presentation clock

Estimate the age of the newest usable snapshot at render time, including downlink and server publication age. Add only the additional jitter-buffer slack needed to bracket interpolation samples. Define **total presentation age** as `estimated_server_now - render_time`; do not call an extra two-frame buffer the entire interpolation delay.

The presentation clock SHOULD advance monotonically during ordinary playback. Increase buffering quickly after repeated underruns and decrease it slowly after stability. If a discontinuity requires jumping, mark a discontinuity segment and use an explicit visual transition. Teleports must never be interpolated as ordinary fast movement.

## 7. Input generation, delivery and finalization

### 7.1 Command schema

Use a fixed-size core plus a bounded action list. The following is a logical schema; compression may change the byte representation but not semantics.

| Field | Proposed representation | Validation |
|---|---|---|
| Connection / stream epoch | Negotiated identity, implicit where transport permits | Must match the authenticated active stream |
| Owner entity and ownership revision | 64-bit entity ID, 32-bit revision | Connection controls entity at the target tick |
| Command sequence | 64-bit internal, optionally truncated on wire | Monotonic, deduplicated, bounded window |
| Target tick | 64-bit internal | One command per owner per target tick |
| Held controls | Bit mask | Only declared bits accepted |
| Move axes | Two signed 16-bit normalized values | Range checked and diagonal magnitude normalized |
| Aim | Quantized yaw/pitch or declared orientation representation | Finite, normalized, range checked |
| Action edges | At most eight per command in the sample profile | Unique action ID, legal type, bounded payload |
| Action fraction | Optional unsigned fraction within tick | Ordered and capped; zero for fixed-tick mode |
| View sample reference | Latest complete relevant snapshot ID and presentation tick/fraction | Untrusted hint, used only under lag-compensation policy |
| Snapshot decode ACKs | Recent applied IDs per relevant stream/group | Only ACK a state actually decoded and retained |

Input capture runs independently of simulation replay. Queue hardware edges with timestamps and drain each into exactly one command. Never poll a live keyboard during replay. Preserve press/release pairs that happen within one render frame. At low render FPS, multiple simulation ticks may be generated: do not multiply a mouse delta or repeat one button edge for each catch-up tick. A missing hardware sample requires a declared held-value interpolation policy, not invented additional clicks.

### 7.2 Redundancy and channel choice

Send the newest command plus a small tail of older commands in each datagram. Start with a four-command window and adapt within a bounded maximum of eight when loss measurements justify the extra traffic. Encode each bundle so it is independently decodable; compression within a bundle can use its full newest sample, not an assumed lost previous datagram. Unity's documented command stream uses a newest sample plus three older delta-compressed inputs, providing a useful concrete precedent. [S19](#s19)

Use unreliable freshness-oriented delivery for continuous commands. A reliable stream containing every input can spend time delivering obsolete movement. However, input loss is not permission to silently lose every important action. Each discrete action has a stable identity and a terminal outcome receipt. Resend the **same** request within its validity window until acknowledged; never create a new action identity because a packet was lost.

Define action delivery policy by type. A missed jump/fire edge expires with its target window rather than firing arbitrarily late. A non-time-critical inventory request can be reliably retried and execute in a later valid transaction. An automatic weapon may continue a held trigger only for the configured short missing-input grace. The policies are game rules and must be identical in the predictor and authority where applicable.

### 7.3 Server admission rules

Process each command through this exact order: authenticate stream; bounded decode; verify ownership; check numeric and enum ranges; check sequence and target windows; compare against an existing record; validate action identities; store immutable input for a not-yet-executed tick.

An exact duplicate is a no-op. The same `(epoch, sequence)` with different bytes is a protocol violation. A different command sequence attempting to replace an already admitted target tick is rejected. A command received after its execution tick is finalized is terminally rejected as late; the server does not rewrite past movement. Keep a short audit record so late duplicates receive a consistent response.

The server may validate a late combat request under a separately declared lag-compensation action path, but it must not let the same input secretly move the player in the past. The baseline implementation keeps movement and action scheduling simple: expired scheduled action edges are rejected. Enabling late action admission is an advanced ruleset change with separate origin, cooldown, ordering and exploit tests.

### 7.4 Missing input

At tick `k`, select the admitted command for that player and tick, or construct a **server substitute**. For at most three ticks, preserve permitted held movement; clear all one-shot edges. Release automatic-fire and other risky held controls after the configured grace. Then neutralize movement intent. Existing physical velocity still evolves according to the movement rules; neutral input is not an arbitrary position freeze.

Record the substitute explicitly. A late real input cannot replace it after the tick commits. Clients will receive an authoritative checkpoint reflecting the actual substitute and reconcile from that point. The server never waits for the worst connection before advancing all other players.

### 7.5 ACK taxonomy

Maintain separate fields and metrics:

* **Transport receipt:** ciphertext/datagram reached the transport. It says nothing about successful application decoding or simulation.
* **Decoded baseline receipt:** the client reconstructed and retained an exact authoritative snapshot. It can now serve as a delta baseline.
* **Finalized input receipt:** each target tick up to a contiguous watermark is committed, with exceptions identifying substituted/rejected commands as needed. Receiving sequence 100 does not imply execution of sequences 1–100.
* **Action outcome receipt:** accepted/rejected/expired, authoritative execution tick, resulting entity or transaction identity and reason code. The final result survives redundant delivery.

For dense one-command-per-tick streams, a finalized-through tick plus a bounded exception map is efficient. Still retain the command sequence/target mapping. A client discards commands only when a valid authoritative checkpoint and the finalized mapping make their effects obsolete. Receipt of a transport ACK alone never permits history deletion.

## 8. Authoritative server loop

The server is authoritative because it computes results, not because it rubber-stamps plausible client transforms. Use the following reference loop:

```text
for each fixed authoritative tick k:
    ingest bounded validated messages available at the admission cutoff
    apply ticked lifecycle, ownership and scene changes effective at k
    for every controlled entity:
        select immutable U[k], or construct and record substitute U*[k]
    step the shared simulation from S[k-1] to candidate S[k]
    build server combat poses and generate validated attack intents
    query historical collision state under the configured hit policy
    resolve authoritative damage/status/lifecycle transactions
    commit S[k] atomically
    capture history, complete owner prediction groups and replay inputs
    publish command finalization and action outcome receipts
    apply committed graph routing/bounds/dormancy/observer changes
    prepare persistent graph once for this replication frame when due
    for each connection selected by the replication-frame scheduler:
        gather node candidates; close permitted prediction dependencies
        filter disclosure/scene rules; reconcile scope entry/exit and dormancy
        build atomic units; schedule under priority/bandwidth/CPU limits
        encode only authorized scope state; pace transport output
    publish telemetry without blocking the simulation
```

Server-only events, such as an authoritative knockback caused by another player, are part of the next appropriate checkpoint and any replayable dependency journal. They are not injected unpredictably from another thread halfway through movement.

### Fair processing order

Do not reward the connection that happens to be first in a hash map. Use deterministic ordering for command ingestion and state mutation. For attacks with the same effective combat timestamp, generate intents from the same eligible state and batch resolution if the game's policy allows trades. A later attack must not be backdated to a time before an already-finalized death unless that behavior is explicitly part of the rules.

A fixed order is reproducible but can still be unfair; evaluate both determinism and player-facing tie policies. Randomizing player iteration each frame is not a replacement for a defined simultaneous-action model.

### Load management

Use bounded catch-up: the arena default permits up to four overdue fixed steps in one scheduling burst, without changing their `dt`. Prioritize simulation and authority over cosmetic replication, tracing detail and optional work. If the server cannot maintain the tick deadline, flag the instance unhealthy, stop admitting new matches and investigate or end/migrate the match under an explicit policy. Do not silently skip authoritative ticks, enlarge `dt`, or simulate unlimited catch-up until the host becomes unresponsive.

Keep account persistence, match analytics and external services out of the tick critical path. Emit idempotent committed events to an out-of-band service. An unavailable telemetry endpoint must not pause combat.

## 9. Client prediction and reconciliation

### 9.1 Normal prediction

Admit a prediction group only after its complete authorized scope/dependency checkpoint is decoded. Scope loss or a representation/scene revision that removes a required dependency invalidates admission at a defined tick. Keep replay-only dependency history independently from current visible replicas; never dereference a replica freed by spatial exit.

After initialization from a complete checkpoint, maintain a ring of commands, predicted checkpoints and dependency versions. Advance from the last predicted tick to the target predicted clock. For each tick, read only the stored command and the declared dependency view; execute the same shared gameplay step used by the server; capture predicted state and an event journal.

A predictor that stores commands but not the state needed for comparison and restoration is incomplete. A predictor that stores transforms but omits cooldowns, grounded flags or random-stream position is also incomplete.

### 9.2 Reconciliation algorithm

```text
on complete, decoded authoritative checkpoint A at end-of-tick K:
    validate epoch, schema, ownership, lifecycle and group revision
    reject stale or superseded correction versions
    retain A as an eligible replication baseline; send decoded ACK
    reconcile terminal action receipts separately from visual events
    if K is newer than the current predicted tick:
        enter controlled clock/prediction resynchronization
    else:
        locate predicted state and dependency manifest at K
        compare canonical predicted fields and dependency/input validity
        if there is no material difference and dependencies are identical:
            retain authoritative K; retire only safely finalized history
        else:
            validate availability of commands and dependency data K+1..P
            if required history is missing:
                request full group resync; do not fabricate replay state
            else:
                remember current displayed pose for presentation correction
                restore the complete group from A into staging state
                replay K+1..P using stored immutable commands/dependencies
                reconcile old and new deterministic event journals
                atomically publish corrected simulation state
                derive a visual-only correction offset
```

The fast path requires **more than a small position delta**. Equal positions with different velocity, stun state, cooldown, attachment or dependency versions can diverge on the next tick. Quantization tolerance may be used for presentation-only comparisons, but must not suppress meaningful state correction. Exact canonical equality is the safest initial fast path.

Take the newest complete applicable correction per group when several snapshots arrive together. Do not repeatedly replay every queued snapshot if newer state supersedes it. Preserve receipts and baseline records from older packets as necessary without applying stale simulation corrections.

### 9.3 External changes and partial knowledge

A checkpoint at `K` incorporates authoritative events through `K`. Replaying after `K` still requires known ticked dependencies after that time. If a corrected remote impulse or world revision is supplied for an intermediate tick, it invalidates dependent predicted history even when the owner's state at an earlier checkpoint looked equal.

Unknown future opponent inputs remain unknown. The library may extrapolate a collision proxy or withhold an interaction from prediction, but must record that policy. Do not describe a correction caused by a genuinely unknown enemy stun as a determinism bug. Diagnostics must distinguish expected new information from a replay inconsistency.

### 9.4 Replay CPU budget

Track replayed ticks, affected entities and collision queries per frame. Start with a 32-tick replay-work limit for the arena sample, separate from the 256-tick storage capacity. If a complete correction needs more work, use a controlled resync or a staged replay in an unpublished scratch world. Never expose a world partially replayed to different times as if it were ready for gameplay.

A staged replay must either converge on a declared publication tick before swapping or cancel in favor of a newer checkpoint. It must not fall indefinitely farther behind as fresh input arrives. The baseline implementation chooses resync rather than complex staged recovery; only optimize after measurement.

### 9.5 History invalidation

Invalidate by epoch, entity generation, ownership revision, prediction-group revision, scene revision and teleport segment. History entries distinguish `KnownValue`, `ExplicitlyAbsent` and `Unavailable`; a sparse missing entry does not prove a component was removed. Closed Lightyear reports illustrate why unchanged state and missing/removal distinctions deserve regression tests, without implying those reports are current unfixed defects. [S30](#s30) [S31](#s31)

## 10. Presentation, interpolation and correction smoothing

Use three separate representations where needed: authoritative/predicted gameplay state, collision proxy state and rendered state. Never attach the physics controller to a mesh transform that is being visually smoothed after a correction.

### Local player

Apply corrected simulation immediately. Preserve the old visual pose by computing a render-only position/orientation offset relative to the corrected pose. Decay the offset with a frame-rate-independent exponential or critically damped spring. A proposed default positional half-life is 50 ms; clamp very large errors and snap teleports/respawns. These values are tuning starts, not commercial-game measurements.

For a first-person game, separately choose camera and body policies. Camera yaw/pitch can respond immediately to local input while body movement stays ticked. Avoid smoothing the camera through solid walls or maintaining a large misleading camera offset from the authoritative collision capsule. Run a presentation collision constraint or snap when needed, but do not feed that adjustment back as a gameplay impulse.

A partial-frame local extrapolation MAY reduce fixed-tick visual latency. It must operate on a scratch copy and be discarded before the next real simulation tick. It must not consume RNG, emit an irreversible event, change the command journal or advance authoritative cooldowns.

### Remote entities

Buffer snapshots by authoritative tick, not packet arrival order. Choose two samples bracketing the remote presentation time. Interpolate position and orientation using the declared policy; clamp interpolation to avoid overshoot around abrupt changes. Velocity-aware Hermite interpolation can help smooth motion but needs bounded tangents and discontinuity detection. Quaternion interpolation must use the shortest valid arc.

If no future sample exists, bounded extrapolation is allowed for at most 100 ms in the initial profile, followed by a documented freeze/soft-stop/degraded behavior. Do not extrapolate across a known teleport, death, parent change or discontinuity. A late packet that precedes the presentation cursor may fill history but must not make playback run backward.

Snapshot interpolation is a separate technique from local prediction. Fiedler's original explanation provides the basic buffering model; the library still needs per-entity discontinuities, starvation handling and engine-specific presentation policies. [S46](#s46)

### Animation and effects

Drive remote animation from replicated simulation state and a presentation clock, not solely RPCs that say “play animation now.” Persistent states such as charging, healing beams and interaction progress need start tick, duration/phase, parameters and termination state, so an entity newly entering relevance can reconstruct them.

Keep gameplay collision poses independent of render-only animation blending. The server must produce the combat pose it actually uses. When cosmetic blending differs from authoritative hitboxes, measure and bound the discrepancy rather than assuming the art rig exactly matches the collision rig.

## 11. Kinematic movement, collision and moving bases

### 11.1 Baseline controller

The initial library sample uses a capsule-based kinematic controller with shared client/server collision code and a versioned static collision map. Implement acceleration, braking, gravity, jump, grounded motion, air control, crouch and dash as explicit state transitions. Each is a parameterized ruleset value, not hidden in a client animation script.

For every tick, compute intended displacement from velocity and `dt`; sweep the capsule against the declared collision view; move to a safe contact point with fixed skin width; project the remaining displacement onto the permitted contact plane; repeat up to a bounded contact-iteration limit; then perform deterministic ground and step tests. Resolve equal-time contacts with stable collider IDs. Handle initial overlap with a capped, deterministic depenetration policy. Do not teleport out along whichever collider happened to be returned first by an unordered query.

Step-up must have a precise sequence: test upward clearance; test forward clearance at the elevated position; sweep downward to a walkable surface; accept only within maximum step height and slope. Crouch-to-stand first tests the larger capsule; blocked standing stays crouched. Ground snap must be suppressed on the jump tick and across deliberate teleports. Slopes, ledges, thin walls and tunneling require dedicated maps in the test suite.

A shipped controller should use a proven collision library rather than a hastily written general geometry engine. The prediction library must nevertheless own its deterministic query contract and validate returned ordering, contact tolerances and cross-platform replay behavior.

### 11.2 Moving and rotating platforms

Store the platform entity/generation, attachment revision and ground-local transform in player history. Evaluate the platform's pose at the **simulation tick being replayed**, not its currently rendered pose. Apply base motion using the documented ordering relative to player input. On dismount, inherit the configured linear and angular contribution once. Parent change is a ticked discontinuity with preserved world-space state and explicit velocity conversion.

A platform following a deterministic trajectory can be reconstructed from versioned parameters and phase. A player-driven platform needs authoritative history or inclusion in the prediction group. A snapshot of the rider without the base's replay data is not a complete correction.

### 11.3 Remote-player collisions

Do not collide a predicted local player directly against delayed rendered opponent meshes. Options are: non-blocking player collisions; a separately extrapolated, bounded collision proxy; or coherent multi-entity prediction. The first is simplest, the second can be good enough for some action games, and the third is expensive and still cannot know future remote inputs.

Choose the policy by gameplay. A crowded arena with hard player blocking needs explicit correction testing. A cooperative action game may accept softer server-corrected overlap. Collision proxy extrapolation must never grant authority to the client's predicted position of the other player.

## 12. Prediction groups and dependency closure

A **prediction group** is an atomic correction unit. Every entity in it shares one end-of-tick checkpoint and a group revision. Dependencies outside the group are read-only, versioned sources during replay. Within a group, writes and interactions are permitted; across groups, mutation requires an explicit ticked event or a group merge before the interaction.

Unity documents the failure mode that motivates this: partially updated interacting predicted entities can be simulated at inconsistent times; grouping and scheduling policies are required to avoid that mismatch. [S17](#s17) The concrete group protocol below is an independent design.

### Group manifest

Each checkpoint contains `group_id`, `group_revision`, `tick`, member entity/generation IDs, component schema revisions, ownership revisions, scene revision and a dependency manifest. The owner character, active weapon, predicted attachment and deterministic ability state form the smallest useful group. Own simple projectiles can be included or tracked in a separate explicitly coupled group depending on their interactions.

No client may apply the character half of a character/platform checkpoint and immediately run collision against an old platform half. Decode into staging storage; validate all members and dependencies; publish atomically. If a group exceeds the datagram budget, chunk a bounded checkpoint at the application layer or request a compact complete representation. The entire group is valid only after all required chunks arrive and pass integrity/schema checks.

### Merging, splitting and interaction escalation

Group topology changes are ticked and versioned. For the first release, use conservative static groups plus explicit read-only world dependencies; this is easier to validate than fully dynamic prediction islands. Dynamic merge/split support comes after the base slice passes.

A merge at tick `m` requires a coherent authoritative state for all new members at `m`. Invalidate incompatible histories; establish a new group revision; replay only when every necessary command/dependency is available. A split similarly establishes new manifests and baselines. Temporary client disagreement with server group membership must not produce double authority or double event delivery.

The persistent Replication Graph consumes this dependency manifest before scheduling. Its closure either supplies every authorized dependency or returns an explicit prediction-admission failure; it may not trim mandatory members to a bandwidth target. Epic dependent actor lists motivate grouped delivery relationships, but atomic prediction groups are our additional contract. [S61](#s61)

Prediction relevance and network relevance are related but not identical. An invisible moving platform supporting the local player may be a required dependency. Conversely, a hidden enemy might intentionally be withheld for anti-cheat reasons. The latter cannot safely be part of a client prediction closure until disclosure is permitted; choose a boundary behavior rather than leaking secret state to improve replay accuracy.

## 13. Abilities, weapons and rollback-safe events

A movement predictor is not a complete action-game predictor. Ability state must be modeled as data: phase, start tick, remaining resource, cooldown end tick, charge amount, target or attachment reference, movement override and deterministic random state. The client may predict eligible local phases; the server still validates eligibility and commits outcomes.

### Action identity and effect journal

Define a prediction key from `(connection_epoch, command_stream, command_seq, action_slot)`. Predicted spawned objects add a deterministic spawn ordinal. The action slot is the input action's stable slot, not an index that changes when replay follows a different branch. A continuous effect is keyed by action identity plus semantic effect kind, such as `dash-trail`, not by an ever-incrementing global VFX counter.

The simulation emits deterministic event records. A separate delivery layer compares the previous and corrected event journals:

| Event status after replay | Presentation action |
|---|---|
| Same key, same semantics | Keep existing effect; do not play again |
| Same key, changed parameters | Update or reconcile the existing effect |
| Previously predicted, now absent/rejected | Cancel reversible effects; correct HUD; do not attempt to “unplay” already-heard sound |
| Newly valid event | Create effect once |
| Authoritatively confirmed event | Mark committed; bind authoritative entity/transaction ID |

Epic's prediction-key documentation demonstrates why accepted/rejected predictions and cue deduplication need explicit identity, while also documenting limits to its scoped prediction model. [S11](#s11) This library uses the broader journal above as its own contract, rather than promising arbitrary rollback through RPCs.

Do not predict irreversible external side effects. Damage numbers, hit markers, ammunition UI and cooldown display may be provisional, but inventory awards, ranked results and persistent purchases require authoritative commit. A predicted muzzle flash is not a confirmed hit marker. Distinguish them visually and in telemetry.

### Ability implementation pattern

For a dash: read a `DashPressed` edge; check predicted eligibility; set phase and start tick; consume predicted stamina; set movement override; emit `DashStarted(key)`. During replay the same key is reused. The server validates actual stamina and status. A rejection restores authoritative resource and movement state, replays subsequent commands, and cancels the provisional trail. A stun from another player can legitimately invalidate the dash; that is new authoritative information, not necessarily a broken predictor.

For a charged shot: begin a charge state from a stable action key; accumulate charge from tick duration; release with a distinct edge referring to the charge key; server clamps charge to actual eligible duration and resource history. A client cannot report “fully charged” without the server observing the corresponding state transition.

For automatic fire: a held trigger drives a server-owned firing schedule. Each shot has a stable identity derived from the trigger episode and shot ordinal, with a maximum fire rate enforced in simulation time. Missing input must not create unlimited firing, and replay must not produce duplicate shots.

### Root motion and animation-driven movement

Do not read movement displacement from a frame-dependent rendered animation and expect server replay to match. Bake or evaluate a deterministic movement curve indexed by ability phase and tick/sub-tick time; include curve version and phase in predicted state. Cosmetic animation follows that authoritative motion source. If using engine-native root motion, the adapter must capture all root-motion state and prove its resimulation behavior. Unreal's movement documentation addresses root-motion and movement-base integration; those engine-specific responsibilities cannot be replaced with generic transform replication. [S08](#s08)

## 14. Historical hit validation and fairness policy

### 14.1 Separate history from live authority

Maintain a read-only historical collision world on the server. For every relevant tick, record entity/generation, alive and damageable state, combat hitboxes, world transform, pose revision, discontinuity segment, team/filter state, shields and dynamic blockers needed by the game's policy. Broadphase history can be compacted, but exact candidate hit geometry must be reconstructable.

Never temporarily rewind the live world that movement, other attacks and worker threads are accessing. Valve's Source code provides a historical rewind/restore reference; this design instead exposes immutable historical queries to eliminate accidental live-state contamination. [S03](#s03)

Capture the server's actual combat pose, not the client's skeletal pose. A headless animation/collision rig must update even when no mesh is being rendered. Interpolate history only between compatible lifecycle and teleport segments. When a door, shield or character changes discontinuously, use event-boundary semantics, not a blend through an impossible intermediate state.

### 14.2 The timing contract

Define four times:

* `E`: estimated server time when the client sampled the action.
* `C`: scheduled authoritative execution time, approximately `E + prediction_lead`.
* `R`: server-world time the client used to render remote targets, approximately `E - total_presentation_age`.
* `A`: server-observed arrival time of the action request.

For the baseline mixed-time hitscan policy, use the authoritative shooter's eligible origin at `C` and historical target/blocker geometry at a validated `R`. This reflects the fact that local movement is predicted while remote players are displayed in the past. It is a deliberate gameplay policy, not a universally correct reconstruction of one simultaneous physical world.

Example: at estimated server time 1,000 ms, a client predicts 50 ms ahead and displays opponents at 925 ms. It schedules the shot at 1,050 ms and requests target history at 925 ms. The resulting rewind from execution time is 125 ms. **Do not subtract another half-RTT or full RTT from 925 ms**; network age is already represented in the chosen view time.

The actual implementation cannot trust the client's claimed `E`, `R` or camera. Validate against arrival history, the negotiated presentation-delay envelope, server-issued snapshot references, observed timing slack and a hard rewind cap. A decoded snapshot reference can constrain which state the client could know; it cannot prove what the player actually displayed or when they pressed a button.

### 14.3 Validation sequence

Authenticate and deduplicate the action. Verify ownership and lifecycle at execution. Check weapon state, ammo, cooldown, stance, range and legal aim representation. Resolve the firing origin from authoritative state; do not accept arbitrary client muzzle coordinates. Derive the allowed historical interval from server-observed timing and rules. Clamp the requested view time within that interval and the retained history horizon; record whether clamping occurred.

Query eligible historical target geometry and blockers. Resolve hits and damage under deterministic ordering. Return a terminal result containing action key, accepted/rejected status, effective execution/query times, target identity, hit region, damage transaction ID and a compact reason code. Do not send hidden geometry or unnecessary opponent information in rejection details.

For missing or invalid history, default to rejecting compensated validation with an explicit reason, not silently pretending a current-world test is equivalent. A game may choose a current-world fallback, but the outcome must be distinguishable in telemetry and tests. The initial arena profile uses a 150 ms maximum rewind and stores a larger history horizon for interpolation and diagnostics. The cap is a proposed fairness choice, not a published Overwatch setting.

### 14.4 Defense, cover and simultaneous actions

Specify a policy for each interaction, rather than advertising a single “favor shooter” switch:

| Interaction | Baseline policy | Alternative that requires a ruleset change |
|---|---|---|
| Ordinary target motion | Bounded historical target pose | Current-world target pose |
| Static wall | Always blocks in its unchanged world geometry | None unless game rules allow penetration |
| Moving door / destructible cover | Historical state at the validated query time | Additional current-cover veto, favoring defender |
| Invulnerability / shield activation | Explicit ticked defensive state under the selected combat timeline | Current-state veto or attacker-favor window |
| Teleport / respawn | Never blend across segments; no hit on the wrong lifecycle | A declared grace policy for a specific ability |
| Equal-time lethal attacks | Batch eligible intents and permit trades in the arena sample | Deterministic first-hit-wins policy |

The server must not let a client extend rewind by intentionally increasing its local interpolation buffer. Cap negotiated delay independently of client preference. High latency does not entitle a player to unlimited historical hits. Conversely, lowering rewind cap makes some apparently well-aimed high-latency shots fail; measure that tradeoff openly.

Riot and Respawn's developer explanations both emphasize latency/fairness tradeoffs. Their reasoning supports making these policies explicit, but does not establish a single universally optimal cap or defender/attacker rule. [S40](#s40) [S42](#s42)

### 14.5 Sub-tick support

Implement sub-tick timing only after ordinary fixed-tick replay and hit history pass all tests. Represent an action timestamp as `(tick, fraction)` with a bounded fraction and sorted, capped action list. In a sub-tick movement model, integrate deterministic segments whose durations sum exactly to one fixed tick. In a fixed-movement/sub-tick-hit model, preserve sub-tick aim and attack timing but document how shooter pose is interpolated or reconstructed between tick states.

A fraction does not grant extra movement time or more fire-rate opportunities. Duplicate fractions, reordered packets and excessive action density must not bypass resource/cooldown limits. Client and server must use identical event ordering, integration and rounding. Valve's CS2 description establishes sub-tick timing as a relevant feature, not proof of the exact policy to implement here. [S06](#s06)

### 14.6 Melee, beams and area attacks

Melee requires swept attack geometry or sampled authoritative attack volumes over the active interval, not just a single endpoint ray. Record hit-per-target suppression in authoritative ability state. Beams use ticked continuous queries and a stable effect episode; damage occurs on a server schedule, not once per render frame. Area attacks query authoritative eligible entities at the selected impact time and use generation-safe deduplication. Persistent damage volumes need ticked spawn/despawn and an explicit maximum catch-up cost.

## 15. Projectiles and predicted entity lifecycle

### 15.1 Spawn matching

A predicted projectile begins with a stable key `(epoch, command_seq, action_slot, spawn_ordinal)`, a declared prefab/schema ID and predicted initial state. The server validates the action and replies with the corresponding authoritative `EntityId`. Match by the key, not distance, spawn time or “the nearest projectile of this type.”

On acceptance, bind the predicted object to the authoritative identity without duplicating its visual instance. On rejection, cancel or fade it according to presentation policy and remove its predicted simulation state. When acceptance arrives after a local predicted collision/despawn, retain enough lifecycle history to bind or retire it correctly; otherwise a delayed spawn confirmation can resurrect a dead projectile.

### 15.2 Projectile time policy

The simplest correct baseline creates the authoritative projectile at scheduled execution time and simulates forward. The owner sees a predicted projectile immediately; authoritative corrections reconcile its flight. Remote observers see the replicated projectile at their presentation time. Local collision feedback remains provisional until the server confirms the impact.

A projectile is not a hitscan ray with delayed graphics. Full flight must account for obstacles and targets along time. Optional catch-up simulation may advance a newly admitted late projectile through a bounded historical interval, but must sweep each interval against the corresponding historical world and apply a declared defense policy. Do not teleport a bullet to a guessed present location and test only the final position. Bound catch-up steps, penetration count, ricochets and historical queries per action.

### 15.3 Lifecycle journal

The same entity generation can enter, leave and re-enter a connection's interest many times. Every re-entry establishes a new connection-local `scope_epoch` and complete baseline (Section 17.7). Interest removal must not invoke authoritative death hooks or erase still-required replay history. A stale exit cannot delete a newer replica; a stale scope ACK or delta cannot revive the older one.

Record spawn, despawn, ownership transfer, parent change, component add/remove and teleport as ticked lifecycle events. Use index-plus-generation IDs and never reuse a generation while its history or in-flight references can remain valid. Generation wrap needs a match/epoch reset or a sufficiently wide non-reuse policy.

Distinguish **not relevant to this client** from **destroyed in the authoritative world**. Leaving relevance can remove a render replica but must not create a global despawn tombstone. Re-entry sends a complete current state and persistent effect state, not merely a replay of old RPCs.

An object pool is an allocation optimization, not entity identity. Reset all pooled prediction, ownership, event and smoothing metadata before reuse. Predicted cancellation and authoritative despawn are idempotent. Repeated lifecycle delivery must not destroy a newer object occupying the same engine pool slot.

### 15.4 Reconnect and ownership handoff

A reconnect obtains a new authenticated connection epoch and a complete current state. Old datagrams cannot control the new session. Preserve server-committed action outcome records for the reconnect policy window; do not replay purchases, shots or spawns just because a client lost its previous transport.

Ownership transfers take effect at a declared tick and increment ownership revision. Stop accepting old-owner commands after the boundary, supply the new owner a complete prediction checkpoint, and reset incompatible local histories. For vehicles, use a single authoritative control arbitration rule for driver/passenger/turret roles. Host migration is not part of the baseline dedicated-server library; it requires authoritative checkpoint transfer, authentication continuity and split-brain prevention as a separate project.

## 16. Replication schema, compression and baselines

### 16.1 Schema registry

Every replicated field defines stable field ID, type, units, range, quantizer, default, authority class, visibility class, prediction class, interpolation policy, change-detection rule and compatibility version. Generate codecs and diagnostics from this registry where practical. Unknown mandatory fields reject negotiation; optional fields may be skipped only when length-delimited and semantically safe.

Use separate schemas for full simulation checkpoints and lossy presentation snapshots. A low-precision remote mesh position must not accidentally become a high-precision owner replay baseline. Owner authoritative state may use an exact canonical representation or a carefully proven quantized recurrence shared by both sides.

### 16.2 Quantization

Choose precision from gameplay error budgets. Proposed examples are millimeter-scale local movement state where feasible, bounded signed velocity, quantized angles and cell-relative positions for large worlds. The actual range and number of bits must be derived from maximum map extent and motion speed. Saturation is an error signal, not an acceptable silent clamp for authoritative positions.

Use explicit endianness and bit order. Define rounding ties, negative zero, invalid enum behavior and out-of-range handling. Rotation compression must reconstruct a normalized quaternion and test error near boundary cases. Delta encoding, sparse changed-field masks and entity relevance should be implemented before exotic compression. Fiedler's compression and state-synchronization articles are useful original references for the technique families; this specification's sizes and policies remain proposed. [S47](#s47) [S48](#s48)

### 16.3 Baseline protocol

A server may encode a delta only against a baseline that the destination client has **decoded and promised to retain**. Identify the baseline by connection epoch, stream/group revision, member entity/generation and scope/representation revisions, baseline generation and snapshot ID. A representation or scope-incarnation change invalidates the prior baseline association and requires full current authorized state. Include target tick and baseline ID in every delta. On the client, decode to staging state, validate all fields and group completeness, then publish and acknowledge.

If the baseline is absent, reject the delta without modifying current state and request a keyframe. Do not guess a nearby baseline, apply only the fields that happen to parse, or ACK the failed decode. A keyframe is a complete state for its declared scope, not necessarily the entire match.

A bounded retention contract is necessary: the server may use only a small negotiated set of recent ACKed baselines; the client retains that set until retirement is acknowledged or a baseline-generation reset occurs. A client eviction request must not immediately discard a baseline the server still uses. Timeout and reset paths must remain bounded under a malicious or disconnected peer.

### 16.4 Packet loss and scheduling

Continuous snapshots are freshness-oriented and may supersede older unsent snapshots. Lifecycle and action results have idempotent state-repair or reliable delivery semantics. Do not make all state reliable merely to simplify one loss bug; stale reliable data can consume bandwidth that should carry current state.

The graph first determines eligible authorized scopes; the scheduler never makes unauthorized entities eligible. Use frequent complete owner-group checkpoints or deltas with robust baseline recovery. Lower-priority remote entities can update less frequently. On baseline reset or relevance entry, send a keyframe. Periodic state refresh repairs missed state transitions even when change detection believes nothing new happened.

For oversized state, split by independently meaningful replication groups whenever possible. A large atomic group may use bounded application-level chunks with snapshot ID, group revision, chunk index/count and total-size cap. Incomplete chunks expire without publication. A newer complete keyframe can supersede an old incomplete one. Avoid relying on IP fragmentation.

### 16.5 Source versus proposed scaling

Epic's Replication Graph documentation uses Fortnite's 100-client, approximately 50,000-replicated-actor example to motivate persistent replication structures. That is evidence for the architectural scaling problem, not a claim that every actor is transmitted to every client each tick. [S09](#s09)

The library must decouple simulation population, potential replicated population, per-client relevant population, changed population and actually transmitted population. Treating those as one number leads to misleading capacity claims.

## 17. Fortnite / Unreal Replication Graph spatial foundation

This is the normative spatial-replication design for the portable library. It is part of the core architecture and the M1–M3 deliverables, not an optional M6 optimization. The foundation is **Epic's publicly documented Replication Graph model associated with Fortnite**. It is not a claim about the undisclosed implementation or configuration of the current Fortnite service. Unity Ghost relevancy/importance remains a supporting comparison, not the architecture to implement.

### 17.1. Evidence and adaptation boundaries

**Observed Epic pattern:** persistent nodes hold reusable actor lists; each connection gathers candidate lists from the graph rather than requiring every actor to evaluate every connection. Epic illustrates this with a historical Fortnite workload of 100 connections and roughly 50,000 replicated actors. That population is not an on-wire update count. [S09](#s09)

The API documents node preparation and per-connection gathering, separate static/dynamic/dormancy-driven spatial registration, global and per-connection actor information, connection-specific dormancy, and distance-dependent replication frequency. These are the named mechanisms to follow. [S51](#s51) [S52](#s52) [S55](#s55) [S56](#s56) [S57](#s57) [S61](#s61) [S62](#s62)

**Derived library contracts:** Rust types, sparse storage, the exact cell footprint algorithm, authorization masks, prediction-dependency closure, scope epochs, checkpoint receipts, recovery rules, numerical settings and tests below are this library's implementation choices. They are not copied Epic APIs or verified Fortnite internals. An Epic dependent-actor list is not evidence that Unreal guarantees this library's atomic rollback checkpoint semantics. [S61](#s61)

**Alternative, not an extra layer:** Epic documents Iris and Replication Graph as separate replication systems; a network driver uses one or the other. Do not describe the native Unreal integration as “Replication Graph plus Iris.” A portable implementation may borrow general principles, but must not claim wire compatibility or native interoperability. [S60](#s60)

| Epic reference | Portable library responsibility | Evidence boundary |
|---|---|---|
| `UReplicationGraphNode` [S51](#s51) | Persistent node storage; prepare once, gather per connection | Lifecycle pattern observed; Rust API derived |
| `GridSpatialization2D` [S52](#s52) | XY broad-phase lists with static, dynamic and dormancy-driven routing | Node modes observed; sparse influence-cell implementation below derived |
| `AlwaysRelevant` [S53](#s53) | Small globally eligible actor list | Never a blanket override of field authorization |
| `AlwaysRelevant_ForConnection` [S54](#s54) | Owner/view-target list and explicit per-connection extensions | Documented node includes controller/view target; arbitrary teams/items need application routing |
| `DormancyNode` / `ConnectionDormancyNode` [S55](#s55) [S56](#s56) | Shared dormant membership plus per-connection delivery state | Our ACK/version protocol is not Unreal's actor-channel protocol |
| `DynamicSpatialFrequency` [S57](#s57) | Optional distance-based due times inside eligible candidate sets | Frequency is scheduling, not permission or prediction completeness |
| Global / connection actor information [S61](#s61) [S62](#s62) | Separate shared entity metadata from connection-local tracking | Our entity IDs, revision fields and memory limits are derived |
| Game-specific nodes [S09](#s09) | Teams, rooms/portals, objectives, audible events, approved spectator views | Public extension pattern; actual game policies must be specified |

### 17.2. Three different relevance decisions

**Simulation relevance** decides which authoritative systems run. **Replication relevance** decides what a connection is eligible to learn. **Prediction relevance** decides what an admitted client predictor needs to reproduce its own gameplay. These are separate library concepts. Removing a client replica MUST NOT despawn or stop simulating the authoritative entity.

Replication has a mandatory two-stage distinction: **eligibility** determines permitted state; **scheduling** determines which permitted updates fit this frame. A gathered candidate is neither a disclosure authorization nor a promise of immediate transmission. Never use a low priority as a privacy filter.

Prediction requirements can bypass distance and optional update-frequency reduction, but not authorization. A permitted supporting platform outside the camera may be required. A hidden enemy with prohibited coordinates may not be included simply because it would improve collision prediction. Use an approved proxy, a server-authored interaction correction, or disable the affected prediction path. A proxy requires its own disclosure review; a renamed copy of the hidden transform is still a leak.

### 17.3. Persistent graph and state ownership

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

Do not require every node to be purely event-driven. Epic exposes a once-per-frame prepare hook, and its per-connection always-relevant node documents a rebuilt view-dependent list. Updating all dynamic actor poses once per replication frame is an acceptable fallback when an engine lacks reliable dirty notifications; it is not an actors-times-connections scan. [S51](#s51) [S54](#s54)

The shipped core MUST include functional spatial and connection nodes, not only a user-supplied `is_relevant(entity, connection)` callback over the entire world. Custom nodes receive bounded candidate lists and documented change feeds. An opt-in brute-force oracle is allowed in tests and debugging only; production configuration must not silently fall back to world-wide broadcasting.

### 17.4. Class routing and first-release node set

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

The Unreal BasicReplicationGraph is an example, not a complete solution: Epic documents limited class-based cull/always-relevant/owner-only support and restrictions on changing those values per actor. The portable implementation here must support explicit ticked runtime rerouting. [S59](#s59)

Static collision geometry that is already verified by map/content hash does not need a separately replicated entity for every triangle or prop. Replicate interactive state and authoritative topology revisions. This avoids making “50,000 actors” a target for needlessly fragmenting the game's state.

### 17.5. Spatial algorithm and observer admission

Use a sparse uniform XY grid as the first implementation, retaining exact Z/3D and room/portal tests after broad-phase gathering. This is a derived storage implementation of the selected 2D spatialization model. It is not a claim about Fortnite's cell dimensions or container type. For deeply vertical games, use layered or hierarchical custom nodes while retaining the graph/gather contract.

**Index actor influence, not only actor centers.** For entity `e`, conservatively cover cells intersecting its bounds expanded by its maximum permitted cull distance, leave hysteresis and configured prefetch allowance. A connection gathers the cell containing each server-approved observer, then performs exact policy checks on that bounded candidate set. This matches Epic's conceptual example of persistent lists for locations from which an actor can matter; the precise expansion below is our design. [S09](#s09)

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

### 17.6. Gather transaction and dependency closure

For each connection and graph frame:

1. Read the frozen world/routing/observer-policy revision. Gather global, spatial and approved semantic lists. Include connection-local pending lifecycle/repair work, without scanning unrelated entities.
2. Deduplicate `(entity index, generation)` and retain reason bits. Apply cheap permission checks before expensive geometry or dependency work.
3. Expand the server-declared prediction dependency graph from admitted local prediction roots. Validate every target, dependency edge, allowed representation and budget. Use a visited set; cycles do not cause repeated work.
4. Apply the final entitlement, field visibility, scene and exact spatial checks. Required permitted dependencies bypass distance; prohibited dependencies generate a prediction-unavailable verdict rather than leaked payload.
5. Diff the resulting eligible set against connection state. Generate versioned entry, exit, representation-change, dormancy and repair work. Recheck policy immediately before enqueueing serialized data if workers can race a newer revocation barrier.
6. Form atomic checkpoint units with their dependency manifests. Only then compute due times, prioritize, fit to bandwidth and serialize. Advance lifecycle/baseline state on explicit receipts, not candidate selection.

The closure is all-or-nothing for a prediction group. Missing, unauthorized or over-budget dependencies prevent that group from being admitted to prediction. Never truncate a dependency list to fit a packet while reporting the group complete. An independent presentation replica may still be sent under its own permitted representation; prediction stays disabled until its complete checkpoint is available.

Keep delivery scope and replay retention separate. After dismounting a platform, current spatial delivery may end, but immutable historical samples still needed by buffered replay remain pinned within the history budget. Releasing a current network replica cannot invalidate a replay pointer. On privacy revocation, invalidate affected local prediction and use a safe correction path; the server cannot make an untrusted client forget data already delivered.

### 17.7. Protocol identity and interest lifecycle

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

### 17.8. Dormancy, flush, destruction and repair

Epic's dormancy documentation distinguishes dormant actors, which remain known locally, from dynamic actors losing relevance. It also describes waking/flushing before changing replicated properties and completing unacknowledged updates before becoming fully dormant. Its grid handles dormancy-driven actors as static while dormant and dynamic while awake. [S58](#s58)

Implement the equivalent library responsibility through `begin_replication_mutation(entity, tick)` or an atomic `mark_changed_and_publish` transaction. It increments the authoritative state version and makes the actor's changed state eligible for relevant connections. `flush_once` schedules the new version without promising perpetual awake status; `wake` resumes normal change/frequency processing. Every mutation that matters to clients must use this path, including subobjects and containers.

Shared dormancy intent does not prove delivery to any specific client. Track the last decoded state version per connection. Connection A can be dormant at version 7 while connection B still needs version 7 or its initial baseline. Retrying B's update must not cause A to receive all dormant state again. Re-entry still receives complete current state even when the server's entity is dormant.

Do not periodically broadcast all dormant actors. Maintain bounded pending-wake/dirty versions, scope repair requests and due timers. Retry changes until decoded or replaced by a newer equivalent complete state. If a full repair is needed, schedule it only for currently entitled connections. Dropped wake, last-state, destroy and exit messages all require repair paths.

The portable default retires a replica after ordinary spatial exit, including dormant replicas, subject to explicit cache/history policy. This is our lifecycle choice, not a claim that every Unreal configuration destroys out-of-range dormant actors. A hard entitlement revocation bypasses normal distance hysteresis and cancels unsent state for the revoked representation.

### 17.9. Frequency, priority and bounded bandwidth

Distance-dependent frequency is a scheduling option after eligibility. Implement it with per-connection last/due transmission ticks. The optional dynamic-frequency node may group candidates by due time; it must not become a second world-wide scan or hide a missing dependency. The documented Epic node derives update frequency from distance to the connection's view. [S57](#s57)

Prioritize lifecycle/repair and owner prediction checkpoints, then threats and interactive state, then lower-value remote presentation. Use capped age accumulation or a fair queue with deterministic tie-breaking. Under a feasible offered load, eligible lower-priority state must have a tested maximum update age. Under sustained overload, no algorithm can guarantee every deadline; expose violations, lower optional quality or stop admitting additional scope. “Required” is not unlimited bandwidth.

Reserve a configurable share of each connection's transport-derived budget for critical units. Unused reserve may serve ordinary state. Atomically required groups are either scheduled completely (possibly as bounded staged chunks) or not published. Staged chunk transfers need progress deadlines and cannot be perpetually restarted by every new snapshot. Bound queued entries separately from ordinary deltas to handle spawn storms and teleports.

A keyframe is paid for once its bytes are actually sent; retransmission, headers and chunks also count. Never count a selected actor as successfully updated before transmission or treat transmission as decode acknowledgement. Encoder reuse across connections is permitted only for an identical authorized representation and canonical state; baselines, ownership, private fields and scope envelopes remain connection-specific. Never reuse authenticated encrypted datagrams across peers.

### 17.10. Event, attachment and information filtering

Route effects and event audiences through the same authority policy, but do not equate event scope with entity scope. A player may hear an approved sound while not being entitled to its source's precise hidden transform. Send an authorized sound proxy or coarse event representation; do not include a hidden entity ID, target lock, direction or exact coordinates unless the rules permit it. An RPC addressed to a missing replica cannot be the only representation of persistent gameplay state.

A charging weapon, healing beam, opened door or looping effect must be reconstructible from its current semantic state on entry: phase, start tick/duration where permitted, attachment and cancellation state. This requirement applies even when the original start event occurred outside the connection's interest.

Resolve attachments by generation and scope-aware dependency identity. Unknown parents remain staged or use an explicitly allowed independent transform; never bind to the nearest entity. Owner-only inventory must not become visible to all observers of its carrier. Revoking a team reveal removes only that grant and reevaluates remaining legitimate reasons and allowed fields.

The spatial grid is not a line-of-sight or anti-wallhack solution by itself. A game that promises hidden-state protection must supply authoritative disclosure rules and test all serialized fields, events, debug views and derived proxies. Compare this requirement with Riot's published selective-disclosure work rather than attributing the exact policy to Fortnite. [S41](#s41)

### 17.11. Engine-neutral API and integration order

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

### 17.12. Cost model, budgets and diagnostics

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

### 17.13. Milestone and qualification gates

M0 approves the Replication Graph foundation, route table, disclosure policy, source boundaries and budgets. M1 implements persistent nodes, prepare/gather, the brute-force oracle and in-memory dependency-admission tests. M2 adds real per-connection scope entry/exit, full baselines, decoded-ready receipts and bounded lifecycle repair. M3 cannot pass until two separated clients receive different spatial entity sets, converge after loss/re-entry and retain complete authorized prediction dependencies. M4 adds advanced attachments/abilities to that existing path. M6 expands capacity and tunes compression/frequency; it does not introduce spatial interest for the first time.

Qualify sparse and dense layouts separately. The scale scenario is a proposed 100-connection / 50,000-potential-actor benchmark inspired by Epic's published motivating scale, not a claim of Fortnite parity. In the sparse case, unrelated distant actor growth must not cause per-connection world-wide visits. In the dense case, report saturation and degradation without discarding required groups or disclosing hidden state.

Test negative coordinates; coverage while the center stays in one cell; bounds/cull changes; multi-observer deduplication; authorization overrides; wake/flush loss; late joins to dormant entities; re-entry and stale epochs; leaving versus destruction; hidden dependencies; dependency cycles and capacity overflow; predictive-history retention; teleport/entry storms; scene revision changes; field-policy changes while deltas are queued; and spatial event audiences. The planning matrix contains production pass criteria. The executable model checks a deliberately narrower subset and is not a production graph implementation.

## 18. Transport, protocol and security

### 18.1 Transport selection

Reuse a maintained secure transport; do not invent encryption, key exchange or congestion control. The portable Rust reference candidate is Quinn/QUIC with DATAGRAM support. GameNetworkingSockets is a native alternative, and Renet is another networking building block to evaluate. None of these alone supplies the gameplay prediction contracts in this document. [S32](#s32) [S33](#s33) [S34](#s34) [S35](#s35) [S36](#s36)

QUIC DATAGRAMs are unreliable and congestion-controlled, while reliable streams have their own ordering semantics. Avoiding reliable-stream head-of-line blocking does not eliminate shared congestion or bandwidth competition. [S35](#s35) Keep large reliable asset downloads off the match connection or in explicitly budgeted lower-priority delivery. Benchmark transport behavior under loss rather than assuming any protocol label guarantees low latency.

For browsers, provide a separately validated WebTransport/WASM path or another supported browser transport. A WebSocket fallback has different loss/ordering behavior and must not be presented as equivalent competitive performance without measurement. NAT traversal, relays, platform SDKs and hosted relay entitlements are integration decisions; standalone library access does not imply access to a provider's hosted services.

### 18.2 Protocol channels

| Logical lane | Semantics | Examples |
|---|---|---|
| Control | Reliable, bounded, versioned; independent of bulk data | Authentication, observer grants, readiness, ruleset, ownership, scope exit/repair, disconnect reason |
| Input | Unreliable redundant, target-tick and identity checked | Continuous controls and time-critical edges |
| State | Unreliable latest-useful, scope- and baseline-aware | Full scope-entry checkpoints, owner groups, remote entity snapshots |
| Outcomes | Idempotent repeated-until-ack or reliable bounded lane | Spawn binding, action rejection, damage/result confirmation |
| Bulk | Rate-limited separate service/stream where possible | Maps, cosmetics, replay downloads |
| Diagnostics | Optional, lossy or low priority | Trace samples and debug overlays |

Every lane has queue capacity, per-peer rate cap, message-size cap and overflow policy. Dropping an obsolete state snapshot is acceptable; dropping an authoritative commit and forgetting it is not.

### 18.3 Application frame layout

Use a documented binary frame inside the selected transport, not an unspecified engine object serializer:

```text
ApplicationFrame:
    protocol_major:u16 | protocol_minor:u16
    stream_kind:u8 | flags:u8
    application_epoch:u32
    frame_sequence:u32
    payload_length:u16
    payload[payload_length]

InputBundle:
    owner_entity:u64 | ownership_revision:u32 | command_stream:u32
    newest_command_seq:u64 | newest_target_tick:u64
    command_count:u8 | baseline_ack_count:u8
    bounded independently decodable command records
    bounded decoded-baseline acknowledgements

SnapshotGroup:
    snapshot_id:u64 | group_id:u32 | group_revision:u32
    end_tick:u64 | baseline_id:u64 (zero means keyframe)
    schema_revision:u32 | scene_revision:u32
    finalized_through_tick:u64
    bounded finalization exceptions / action receipts
    member manifest: entity_generation, scope_epoch, representation_revision
    authorized field data and dependency manifest

ScopeControl:
    connection_epoch | entity_generation | scope_epoch
    kind: Enter | ReadyAck | Exit | ExitAck | RepairRequest
    representation_revision | state_version | effective_tick
    bounded full-entry/checkpoint reference or safe reason
```

This is a **reference logical wire contract**, not a requirement to transmit every field redundantly in every packet. Protocol major/minor and epoch may be connection-scoped after negotiation, but the decoder must know their values unambiguously. Lengths and counts are checked before allocation. Compression formats require a strict decoded-size limit.

`frame_sequence:u32` uses tested serial arithmetic within a half-range window; long-running sessions rotate or unwrap to an internal 64-bit counter. Never compare wrapping unsigned wire sequences using ordinary `>` without a declared window. Transport encryption nonces and anti-replay are handled by the transport; application frame sequence is not a substitute for cryptographic security.

Budget the **entire datagram**, including transport/IP overhead. Start conservatively around a 1,200-byte transport-safe envelope and query the transport's actual datagram payload limit; a 1,200-byte plaintext application message is not automatically a 1,200-byte wire packet. Reserve framing space and fail safely when a path's negotiated limit is smaller.

### 18.4 Connection state machine

Implement `Disconnected -> Connecting -> Authenticated -> Synchronizing -> Ready -> Playing -> Resynchronizing/Closing`. During synchronization, verify build/protocol compatibility, ruleset and map hashes, ownership grants, tick epoch and complete initial state. Server-approved observers must also have an initialized graph scope and complete required dependencies. Only then enable local simulation and input issuance. A client must not send normal gameplay actions while merely downloading its first checkpoint.

Bind authentication tokens to account, match, expiry and intended service. Reject replayed/expired credentials. Disable or gate replay-sensitive gameplay mutations during any replayable transport early-data phase. Apply anti-amplification and rate limits before sending large snapshots to unauthenticated peers. Do not allow a spoofed address to trigger a multi-megabyte baseline response.

### 18.5 Threat model

Assume the client controls its clock, input packets, executable, displayed interpolation buffer and local assets. The server checks ownership, tick budget, action frequency, finite numeric values, world bounds, ammo/resources, movement rules, cooldowns and generation-safe references. Never accept client-declared damage, hits or final transforms as authoritative.

Bound future command queues, historical hit requests, action counts, ray penetration, projectile catch-up, compression expansion and trace upload. A malicious client's work must be bounded before expensive collision or deserialization. Isolate per-client fault handling so invalid input does not panic the match process.

Server authority does not prevent an aimbot from producing plausible legal aim inputs. Withholding unnecessary information, rate/behavior analysis and platform anti-cheat can complement this library, but a clean server-authoritative movement loop is not a complete anti-cheat product. Debug builds must not disclose hidden entities in production telemetry channels. Observer grants, custom graph nodes, field projections, dependency proxies and event audiences are security-sensitive API surfaces, not convenience client subscriptions.

## 19. Engine integrations and API ownership

This implementation uses the Rust/Bevy integration. The Unity and Unreal discussions below are architectural references, not additional adapter deliverables.

### Rust / Bevy

Evaluate Lightyear before building a competing full stack. Its public repository and prediction module entry points make it a relevant implementation candidate. [S27](#s27) [S28](#s28) [S29](#s29) Pin a compatible Bevy/Lightyear pair, run the arena qualification scenes, and document missing requirements instead of deciding from feature names.

For a new portable kernel, Bevy should provide input capture, entity mapping, render extraction and scheduling adapters. The kernel owns authoritative/predicted state. Register deterministic systems in an explicit fixed order; map `EntityId` to engine entities with generations; apply lifecycle changes through a controlled boundary. Do not allow both Lightyear and a custom predictor to write the same transform or maintain competing rollback histories.

A game can reuse transport or replication packages without adopting every part of a framework, but the chosen version's public interfaces and license determine whether that split is maintainable. A private-module fork is a maintenance commitment, not a free abstraction.

### Unity

Choose one of two integration models. For a Unity-native project, prefer a validated Netcode for Entities stack when its ECS workflow fits, using official predicted-character and netcode samples as starting references. [S23](#s23) [S24](#s24) For a portable core, Unity renders and captures input while the shared kernel performs predicted simulation through a native/WASM-compatible interface.

Do not run Unity physics and a separate portable physics kernel as competing authorities for the same character. The Unity adapter may render a GameObject whose pose comes from the core, but its Rigidbody/CharacterController must not independently advance that gameplay state. Conversely, when Unity physics is the authoritative shared simulation, restoration must include the exact engine state required by that integration.

The reviewed NGO anticipation API is not a drop-in implementation of a full command replay predictor. [S22](#s22) [S50](#s50) It may still serve presentation needs, but do not place it over the portable predictor so that two smoothing or anticipation systems repeatedly correct one another.

Unity's official samples use the Unity Companion License for Unity-dependent projects. Treat them as Unity integration references, not unrestricted source for an engine-independent library. [S25](#s25) [S26](#s26)

### Unreal

For native replication, configure the selected Replication Graph path and game-specific routes. Iris is a separate backend, not a compatible layer beneath the graph; changing to Iris is an explicit architecture exception with its own conformance tests. [S60](#s60) For the portable kernel, the first-class spatial core remains `net_interest`, with Unreal providing committed entity bounds and route changes rather than a second replication authority.

For an Unreal-only character game, extend Character Movement's saved state and authoritative validation rather than casually replacing its movement loop. Custom movement mode, resource and root-motion state must be represented in the relevant saved/corrected state. GAS prediction keys should align with the game's action identities and outcome lifecycle, not become a second incompatible action namespace. [S08](#s08) [S11](#s11)

For a portable kernel, disable the corresponding engine-owned movement replication and let the adapter expose the kernel's predicted and rendered transforms. Unreal animation, visual smoothing and collision callbacks must respect the authority boundary. Engine source access is subject to Epic's licensing/account process; this handoff does not claim to have audited unpublished Fortnite code. [S15](#s15)

NetworkPrediction and NetworkPredictionExtras provide additional engine API/model entry points worth evaluating in the pinned engine checkout. Public API documentation establishes their existence, not their exact suitability for every game or a promise of stable integration across versions. [S13](#s13) [S14](#s14)

## 20. Diagnostics, replays and developer tooling

The library is not ready for a team until a gameplay engineer can explain a correction without packet-capture archaeology. Ship the following tools as part of the vertical slice.

**Timeline overlay:** show current server estimate, predicted tick, latest authoritative tick, presentation tick, command arrival slack, input substitution rate, snapshot age, RTT/jitter estimates, packet loss, queued bytes, correction magnitude, replay ticks and group sizes. Separate transport RTT from total input-to-confirmation latency.

**Interest inspector:** display approved observer cells, candidate versus eligible versus sent counts, per-entity reason bits, field grants, scope epoch, dormancy version and bandwidth deferral. Explain missing mandatory dependencies and stale-scope rejection. Server-side private state must not appear in ordinary client overlays. This tool is an M3 deliverable.

**Correction inspector:** select a correction and compare authoritative versus predicted state at the same tick, field by field. Show command sequence/target, dependency revisions, movement base, lifecycle state, first divergence and replay cost. Classify causes: unknown external event, different command, missing dependency, numerical divergence, lifecycle mismatch, codec mismatch or timing error.

**Shot inspector:** reconstruct authoritative origin, requested and clamped query time, target historical geometry, blocker state, defense policy and verdict. Keep one action transaction trace through prediction, arrival, execution, replication and presentation. Restrict access and redact hidden gameplay details from ordinary clients.

**Deterministic replay:** record build/schema/ruleset/map hashes, starting checkpoint, accepted/substituted commands, external events, lifecycle changes and all nondeterministic authoritative inputs. Replaying only client inputs is insufficient if server AI, random seeds or world events differ. Checkpoint periodically for seeking. A replay from a different build must reject or use an explicit migration path, not silently produce a misleading result.

**Network emulator:** implement latency, asymmetric delay, jitter, independent loss, burst loss, reordering, duplication, bandwidth caps, queue limits and disconnect/reconnect. Distinguish a delay queue from a bandwidth serializer: simply sleeping each packet does not model congestion or queue growth.

**Production correlation:** include match/instance ID, build ID and privacy-safe connection correlation IDs in logs. Sample expensive traces after anomalous events; keep a bounded pre-event ring for diagnosis. Respawn's developer account illustrates why matching client symptoms with server/fleet telemetry is operationally important. [S42](#s42)

A killcam or spectator view is a reconstruction with its own interpolation and missing-information constraints. Do not present it as an exact replay of what either player's screen displayed unless those presentation inputs were actually recorded.

## 21. Capacity model and initial budgets

Every number in this section is a **proposed target or derived illustration**, not a benchmark result. Record the CPU model, core allocation, OS, compiler, build mode, physics configuration, entity population and network profile with every result.

### 21.1 Arena qualification profile

| Parameter | Initial setting | Rationale / constraint |
|---|---:|---|
| Authoritative simulation | 60 Hz | 16.667 ms interval; evaluate 120/128 only after profiling |
| Input generation/send target | 60 Hz | One control sample per tick; bundles carry redundancy |
| Owner checkpoint target | 30 Hz | Replay must handle every useful correction; may raise independently |
| Remote snapshot target | Up to 30 Hz | Relevance/priority may reduce per-entity rate |
| Command redundancy | 4 samples, cap 8 | Loss resilience without indefinite history retransmission |
| Lead range | 1–12 ticks | Tuned from measured command arrival slack |
| Prediction history storage | 256 ticks | About 4.27 s at 60 Hz; memory capacity, not allowed rewind |
| Maximum replay work per frame | 32 ticks per bounded group | Overflow triggers explicit recovery |
| Maximum gameplay rewind | 150 ms | Fairness policy, not a latency guarantee |
| Hit-history storage | 32 ticks plus discontinuity events | About 533 ms; exceeds allowed rewind for margins/inspection |
| Missing held-input grace | 3 ticks | About 50 ms; one-shot edges never repeated |
| Remote extrapolation cap | 100 ms | Then show documented degraded behavior |
| Local visual error half-life | 50 ms | Presentation-only, adjusted by playtest |
| Server catch-up burst | 4 ticks | Larger debt makes the instance unhealthy |
| Snapshot baseline slots | 8 per negotiated stream/group scope | Explicit retention/retirement protocol required |
| Spatial foundation | Persistent Replication Graph | Required in arena, not only scale tests |
| Sparse XY cell size | 32 m | Derived library default, not Fortnite configuration |
| Base cull / leave margin / prefetch cap | 80 m / 10 m / 8 m | Class- and disclosure-bounded; exact post-filter mandatory |
| Approved observers / connection | At most 2 | Server-granted; no arbitrary client camera subscriptions |
| Cells / indexed entity | At most 256 | Reject or route oversized objects explicitly |
| Candidates / connection / gather | At most 4,096 | Explicit overload; never truncate a required group |
| Required dependency entities / connection | At most 128 | Existing per-group/global prediction caps also apply |

History capacity must satisfy the supported correction age, prediction lead, jitter and stall allowance, with safety margin. Formula:

```text
prediction_history_ticks >= ceil((maximum_correction_age + prediction_lead_time
                                  + supported_stall_time) / dt) + safety_ticks
history_memory = retained_ticks * bytes_per_complete_group * simultaneous_groups
```

Do not confuse storage capacity with fairness rewind or permitted replay work. A client may store 256 ticks while refusing to replay more than 32 in one frame and while the server validates shots only within 150 ms.

### 21.2 Derived bandwidth examples

For illustration, 40 changed remote entities averaging 24 encoded bytes each at 30 updates/second consume `40 × 24 × 30 = 28,800 bytes/second` of entity payload. Headers, lifecycle, owner state, receipts, encryption and loss redundancy add to that. A provisional total per-client downstream budget of 60,000 bytes/second leaves room but must be measured against the actual schema.

A 24-byte newest input plus three illustrative 8-byte deltas and 32 bytes of framing/ACK material gives an 80-byte application input datagram. At 60 sends/second that is 4,800 application bytes/second, before transport/IP overhead. These are arithmetic examples, not prescribed serialized field sizes; the logical schema must be encoded and measured to establish real costs.

At 100 clients each receiving 60,000 bytes/second, outbound application-plus-budgeted-overhead traffic is 6,000,000 bytes/second, or 48 Mbit/second, before any overhead not included in that per-client figure. Replication CPU and burst queue behavior may constrain capacity before average link utilization does.

### 21.3 History memory examples

A compact hit-history record of 256 bytes for 128 actors across 32 ticks consumes `256 × 128 × 32 = 1,048,576 bytes`, exactly 1 MiB before indices and allocation overhead. Real animated hitboxes may require much more than 256 bytes per actor; calculate from the actual pose representation rather than accepting that illustration as a requirement.

A 4 KiB prediction group retained for 256 ticks consumes 1 MiB before metadata. Thirty-two such groups consume 32 MiB. Measure copy, encode and restore costs as well as memory size.

### 21.4 CPU and latency gates

For the arena workload on a declared reference machine, propose server simulation p99 below 8 ms at 60 Hz, p99.9 below the 16.667 ms deadline, and no sustained catch-up debt. On a declared minimum client machine, propose p99 replay CPU below 2 ms for the normal supported network envelope. These are acceptance ambitions, not measured capabilities of this handoff.

Report graph prepare/gather/filter/schedule/encode costs separately. The proposed 100-connection / 50,000-potential-actor test must include a sparse distributed layout, dense hotspot, dormant population, mass wake and observer teleport. Publish candidate-visit counts and memory, not only bytes. An index avoids irrelevant pair tests but cannot remove dense required traffic.

Measure input-to-local-visible-response with a high-speed camera or trustworthy instrumented pipeline, including device sampling, simulation wait, render queue and display latency. Library-only timestamps cannot prove end-to-end motion-to-photon latency. Compare 60 and higher tick rates against the same workload; a more expensive tick that causes deadline misses may produce a worse game.

## 22. Test strategy and release acceptance

The planning directory contains a task backlog and a test matrix. The executable Python reference model demonstrates selected protocol/replay invariants only; it is not a 3D engine, secure transport, production library or performance benchmark.

### 22.1 Deterministic unit and property tests

The M1 graph suite compares grid eligibility to an independent brute-force oracle, including negative coordinates, within-cell coverage changes and multiple observers. M2 tests scope identities and per-connection dormancy delivery; M3 tests selective state delivery with actual clients and prediction. The Python spatial model in this package is a smaller executable contract oracle, not proof that these production gates have passed.

Test identical initial state plus commands twice; snapshot/restore at every tick then replay to the same result; comparison at the same end-of-tick convention; no replay of `U[K]` after restoring `S[K]`; quantizer boundaries; stable random streams; lifecycle generation safety; ownership boundaries; and explicit absent versus unavailable history.

Property/fuzz tests generate arbitrary valid command sequences and transport permutations, then assert bounded memory, no duplicate authoritative action, no mutation on failed decode and eventual convergence once a complete valid checkpoint is delivered. Malformed-byte fuzzing covers every codec and decompressor separately, before the simulation layer.

### 22.2 Required network profiles

Run at least these named profiles, each with a documented seed set and both bot and human sessions:

| Profile | Delay and impairment | Required behavior |
|---|---|---|
| LAN | 0–2 ms RTT, no intentional loss | Stable replay; no unexplained drift |
| Typical | 40 ms RTT, 5 ms jitter, 1% loss | Responsive local play, bounded corrections |
| Moderate | 100 ms RTT, 15 ms jitter, 3% loss | Convergence and bounded queues/replay |
| Difficult | 200 ms RTT, 30 ms jitter, 5% loss | Explicit fairness limits and graceful degradation |
| Burst | Typical delay, repeated 100–300 ms loss bursts | Recovery without duplicated actions or permanent stale baselines |
| Asymmetric | 10/90 ms and 90/10 ms one-way delay | Lead feedback works; no false one-way-latency certainty |
| Reordered | Up to 10% delayed/reordered packets, duplicates | No rollback to stale state; deterministic deduplication |
| Constrained | Tight upstream/downstream caps, competing bulk traffic | Fresh-state priority and finite queues |
| Severe | 10% loss, long stalls or unsupported RTT | Safe degradation/resync, not a smoothness promise |

Render rates of 30, 60, 120 and 240 FPS must be mixed with the same server tick rate. Include alt-tab/background suspension, 250 ms and 1 s stalls, clock drift, thread scheduling stalls and server overload. LAN-only editor testing is not an acceptance suite.

### 22.3 Gameplay scenes

Build small reproducible maps: flat movement; stairs and slopes; thin walls and corners; crouch under ceilings; jump/ledge boundaries; rotating moving platforms; player crowding; dash into a moving blocker; knockback during dash; hitscan around static and moving cover; shield/teleport on a shot boundary; projectile ricochet and delayed spawn binding; relevance exit/re-entry during a persistent effect; vehicle ownership handoff; scene streaming and floating-origin boundary.

Each scene has deterministic bot scripts, expected authority invariants and a recorded visual test. Cosmetic correction quality needs human evaluation in addition to state equality.

### 22.4 Quantitative quality gates

Separate correctness gates from provisional feel targets:

| Gate | Requirement |
|---|---|
| Duplicate committed actions | Zero in all automated loss/reorder/reconnect tests |
| Spatial selectivity | Separate clients receive only their eligible fields/events plus explicit global/owner/dependency state; no broadcast-all fallback |
| Scope lifecycle | Lost/reordered entry/exit/ACK/delta traffic cannot resurrect or delete the wrong scope incarnation |
| Dependency/privacy | Required prediction closure is complete and authorized, or prediction is explicitly denied/degraded |
| Graph scaling | Sparse gather visits are independent of unrelated distant actor population; dense overload is reported and bounded |
| Invalid client authority | No accepted arbitrary position, damage, resource or ownership mutation |
| Reconciliation | Exact expected canonical state after complete correction and replay in the deterministic reference domain |
| Decoder | No panic, unbounded allocation or partial-state publication on malformed input |
| Lifecycle | No stale generation resurrection, duplicated predicted spawn or wrong-owner command execution |
| History | Missing baseline/dependency invokes explicit recovery, never guessed continuation |
| Normal static-world movement | At 100 ms RTT / 15 ms jitter / 1% loss, propose p95 correction below 2 cm and p99 below 10 cm, excluding deliberately injected external impulses and reported separately |
| Server deadline | Proposed arena p99 <8 ms and p99.9 <16.667 ms on declared hardware |
| Client replay cost | Proposed p99 <2 ms in the normal profile on declared minimum hardware |
| Recovery | After delivery resumes, a valid complete checkpoint is applied within two successful snapshot opportunities plus processing/lead adjustment; measure action completion separately |
| Soak | At least 24 h with bounded memory, no identifier/lifecycle corruption and no accumulated clock drift |
| Cross-platform | Same canonical replay results within the explicitly supported determinism domain; exceptions require documented tolerance and correction tests |

The centimeter targets are proposed for a meter-based kinematic arena and must be calibrated to movement speed and collision scale. Do not hide mandatory server corrections caused by new opponent actions inside a favorable metric. Report expected external corrections separately from preventable predictor errors.

### 22.5 Comparison with commercial games

A defensible comparison controls hardware, display, network impairment, game speed and action type. Measure input response, remote motion age, correction frequency/magnitude, hit acceptance near cover, server stalls and failure recovery. Commercial games have different movement and fairness rules, so a single aggregate score is misleading.

Where controlled server access is unavailable, treat comparisons as observational and disclose confounders. A working arena passing these gates supports a claim about the library's tested envelope, not universal superiority over every Overwatch, Counter-Strike or Fortnite interaction.

## 23. Development milestones and ownership

Use role ownership rather than assuming a particular team size. A lean team can combine roles, but each acceptance decision has an owner: networking/runtime engineer, gameplay/simulation engineer, engine/presentation engineer, and QA/performance responsibility. The technical lead approves protocol and authority contracts; game design approves fairness policy; security/licensing reviewers approve external-risk decisions.

| Milestone | Deliverable | Depends on | Exit condition |
|---|---|---|---|
| M0: evidence and contracts | Immutable pins, licenses, Replication Graph ADR, routing/disclosure table, state schema and quality envelope | None | Tick/ACK/authority/scope semantics fixed; Epic basis and derived extensions labeled |
| M1: offline simulation and persistent graph | Headless movement, checkpoint/restore, graph nodes, change feed, routing/gather oracle | M0 | Replay tests and selective spatial/semantic/dependency gather tests pass |
| M2: real networking and scoped delivery | Secure transport, admission, ticked inputs, authorized observers, scope entry/exit, full baselines and lifecycle repair | M1 | Separate clients receive distinct sets; lost/reordered scope traffic is safe; input admission passes |
| M3: spatial prediction vertical slice | Owner prediction/replay, dependency closure, interpolation, interest inspector and bounded scheduler | M2 | Separated clients pass core network profiles including platform dependencies and scope re-entry |
| M4: gameplay closure | Moving platforms, abilities, event journal, predicted projectile lifecycle | M3 | No duplicate effects/spawns; dependency mismatch tests pass |
| M5: combat authority | Historical hit world, hitscan/projectile policies, receipts and shot inspector | M4 | Boundary fairness tests pass; no client damage authority |
| M6: replication scale and optimization | Compression/deltas, baseline retirement, graph/frequency tuning, shared encoding and 100-client benchmark | M3, M4 | Sparse/dense/dormant/wake loads meet declared budgets; core scope semantics already proven |
| M7: production hardening | Fuzzing, security, reconnect, cross-platform CI, soak and fleet telemetry | M5, M6 | Correctness/security/soak gates pass |
| M8: advanced features | Sub-tick, dynamic prediction groups or large-world features | M7 | Each feature independently meets an expanded workload envelope |

Do not begin M8 merely because movement looks good on localhost. Sub-tick timestamps cannot repair incomplete restoration; replication compression cannot repair incorrect command finalization; a second engine adapter cannot repair unclear authority.

### Team working agreements

Every gameplay change that affects predicted state adds or updates its schema classification, checkpoint behavior, deterministic event keys, graph route, permitted fields, dependency declaration, regression scene and bandwidth/CPU estimate. Every protocol change adds compatibility and malformed-input tests. Every performance optimization retains a simple reference path or golden-vector test.

Pull requests must answer: what is authoritative; which state changes; what gets replayed; what happens under loss/reordering; how identity is preserved; what the worst-case work is; and how it was tested. “Works in the editor” is not evidence for release.

Each milestone produces runnable commands and artifacts: headless server binary, client build, bot runner, deterministic replay, trace report and test report. CI must run separate-process networking as well as in-memory simulation tests. A sample that cannot launch outside the engine editor does not count as the client/server deliverable.

## 24. Architectural decisions to record before implementation

Create the following ADRs with an owner and test reference. Defaults are provided so development is not blocked by open-ended choices.

| ADR | Default decision | Revisit trigger |
|---|---|---|
| ADR-01 Simulation clock | Global fixed 60 Hz; end-of-tick snapshots | Measured need for another tick rate |
| ADR-02 Authority | Server computes all gameplay outcomes | None for competitive authority |
| ADR-03 Prediction boundary | Owner character/weapon/ability plus declared dependencies | New interactive gameplay requires a larger closure |
| ADR-04 Physics | Shared kinematic controller | New movement or collision requirements |
| ADR-05 Input scheduling | Immutable target tick; reject after finalization | Explicit late-action-only policy with tests |
| ADR-06 Clock lead | Feedback from server arrival slack | New platform/network envelope |
| ADR-07 Hit validation | Isolated historical query; mixed-time origin/target policy | Game design changes defense fairness |
| ADR-08 Rewind cap | 150 ms starting point | Measured fairness/accessibility tradeoff |
| ADR-09 Snapshot baselines | Decoded-retained ACKs and bounded retention | Compression/storage scale pressure |
| ADR-10 Events | Stable keys plus reversible presentation journal | New persistent/irreversible effects |
| ADR-11 Transport | Evaluate Quinn DATAGRAM and native alternatives | Platform support or measured transport limits |
| ADR-12 Spatial foundation | **Selected:** Fortnite/Unreal public Replication Graph model; core nodes and per-connection gather in M1–M2 | Alternative backend requires an explicit ADR and full conformance, not an M6 design choice |
| ADR-13 Replay overload | Controlled resync when work cap exceeded | Proven staged-replay implementation |
| ADR-14 Source reuse | License-vetted dependencies; independent implementation otherwise | New licensing terms or dependency change |
| ADR-15 Numerical domain | Explicit compiler/platform/physics contract | New target platform or optimization flags |
| ADR-16 Sub-tick | Disabled until core qualification | Evidence it materially improves the target gameplay |
| ADR-17 Dependency admission | Required permitted closure before scheduling; privacy denial wins | New gameplay dependency or disclosure policy |
| ADR-18 Scope lifecycle | Per-connection scope epoch, full re-entry, decoded-ready ACK; exit is not death | Representation or lifecycle extension |
| ADR-19 Observers and fields | Server-granted viewpoints and final field/event authorization | Spectator, team-reveal or camera feature |
| ADR-20 Dormancy | Shared mutation version plus connection-specific delivery completion | New subobjects or streaming cache policy |
| ADR-21 Native Unreal backend | Replication Graph, not Replication Graph plus Iris | Deliberate Iris migration with equivalent tests |

## 25. Failure patterns that must block release

Reject implementations that replay the correction tick twice; delete input history after transport ACK; compare present predicted state to past server state; restore only position; smooth the collision body instead of the render body; run gameplay on wall-clock delta during replay; duplicate button edges during packet retransmission; or change command target ticks after sending them.

Also reject implementations that rewind only a target's position but use current shield/cover state without a declared policy; interpolate through teleports; match predicted spawns by distance; treat absent history as a component-removal event; use an unacknowledged snapshot as a delta baseline; process arbitrary reliable RPCs during rollback; or let every received command advance an extra simulation step.

Less obvious blockers include a client collision proxy built from delayed render transforms, a predicted rider restored without platform history, persistent effects that cannot recover after relevance re-entry, generation reuse while packets remain in flight, unlimited replay after a stall, and a physics backend whose complete continuation state has never been restored in a test.

Additional blockers are a world-wide per-client gather scan, a hidden entity disclosed through a dependency, an old-scope delta reviving a replica, an unacknowledged scope entry used as a baseline, a dropped wake leaving permanent stale state, or a dormancy exit interpreted as global death.

No transport library, ECS framework, engine replication graph or higher tick rate makes these failures harmless.

## 26. Deliverable definition and boundaries

The development team must ultimately deliver an installable versioned library; a documented public API; a dedicated server; at least two separate-process clients; the arena sample scenes; a deterministic bot runner; network impairment testing; replay/correction/shot inspectors; secure admission; baseline/scope/dormancy recovery; a first-class persistent Replication Graph with selective real-client demonstrations; compatibility/fuzz tests; deployment guidance; and measured acceptance reports on declared hardware.

The handoff package accompanying this document contains the research/specification, source registry, implementation backlog, feature inventory, acceptance matrix, proposed configuration and executable contract model. It does **not** contain a completed 3D game engine, a production secure client/server implementation, or measured proof of parity with a commercial game. Those are the M1–M7 deliverables, with M8 extending the envelope.

The unverified portion of the research is primarily proprietary detail: current commercial deployment settings, complete Overwatch internals, Fortnite-specific modifications, and CS2's complete sub-tick implementation. Public code and developer documentation support the mechanism families; the detailed library contracts and tests here are explicit engineering recommendations rather than invented claims about those products.

## 27. Protocol completion contracts

This addendum supplies concrete implementation choices that complement the engineering specification. All wire identities and state machines are proposed library contracts, not undocumented claims about commercial games. Spatial routing follows the selected Epic Replication Graph foundation; see SPATIAL_REPLICATION.md. This revision adds scope identity and is not wire-compatible with an implementation that omitted it.

### 27.1. First connection sequence

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

### 27.2. Reconciliation trace with a lost command

Assume both peers agree on `S[100]`, the client predicts through tick 108, and command 105 is lost until after its server deadline. The server executes real inputs for 101–104, substitutes input for 105, then executes real inputs for 106. It sends a complete checkpoint `S_server[106]`, a finalized-through watermark of 106, and an exception marking command 105 expired/substituted.

The client decodes and retains the checkpoint. It compares **predicted state at 106**, not predicted state at 108. It restores `S_server[106]` and replays exactly inputs 107 and 108 with their dependency histories. It does not replay 106, and it does not try to execute expired input 105 again. A fire action that existed only in command 105 receives an expired result and has its provisional effect canceled or retired.

The next datagram may redundantly contain command 105. That does not alter history: the server returns the same terminal late/expired decision. Commands 107–108 remain pending until their authoritative execution/checkpoint makes them obsolete. A transport ACK for the earlier datagram never replaces this logic.

### 27.3. Terminal action outcome state machine

Client-side action state is `Pending -> Accepted | Rejected | Expired`. An accepted result may separately be `AwaitingEntityBinding` for a spawn, but acceptance itself is terminal. The server must not send acceptance and later silently reinterpret the same request as rejection. A later game event can destroy an accepted projectile; that is a new authoritative lifecycle event, not a retroactive rejection of its original spawn request.

Use an outcome record containing action key, authoritative status, execution tick, reason class, optional entity binding, optional authoritative transaction ID and outcome sequence. Repeat until the client acknowledges outcome decoding, or include it in a reliable bounded control/result stream. Also retain enough outcome history for the declared reconnect window.

When a connection restarts, an old action key may be submitted to an **outcome lookup** endpoint, not re-executed as a fresh gameplay request. For durable non-gameplay operations, use a persistent transaction ID independent of transport epoch. Transport reconnection must never regenerate an irreversible operation under a new transaction identity.

A complete checkpoint can finalize all ticks through K without listing every cosmetic event, but pending local actions through K still need a terminal result or a deterministic, documented expiration rule. Do not infer whether a shot happened merely from current ammo: reloads or other resource changes can make that inference ambiguous.

### 27.4. Baseline retention and retirement fencing

A replication scope is qualified by connection epoch, entity generation, scope epoch and representation revision (and group revision/member scope manifest for grouped checkpoints). Each scope has a `baseline_generation`, a monotonically advancing snapshot ID, and a bounded allowed-baseline set. A decoded ACK promises that a named state is retained. The server records that promise before using the state as a base.

To retire a baseline, the client proposes retirement. The server first stops creating new deltas against that baseline, establishes at least one usable replacement or keyframe path, and acknowledges a retirement fence. The client may then discard the retired state according to the negotiated rule. Old datagrams referencing it can still arrive out of order after the reliable retirement ACK; therefore the client must recognize them as retired-generation/stale-base traffic rather than assuming cross-lane ordering.

A stale retired-base delta older than already published state may be discarded. A useful newer delta that cannot be decoded triggers a bounded keyframe request or waits for the already-scheduled replacement. It is never partially applied and never acknowledged as decoded. Limit repeated keyframe requests so one lost baseline does not create an amplification loop.

A scope re-entry or field-representation change requires full state under a new scope identity; no old-scope ACK authorizes a new delta. A baseline-generation reset invalidates all previous baselines in that scope and starts from a complete keyframe. Snapshot identity alone is insufficient across a generation or connection reset. Baseline cache size includes state and metadata; the client may refuse a configuration that exceeds its negotiated memory budget.

### 27.5. Resource-budget contract

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

### 27.6. Public API completion checklist

Expose engine-neutral operations for creating/destroying worlds; receiving bounded transport events; advancing the server to a monotonic deadline; capturing hardware input into a command journal; advancing prediction; applying decoded authoritative updates; extracting presentation data; querying action outcomes; registering graph routes; publishing committed bounds/routing/dormancy changes; granting/revoking observers; preparing/gathering the persistent graph; closing authorized dependencies; applying scope-ready/exit/repair receipts; changing ownership under server authority; requesting resynchronization; and collecting telemetry.

Every method specifies thread ownership, allocation behavior, error type and whether it can mutate the world. `apply_snapshot` is transactional. `render_extract` is read-only. `receive_transport_event` cannot directly mutate physics. `advance_server` owns the authoritative tick barrier. `advance_prediction` may perform bounded replay but does not emit irreversible external effects.

Gameplay registration requires a stable schema ID, canonical codec, default state, capture/restore behavior, dependency declaration and interpolation policy. Registering a field as predicted without providing restoration must fail at initialization. Dynamic component changes require an explicit lifecycle path; ordinary engine reflection is not sufficient.

A published adapter must supply executable examples for normal connection, two spatially separated clients, interest exit/re-entry, dropped dormancy wake, hidden-dependency rejection, prediction correction, rejected action, missing baseline, reconnect, spawn rejection, moving platform, historical hit query and graceful disconnect. API documentation must show the complete lifecycle, not only an attractive three-line connect example.

### 27.7. Scene revisions, large worlds and server AI

Use authoritative world-space or cell-plus-local coordinates in history. A floating-origin shift changes the rendering/working origin, not the historical identity of a location. Record the origin/cell revision with cached local-space query data, and transform query origins/targets into a common historical coordinate frame before testing. A shift must not appear as a teleport or invalidate shot distances accidentally.

For scene streaming, collision data has a content hash and revision. A server-approved entry boundary determines when an entity can interact with a region. Clients lacking the required collision revision cannot predict through it as empty space. Dynamic destruction is a ticked topology change represented in the historical blocker world.

Server AI is authoritative. Its decisions and random inputs must be included in replay logs or be reproducible from recorded initial conditions and ticked inputs. Clients normally interpolate AI actors or predict only a limited motion proxy. Do not run unsynchronized client AI and assume it is a trustworthy dependency for owner prediction.

### 27.8. Completion evidence

The production release report must attach a dependency/version manifest, source/license register, serialized protocol golden vectors, compatibility matrix, deterministic replay corpus, real-process networking logs, fuzz results, network-profile reports, hardware/CPU/bandwidth measurements, visual correction recordings, hit-policy boundary traces and a soak report.

The Python movement and spatial models in this handoff validate only their stated subsets. The spatial model is an in-memory contract demonstration, not a live replication backend or proof of the production scaling envelope. In particular, its simplified baseline store refuses unnegotiated eviction; the retirement state machine above still needs production implementation. Its command model is single-owner and does not replace the full multi-owner sequence/ownership admission policy. Its group assembler has one staged group and must be extended with bounded revision supersession and expiry. Do not mistake passing the model's tests for completing the release evidence list.

### 27.9. Scope transition and delivery contract

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

## Sources

<a id="s01"></a>

**S01. Valve. [Source SDK 2013 repository](https://github.com/ValveSoftware/source-sdk-2013).** Repository reviewed 2026-09-09; moving master.

Evidence scope: Public HL2, HL2DM and TF2 game code; non-commercial SDK terms. Not Counter-Strike 2 source.

<a id="s02"></a>

**S02. Valve. [Source SDK: client prediction.cpp](https://raw.githubusercontent.com/ValveSoftware/source-sdk-2013/master/src/game/client/prediction.cpp).** Moving master reviewed 2026-09-09.

Evidence scope: Historical command comparison, simulation restoration and replay, prediction state.

<a id="s03"></a>

**S03. Valve. [Source SDK: player_lagcompensation.cpp](https://raw.githubusercontent.com/ValveSoftware/source-sdk-2013/master/src/game/server/player_lagcompensation.cpp).** Moving master reviewed 2026-09-09.

Evidence scope: Historical hitbox/animation state, bounded rewind and restore. Source-era implementation, not proof of current CS2 behavior.

<a id="s04"></a>

**S04. Valve. [Source SDK: usercmd.h](https://raw.githubusercontent.com/ValveSoftware/source-sdk-2013/master/src/game/shared/usercmd.h).** Moving master reviewed 2026-09-09.

Evidence scope: Command fields, sequence, tick and input representation.

<a id="s05"></a>

**S05. Valve. [Source SDK: gamemovement.cpp](https://raw.githubusercontent.com/ValveSoftware/source-sdk-2013/master/src/game/shared/gamemovement.cpp).** Moving master reviewed 2026-09-09.

Evidence scope: Shared movement implementation as an architectural reference.

<a id="s06"></a>

**S06. Valve. [Counter-Strike 2: official announcement and feature description](https://www.counter-strike.net/cs2).** Official product page reviewed 2026-09-09.

Evidence scope: Sub-tick action timing claim. Does not publish the production implementation or establish that tick rate is irrelevant.

<a id="s07"></a>

**S07. Blizzard / GDC. [Timothy Ford: Overwatch Gameplay Architecture and Netcode](https://www.gdcvault.com/play/1024001/-Overwatch-Gameplay-Architecture-and).** GDC 2017 session; official abstract reviewed.

Evidence scope: ECS and determinism in responsive networked gameplay. Full video/transcript was not available for verification; detailed proprietary algorithms remain unverified.

<a id="s08"></a>

**S08. Epic Games. [Networked Movement in the Character Movement Component](https://dev.epicgames.com/documentation/en-us/unreal-engine/understanding-networked-movement-in-the-character-movement-component-for-unreal-engine).** Live documentation reviewed 2026-09-09.

Evidence scope: Saved moves, move combining, unreliable ServerMove flow, server validation, replay and smoothing. Not an identical fixed-global-tick model.

<a id="s09"></a>

**S09. Epic Games. [Replication Graph](https://dev.epicgames.com/documentation/en-us/unreal-engine/replication-graph-in-unreal-engine).** Public documentation reverified 2026-09-09; historical Fortnite example.

Evidence scope: Primary architectural basis: persistent graph nodes and per-connection actor gathering; historical Fortnite 100-connection / approximately 50,000-actor motivating example. Not current deployment parameters.

Architecture role: Primary spatial architecture reference.

<a id="s10"></a>

**S10. Epic Games. [Introduction to Iris](https://dev.epicgames.com/documentation/unreal-engine/introduction-to-iris-in-unreal-engine).** Live documentation reviewed 2026-09-09.

Evidence scope: Opt-in replication architecture developed from large-scale multiplayer experience; replication is distinct from prediction.

Architecture role: Alternative backend comparison; not combined with native Replication Graph.

<a id="s11"></a>

**S11. Epic Games. [FPredictionKey API and gameplay prediction design](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Plugins/GameplayAbilities/FPredictionKey).** Live documentation reviewed 2026-09-09.

Evidence scope: Prediction keys, accepted/rejected predicted effects, cues and scoped prediction limitations.

<a id="s12"></a>

**S12. Epic Games. [Networked Physics Overview](https://dev.epicgames.com/documentation/en-us/unreal-engine/networked-physics-overview).** Live documentation reviewed 2026-09-09.

Evidence scope: Default replication, predictive interpolation and resimulation; history and performance tradeoffs.

<a id="s13"></a>

**S13. Epic Games. [NetworkPrediction plugin API](https://dev.epicgames.com/documentation/unreal-engine/API/Plugins/NetworkPrediction).** Live documentation reviewed 2026-09-09.

Evidence scope: Public API surface; source must be inspected in the exact licensed engine version before implementation.

<a id="s14"></a>

**S14. Epic Games. [NetworkPredictionExtras plugin API](https://dev.epicgames.com/documentation/unreal-engine/API/Plugins/NetworkPredictionExtras).** Live documentation reviewed 2026-09-09.

Evidence scope: Example input, sync and auxiliary model types; not a Fortnite source dump.

<a id="s15"></a>

**S15. Epic Games. [Downloading Unreal Engine Source Code](https://dev.epicgames.com/documentation/en-us/unreal-engine/downloading-source-code-in-unreal-engine).** Live documentation reviewed 2026-09-09.

Evidence scope: Engine source access subject to Epic account and license requirements.

<a id="s16"></a>

**S16. Unity. [Introduction to prediction](https://docs.unity3d.com/Packages/com.unity.netcode@1.5/manual/intro-to-prediction.html).** Netcode for Entities 1.5 documentation.

Evidence scope: Shared simulation, prediction ticks, slack and rollback cost.

Architecture role: Supporting prediction/replication comparison; not the spatial architecture basis.

<a id="s17"></a>

**S17. Unity. [Prediction details](https://docs.unity3d.com/Packages/com.unity.netcode@1.5/manual/prediction-details.html).** Netcode for Entities 1.5 documentation.

Evidence scope: Partial snapshots, interacting entities at different replay ticks and grouping mitigation.

Architecture role: Supporting prediction/replication comparison; not the spatial architecture basis.

<a id="s18"></a>

**S18. Unity. [Time synchronization](https://docs.unity3d.com/Packages/com.unity.netcode@1.5/manual/time-synchronization.html).** Netcode for Entities 1.5 documentation.

Evidence scope: Prediction/interpolation clocks, command-buffer feedback and gradual clock correction.

Architecture role: Supporting prediction/replication comparison; not the spatial architecture basis.

<a id="s19"></a>

**S19. Unity. [Use the command stream to handle inputs](https://docs.unity3d.com/Packages/com.unity.netcode@1.5/manual/command-stream.html).** Netcode for Entities 1.5 documentation.

Evidence scope: Command redundancy, tick sampling and one-off input events.

Architecture role: Supporting prediction/replication comparison; not the spatial architecture basis.

<a id="s20"></a>

**S20. Unity. [Ghost snapshots](https://docs.unity3d.com/Packages/com.unity.netcode@1.5/manual/ghost-snapshots.html).** Netcode for Entities 1.5 documentation.

Evidence scope: Snapshot replication and diagnostics reference.

Architecture role: Supporting prediction/replication comparison; not the spatial architecture basis.

<a id="s21"></a>

**S21. Unity. [Prediction smoothing](https://docs.unity3d.com/Packages/com.unity.netcode@1.5/manual/prediction-smoothing.html).** Netcode for Entities 1.5 documentation.

Evidence scope: Separate handling of predicted state correction and visible smoothing.

Architecture role: Supporting prediction/replication comparison; not the spatial architecture basis.

<a id="s22"></a>

**S22. Unity. [Client anticipation](https://mp-docs.dl.it.unity3d.com/netcode/2.2.0/advanced-topics/client-anticipation/).** Netcode for GameObjects 2.2.0 documentation; version-specific.

Evidence scope: Anticipation explicitly lacks a full rollback/replay loop. Do not equate it with Netcode for Entities.

<a id="s23"></a>

**S23. Unity. [EntityComponentSystemSamples: Netcode samples README](https://raw.githubusercontent.com/Unity-Technologies/EntityComponentSystemSamples/master/NetcodeSamples/README.md).** Moving master reviewed 2026-09-09.

Evidence scope: Official source examples including NetCube, Asteroids and prediction switching.

<a id="s24"></a>

**S24. Unity. [CharacterControllerSamples: netcode character tutorial](https://raw.githubusercontent.com/Unity-Technologies/CharacterControllerSamples/master/_Documentation/Tutorial/tutorial-netcodecharacters.md).** Moving master reviewed 2026-09-09.

Evidence scope: Official predicted-character integration walkthrough.

<a id="s25"></a>

**S25. Unity. [EntityComponentSystemSamples license](https://raw.githubusercontent.com/Unity-Technologies/EntityComponentSystemSamples/master/LICENSE.md).** Reviewed 2026-09-09.

Evidence scope: Unity Companion License for Unity-dependent projects.

<a id="s26"></a>

**S26. Unity. [CharacterControllerSamples license](https://raw.githubusercontent.com/Unity-Technologies/CharacterControllerSamples/master/LICENSE.md).** Reviewed 2026-09-09.

Evidence scope: Unity Companion License for Unity-dependent projects.

<a id="s27"></a>

**S27. Lightyear maintainers. [Lightyear repository](https://github.com/cBournhonesque/lightyear).** Moving main reviewed 2026-09-09.

Evidence scope: Rust/Bevy client prediction, interpolation, tick inputs and replication; candidate for reuse rather than assuming a new implementation is necessary.

<a id="s28"></a>

**S28. Lightyear maintainers. [Prediction crate source](https://raw.githubusercontent.com/cBournhonesque/lightyear/main/crates/replication/prediction/src/lib.rs).** Moving main reviewed 2026-09-09.

Evidence scope: Source entry point for prediction modules.

<a id="s29"></a>

**S29. Lightyear maintainers. [Rollback module source](https://raw.githubusercontent.com/cBournhonesque/lightyear/main/crates/replication/prediction/src/rollback.rs).** Moving main reviewed 2026-09-09.

Evidence scope: Source entry point for rollback structure; pin before deeper code audit.

<a id="s30"></a>

**S30. Lightyear maintainers. [Issue 1511: sparse prediction history distinction](https://github.com/cBournhonesque/lightyear/issues/1511).** June 2026 report, subsequently closed.

Evidence scope: Regression-test inspiration: unavailable/no-entry is not the same as an explicitly removed component.

<a id="s31"></a>

**S31. Lightyear maintainers. [Issue 1602: unchanged-component prediction history](https://github.com/cBournhonesque/lightyear/issues/1602).** July 2026 report, subsequently closed.

Evidence scope: Regression-test inspiration for unchanged components and baseline/history lifetime, not a claim of an unfixed bug.

<a id="s32"></a>

**S32. Renet maintainers. [Renet repository](https://github.com/lucaspoffo/renet).** Moving repository reviewed 2026-09-09.

Evidence scope: Rust networking and channel/transport building blocks; not by itself a gameplay prediction system.

<a id="s33"></a>

**S33. Valve. [GameNetworkingSockets repository](https://github.com/ValveSoftware/GameNetworkingSockets).** Moving repository reviewed 2026-09-09.

Evidence scope: Native reliable/unreliable networking transport. Hosted Steam services must not be assumed available through the standalone library.

<a id="s34"></a>

**S34. Quinn maintainers. [Quinn repository](https://github.com/quinn-rs/quinn).** Moving repository reviewed 2026-09-09.

Evidence scope: Rust QUIC transport option; benchmark datagrams and shared congestion for game workload.

<a id="s35"></a>

**S35. IETF. [RFC 9221: An Unreliable Datagram Extension to QUIC](https://www.rfc-editor.org/rfc/rfc9221.html).** March 2022.

Evidence scope: Unreliable QUIC datagrams, congestion control and separate reliable stream semantics.

<a id="s36"></a>

**S36. IETF. [RFC 9000: QUIC, A UDP-Based Multiplexed and Secure Transport](https://www.rfc-editor.org/rfc/rfc9000.html).** May 2021.

Evidence scope: Transport security, multiplexing, flow control and connection behavior.

<a id="s37"></a>

**S37. id Software. [Quake III Arena source repository](https://github.com/id-Software/Quake-III-Arena).** Official historical GPL source release.

Evidence scope: Inspectable historical client/server implementation; GPL reuse obligations require review.

<a id="s38"></a>

**S38. id Software. [Quake III: cg_predict.c](https://raw.githubusercontent.com/id-Software/Quake-III-Arena/master/code/cgame/cg_predict.c).** Historical source.

Evidence scope: Prediction from authoritative snapshots, buffered commands and correction handling.

<a id="s39"></a>

**S39. id Software. [Quake III: msg.c](https://raw.githubusercontent.com/id-Software/Quake-III-Arena/master/code/qcommon/msg.c).** Historical source.

Evidence scope: Explicit state fields and delta/bit-level message representation.

<a id="s40"></a>

**S40. Riot Games. [Peeking into VALORANT's Netcode](https://www.riotgames.com/en/news/peeking-valorants-netcode).** July 28, 2020.

Evidence scope: Published latency budget, buffering and responsiveness/fairness tradeoffs. Historical design explanation.

<a id="s41"></a>

**S41. Riot Games. [Demolishing Wallhacks with VALORANT's Fog of War](https://www.riotgames.com/en/news/demolishing-wallhacks-valorants-fog-war).** April 14, 2020.

Evidence scope: Withholding irrelevant information, conservative relevance, and reconstructing persistent effects when entities become visible.

<a id="s42"></a>

**S42. Respawn / Electronic Arts. [What Makes Apex Tick: A Developer Deep Dive into Servers and Netcode](https://www.ea.com/games/apex-legends/news/servers-netcode-developer-deep-dive).** April 28, 2021.

Evidence scope: Developer explanation of lag-compensation fairness, tick/load budgets and operational telemetry. Not a current tick-rate claim.

<a id="s43"></a>

**S43. Mirror maintainers. [Client-Side Prediction](https://mirror-networking.gitbook.io/docs/manual/general/client-side-prediction).** Live documentation reviewed 2026-09-09.

Evidence scope: Prediction/history correction in Unity physics; engine integration comparison, not a vetted production equivalence claim.

<a id="s44"></a>

**S44. FirstGearGames. [FishNet license](https://github.com/FirstGearGames/FishNet/blob/main/LICENSE.md).** Reviewed 2026-09-09.

Evidence scope: Restricts use in competing networking solutions; excluded from implementation derivation in this handoff.

<a id="s45"></a>

**S45. FirstGearGames. [FishNet feature overview](https://fish-networking.gitbook.io/docs/overview/readme/features).** Live documentation reviewed 2026-09-09.

Evidence scope: Capability comparison only; prediction and separately licensed/pro lag-compensation features.

<a id="s46"></a>

**S46. Glenn Fiedler / Gaffer on Games. [Snapshot Interpolation](https://gafferongames.com/post/snapshot_interpolation/).** Original author technical article.

Evidence scope: Snapshot buffering, interpolation and loss/smoothness tradeoffs.

<a id="s47"></a>

**S47. Glenn Fiedler / Gaffer on Games. [Snapshot Compression](https://gafferongames.com/post/snapshot_compression/).** Original author technical article.

Evidence scope: Quantization and snapshot delta/compression design.

<a id="s48"></a>

**S48. Glenn Fiedler / Gaffer on Games. [State Synchronization](https://gafferongames.com/post/state_synchronization/).** Original author technical article.

Evidence scope: Priority accumulation, jitter buffering and quantization consistency.

<a id="s49"></a>

**S49. Dimforge / Rapier. [Determinism (Rust)](https://rapier.rs/docs/user_guides/rust/determinism/).** Live documentation reviewed 2026-09-09.

Evidence scope: Local versus cross-platform determinism and explicit enhanced-determinism preconditions.

<a id="s50"></a>

**S50. Unity. [Latency and performance](https://docs.unity.cn/Packages/com.unity.netcode.gameobjects@2.13/manual/latency-performance.html).** Netcode for GameObjects 2.13 documentation, search-indexed primary page.

Evidence scope: Also distinguishes anticipation from full rollback/replay; corroborates the version-specific NGO comparison.

<a id="s51"></a>

**S51. Epic Games. [Replication Graph: UReplicationGraphNode](https://dev.epicgames.com/documentation/unreal-engine/API/Plugins/ReplicationGraph/UReplicationGraphNode?lang=en-US).** Public UE 5.8 documentation/API reviewed 2026-09-09; pin engine checkout before implementation.

Evidence scope: Persistent node lifecycle; prepare hook and per-connection gather API. Public API, not audited proprietary Fortnite code.

Architecture role: Primary spatial architecture reference.

<a id="s52"></a>

**S52. Epic Games. [Replication Graph: GridSpatialization2D](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Plugins/ReplicationGraph/UReplicationGraphNode_GridSpatia-).** Public UE 5.8 documentation/API reviewed 2026-09-09; pin engine checkout before implementation.

Evidence scope: Static, dynamic and dormancy-driven spatial registration, cell configuration and prepare/gather hooks. Exact portable grid algorithm is derived.

Architecture role: Primary spatial architecture reference.

<a id="s53"></a>

**S53. Epic Games. [Replication Graph: AlwaysRelevant](https://dev.epicgames.com/documentation/unreal-engine/API/Plugins/ReplicationGraph/UReplicationGraphNode_AlwaysRele-).** Public UE 5.8 documentation/API reviewed 2026-09-09; pin engine checkout before implementation.

Evidence scope: Global always-relevant node API. Does not establish application-specific disclosure policy.

Architecture role: Primary spatial architecture reference.

<a id="s54"></a>

**S54. Epic Games. [Replication Graph: AlwaysRelevant_ForConnection](https://dev.epicgames.com/documentation/unreal-engine/API/Plugins/ReplicationGraph/UReplicationGraphNode_AlwaysRele-_1?lang=en-US).** Public UE 5.8 documentation/API reviewed 2026-09-09; pin engine checkout before implementation.

Evidence scope: Connection node adds controller and view target in the documented version; team/private inventory routing is an extension.

Architecture role: Primary spatial architecture reference.

<a id="s55"></a>

**S55. Epic Games. [Replication Graph: DormancyNode](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Plugins/ReplicationGraph/UReplicationGraphNode_DormancyNo-).** Public UE 5.8 documentation/API reviewed 2026-09-09; pin engine checkout before implementation.

Evidence scope: Shared dormancy node and connection-specific dormant-list operations.

Architecture role: Primary spatial architecture reference.

<a id="s56"></a>

**S56. Epic Games. [Replication Graph: ConnectionDormancyNode](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Plugins/ReplicationGraph/UReplicationGraphNode_Connection-).** Public UE 5.8 documentation/API reviewed 2026-09-09; pin engine checkout before implementation.

Evidence scope: Per-connection dormancy node API; portable state-version ACK semantics are a derived contract.

Architecture role: Primary spatial architecture reference.

<a id="s57"></a>

**S57. Epic Games. [Replication Graph: DynamicSpatialFrequency](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Plugins/ReplicationGraph/UReplicationGraphNode_DynamicSpa-).** Public UE 5.8 documentation/API reviewed 2026-09-09; pin engine checkout before implementation.

Evidence scope: Distance-dependent frequency for moving actors; not an authorization mechanism or atomic prediction guarantee.

Architecture role: Primary spatial architecture reference.

<a id="s58"></a>

**S58. Epic Games. [Actor Network Dormancy](https://dev.epicgames.com/documentation/en-us/unreal-engine/actor-network-dormancy-in-unreal-engine).** Public UE 5.8 documentation/API reviewed 2026-09-09; pin engine checkout before implementation.

Evidence scope: Wake/flush ordering, delivery completion, dormancy versus relevancy, and special grid handling. Defaults and console options must be checked against the pinned engine.

Architecture role: Primary spatial architecture reference.

<a id="s59"></a>

**S59. Epic Games. [Replication Graph: BasicReplicationGraph](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Plugins/ReplicationGraph/UBasicReplicationGraph).** Public UE 5.8 documentation/API reviewed 2026-09-09; pin engine checkout before implementation.

Evidence scope: Example implementation and explicitly documented limitations on runtime per-actor policy changes.

Architecture role: Primary spatial architecture reference.

<a id="s60"></a>

**S60. Epic Games. [Migrate to Iris](https://dev.epicgames.com/documentation/unreal-engine/migrate-to-iris-in-unreal-engine).** Public UE 5.8 documentation/API reviewed 2026-09-09; pin engine checkout before implementation.

Evidence scope: Iris and Replication Graph are separate systems; one network driver cannot use both. No claim about current Fortnite deployment choice.

Architecture role: Alternative backend boundary.

<a id="s61"></a>

**S61. Epic Games. [Replication Graph: FGlobalActorReplicationInfo](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Plugins/ReplicationGraph/FGlobalActorReplicationInfo).** Public UE 5.8 documentation/API reviewed 2026-09-09; pin engine checkout before implementation.

Evidence scope: Shared per-actor metadata, dependent actor lists and level-visible dependent gathering; not atomic rollback semantics.

Architecture role: Primary spatial architecture reference.

<a id="s62"></a>

**S62. Epic Games. [Replication Graph: FConnectionReplicationActorInfo](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Plugins/ReplicationGraph/FConnectionReplicationActorInfo).** Public UE 5.8 documentation/API reviewed 2026-09-09; pin engine checkout before implementation.

Evidence scope: Connection-local per-actor dormancy, last/next replication frames and cull metadata.

Architecture role: Primary spatial architecture reference.

