# Network integration contracts

This guide selects the Bevy integration of the normative
[engineering specification](../todo/ENGINEERING_SPEC.md),
[spatial contract](../todo/SPATIAL_REPLICATION.md), and
[protocol addendum](../todo/PROTOCOL_ADDENDUM.md). Those documents define the
release requirements; this guide maps them to Dreamwake and records conservative
integration defaults. Dreamwake's live adapter uses the scoped protocol. Complete
world snapshots remain an offline checkpoint and replay format.

## Authority, ticks and receipts

`S[K]` means the committed state after tick K. Command `U[K]` advances
`S[K-1]` to `S[K]`. Restore K, compare against predicted K, and replay K+1
through the prediction cursor. Tick zero is initialization. Publish a checkpoint
only after all ordered simulation systems and deferred lifecycle changes finish.
The transport frame, replication frame, simulation tick and snapshot identity
are distinct. A paused game may publish a new transport revision without
advancing gameplay; consumers must not infer a simulation step from publication.

Live gameplay input carries immutable target ticks and stream identities. Menu
actions use a separate bounded reliable sequence. The live protocol keeps four
independent receipts:

| Receipt | Exact fact | Permitted consequence |
|---|---|---|
| Transport | Transport received bytes | Transport retry accounting only |
| Decoded baseline | Exact scoped snapshot validated, published and retained | That exact identity may become a delta base |
| Finalized input | Target ticks through a watermark committed, with bounded exceptions for substitution/rejection | Retire commands only with an authoritative checkpoint covering their effects |
| Action outcome | Stable action key accepted, rejected or expired, with execution tick/reason and optional entity binding | Resolve the matching provisional effect; repeated delivery returns the same terminal result |

Scope exits and destructions enqueue once on the reliable control lane. After
transport acceptance, reliable retransmission owns delivery; the application
fence remains pending until its exact acknowledgement arrives. Stream resync
preserves the transport's ordered delivery state and closes old action outcomes
before creating the new stream. It does not discard one endpoint's send queue.

Receipt of a newer input sequence does not prove earlier commands executed.
Keep one control sample per owner per tick and the existing stricter one-action
per-player-per-tick admission until a tested ruleset explicitly changes it.
Redundant commands retain their original sequence, target tick and action keys;
late commands cannot purchase additional simulation steps. Substitute held input
only within its bounded grace, with all one-shot edges cleared. Dreamwake reserves
work for a maximum eight-record input bundle before dequeueing, permitting eight
bundles per committed tick against the 64-record admission budget. Excess queued
bundles remain in the bounded transport queue until another tick commits.
Duplicate records consume this budget; message flood limits remain independent.

Scope identity includes connection epoch, entity generation, scope epoch and
representation revision. Group state adds the group revision and complete member
manifest. Re-entry or a narrower field grant requires new scope identity and
full state. A delta requires a decoded and retained matching baseline; failed or
partial decode changes no published state and produces no decoded receipt.
A Ready handshake removes the connection grant while preserving the advertised
owner entity identity for a retained authoritative owner. Initial recovery follows
the [admission and readiness contract](netcode.md#admission-and-readiness): it
retires an expired startup stream within the existing deadline and retry cap,
without skipping receipts or changing participation. Only fresh stream proofs
can activate it; already participating owners do not use this initial retry.

Retirement fences stop new use before a baseline is evicted. Old control and
state lanes may reorder independently. A delayed owner-group retirement may
refer to an optional member removed by a newer group publication. Reject it as
obsolete only when a retained member proves the older owner-group revision and
the removed member's exact entity generation and connection are covered by a
bounded scope fence. Validate every control in the batch before classifying it:
unrelated, malformed and conflicting identities remain hard errors. Rejection
must preserve current caches and pending retirements atomically.
Interest exit retires a replica without
signaling death; authoritative destruction ends its entity generation. Dormancy
completion is each connection's decoded state version, not a global sleep flag.

## Controller replacement

Trusted server code may call `dreamwake_server::authorize_controller_handoff`
with an active connection and an effective tick within five simulation seconds.
A subsequent authenticated connection must claim the same Traveler identity
before that tick. At most one intent per Traveler and the configured client
count can be pending. Missing, disconnected or expired replacements cancel the
intent without revoking current control; recovery and match restart cancel it.
There is no client controller-transfer message or separate identity registry.

At the boundary, the existing epoch allocator advances connection and ownership
identity while preserving the stable actor. Old queued actions receive
`OwnershipDenied` only if they have no final verdict; earlier terminal outcomes
remain immutable. The previous controller stops before that tick, including held
input and charge motion. The replacement starts neutral with a fresh Welcome and
normal Ready/full-group decode. Its baseline is published from committed state
after the cutover. Gameplay resumes only after ordinary activation, preserving
stamina, cooldowns, health and loadout. This intentionally permits a neutral
interval while the replacement loads its full checkpoint.

The reusable `engine_net::ownership::OwnershipHandoff` validates effective-tick
stream fences without authenticating or allocating identities. Command inboxes
check ownership again at consumption so already queued inputs and held
substitutes cannot bypass a changed authority policy.

## Charged action input

Charge begin, release and cancel are reliable command edges. Begin uses its
assigned command sequence as the episode identity; later edges reference that
identity and the simulation verifies the complete stream key. Continuous held
input cannot carry a charge command or client-supplied elapsed duration. The
client retains a bounded FIFO of 32 action edges and sends one per simulation
tick, so a press and release captured together remain separate commands. Stream
recovery clears this queue and its local episode reference. Shared simulation
owns charge duration, stamina cost, motion and interruption; snapshots carry the
resulting state for authoritative restore and owner replay.

## One writer and Bevy integration

Select the existing headless Bevy simulation path. `DreamwakePlugin` composes
engine capabilities in `DreamStep`; `DreamSimulation` supplies the stepped
authoritative world and offline replay. The restricted live owner predictor
shares the authority's owner intent and component transition functions, using
only its approved checkpoint. Portable networking algorithms may have no
Bevy dependency, but this integration does not replace Bevy scheduling, entity
relationships, transforms or change detection with parallel implementations.

```text
Authenticated transport -> bounded command inbox -> Dreamwake admission
    -> one DreamStep -> committed authoritative state
    -> shared graph prepare -> per-connection gather/policy/dependencies
    -> scoped lifecycle -> budget/encode -> transport

Decoded owner group -> transactional restore -> bounded restricted owner replay
    -> read-only presentation extraction -> renderer correction offset
```

`ServerPlugin<A>` owns generic tick/lifecycle scheduling. `DreamwakeServerPlugin`
owns admission, game validation, grants and snapshot projection. Engine crates
must not import Dreamwake types. Prediction is a separate local simulation world,
never a second writer into the server world. Receive handlers stage messages;
they do not mutate gameplay or spawn mechanic effects. Correction replaces
simulation state before render smoothing. Input capture performs the sole
camera-to-world conversion; replay reads recorded input, never live devices.

Use ordered public system sets and normal deferred-command barriers. Capture
engine components and game payloads together. Restore an entire validated group
transactionally and rebuild Bevy ownership relationships from stable IDs before
stepping. Cached target indices are derived again from restored components.
Graph workers read one frozen committed world/policy revision; newer permission
revocations fence queued unsent work before encoding. A spatial index is an
allowed index, not another owner of actor state. See
[architecture](architecture.md) and [Bevy integration](bevy-integration.md).

## Epic design basis

The selected basis is the public Epic Replication Graph pattern: persistent
shared spatial/global lists, connection lists, once-per-frame prepare,
per-connection gather, and connection-specific dormancy accounting. The source
register identifies the reviewed public API documentation as UE 5.8 (S09,
S51–S62 in the [specification](../todo/ENGINEERING_SPEC.md)). This is an
independently implemented portable algorithm, with no Epic source imported.
It is not Fortnite wire compatibility or evidence of current Fortnite settings.
Unity is supporting comparison. Iris and native Unreal Replication Graph are
alternative native backends; neither is layered into this Bevy path.

For each networking defect, consult the relevant Epic design before choosing
the correction. Keep the applicable mechanism and deliberate local differences
here; runtime captures remain in qualification artifacts. A reference informs
the design but does not replace a reproduction or establish that our code works.

Epic's [CMC/Mover comparison](https://dev.epicgames.com/documentation/en-us/unreal-engine/comparing-mover-and-character-movement-component-in-unreal-engine)
describes Character Movement (CMC) as established in shipped projects and Mover
as experimental. CMC processes client-authored moves when their RPCs arrive.
The shared-timeline Network Prediction model buffers inputs for their intended
simulation frame and predicts ahead locally. Dreamwake uses the latter timing
shape, with its own restricted owner simulation. This resemblance is not evidence
that our implementation or limits are used by Fortnite.

The following references are Epic's UE 5.8 documentation. API descriptions expose
specific contracts; they do not establish undocumented control algorithms.

| Concern | Epic reference and mechanism | Local application and difference |
|---|---|---|
| Effect bindings across delivery lanes | [Replicated object execution order](https://dev.epicgames.com/documentation/en-us/unreal-engine/replicated-object-execution-order-in-unreal-engine) does not guarantee relative reliable/unreliable execution order; [Network Prediction cue dispatch](https://dev.epicgames.com/documentation/unreal-engine/API/Plugins/NetworkPrediction/TNetSimCueDispatcher) separates saved cue delivery, pruning and rollback. | A validated binding for a strictly newer entity generation retires the older journal token atomically, even if its removal checkpoint has not arrived. Cancel the old presentation without creating the replacement from receive; retain its tombstone through replay. Same-generation conflicts and lower-generation bindings for different events remain errors. Generation reuse and this journal transition are our contract, not an Epic cue algorithm or Fortnite behavior. |
| Retired optional group members | [Replication Graph](https://dev.epicgames.com/documentation/en-us/unreal-engine/replication-graph-in-unreal-engine) builds changing actor lists per connection; [actor relevancy](https://dev.epicgames.com/documentation/en-us/unreal-engine/actor-relevancy-in-unreal-engine) removes client replicas when actors cease to be relevant. | Keep bounded scope fences after local replica removal so delayed controls can be checked against connection, entity generation, scope and group revision. A proven old owner-group retirement is obsolete and cannot mutate current caches; an unknown member without that proof remains a hard failure. The atomic baseline-retirement protocol and proof rule are our design, not an Epic wire contract. |
| Effect source visibility | [Actor relevancy](https://dev.epicgames.com/documentation/unreal-engine/actor-relevancy-in-unreal-engine) is decided per connection, and [Replication Graph](https://dev.epicgames.com/documentation/unreal-engine/replication-graph-in-unreal-engine) builds per-connection actor lists for players and AI-controlled actors. | Independently eligible heroes and enemies both authorize their public source references. A visible projectile or effect never grants visibility to its source. Source visibility changes replace only that source's projectile/wisp/effect scopes, even when publication cadence would otherwise freeze them. Separate event-origin approval and anonymous 8 m proxies are Dreamwake policy, not an Unreal projection mechanism. |
| Saved input and correction | [CMC's movement lifecycle](https://dev.epicgames.com/documentation/en-us/unreal-engine/understanding-networked-movement-in-the-character-movement-component-for-unreal-engine) applies corrections before local input, retains saved moves until acknowledgement/correction and replays after authoritative correction. This is also source S08 in the specification. | Retain immutable input/edge history until authoritative finalization. Restore the authorized owner group and replay its recorded transitions before render smoothing. Before the first authored command, with no retained prediction transitions, adopting a newer complete group requires no replay, including at a topology change. This bootstrap rule is our adaptation; CMC's client-move delta-time calculation does not replace our global fixed-step scheduler. |
| Simulation progress, frame batching and wall time | [Fixed tick state](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Plugins/NetworkPrediction/FFixedTickState) allows multiple simulation ticks per engine frame and separates confirmed/pending frames, authority-to-local offset, simulation step, accumulated real time and optional time dilation. | Use timestamped committed ticks for prediction and combat time. Preserve fractional phase across steady committed progress; sampling an integer tick later within its interval must not pause remote playback. Genuine authority stalls still constrain advancement. Prediction catch-up must not author new input for committed ticks or consume queued action edges. Preserve ordinary batched input and keep transport clock/RTT diagnostics separate. Our phase estimator, substitution forecast and replay bound are local choices; they are not Epic's time-dilation algorithm. |
| Input retransmission | [Network Prediction settings](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Plugins/NetworkPrediction/FNetworkPredictionSettings) specify sending several recent commands per update. | Test the same bounded redundant packets as production, with immutable command identities and targets. Eight records is our setting, not an asserted Unreal default. |
| Selective old-command retry | [Saved moves](https://dev.epicgames.com/documentation/unreal-engine/API/Runtime/Engine/FSavedMove_Character) identify important unacknowledged moves to resend; the [packed move container](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Runtime/Engine/FCharacterNetworkMoveDataContain-) includes an optional important old move. | Give retained commands a bounded resend opportunity when committed progress makes their unchanged targets eligible, including commands displaced from the newest-eight packet. This progress-triggered rule is our adaptation; the cited CMC docs do not specify a standalone resend timer during suspended generation. |
| Receipt, consumption and feedback causality | [Fixed server receive data](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Plugins/NetworkPrediction/TServerRecvData_Fixed) separates received and consumed client frames from the server's pending frame. [CMC server prediction data](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Runtime/Engine/FNetworkPredictionData_Server_Ch-) separately tracks processed timestamps, received timestamps including rejections, and time discrepancy. | Transport receipt cannot finalize gameplay. Feedback identifies the observed command; the client distinguishes commands issued before and after a lead change. Count unique missed deadlines even when their old-policy feedback cannot tune the new policy. Our minimum-slack aggregation and policy boundary are local controller rules, not Epic-documented formulas. |
| Starved remote interpolation | [Fixed interpolation service](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Plugins/NetworkPrediction/IFixedInterpolateService) requires valid frame continuity and explicit starvation handling; it stores server frame numbers independently of the prediction offset. | Render from authoritative sample history in simulation time. Preserve scope/segment continuity and bounded starvation behavior; additional prediction cannot create missing remote authority. |
| Remote motion and animation | [CMC's simulated-proxy smoothing](https://dev.epicgames.com/documentation/en-us/unreal-engine/understanding-networked-movement-in-the-character-movement-component-for-unreal-engine) separates replicated movement from visual cleanup. | Enemy gait measures travel between presented positions over the matching display-frame duration, before transform correction. The owner's integer prediction tick does not measure remote travel. Veil shutters interpolate continuous authoritative geometry, with the same motion recorded in combat history; regular endpoints preserve the combat segment. The gait sampler and 30-tick shutter travel are Dreamwake choices. |
| Recovery buffers | [Network Prediction settings](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Plugins/NetworkPrediction/FNetworkPredictionSettings) expose interpolation buffering and a cap on input buffering after faults. | Keep admission, prediction history and recovery work explicitly bounded. The 12-tick lead/admission and 32-tick replay limits belong to this implementation and need direct local qualification. |
| Impaired-network verification | [Network emulation](https://dev.epicgames.com/documentation/en-us/unreal-engine/using-network-emulation-in-unreal-engine) covers directional lag/loss, jitter, reordering and duplication; ideal local connections conceal defects. | Deterministic fixtures must exercise actual command generation and model frame batching, startup checkpoint age, reliable/unreliable lanes and redundant packets. Real client/server tests must match the intended build profiles, replication workload and render configuration as well as impairment. Measure frame duration separately from transport RTT: slow frame processing can exhaust command-arrival headroom without oscillator drift. Dreamwake qualifies captured native release artifacts at its normal 80 m radius separately from its 12 m disclosure fixture; these are local qualification choices. A short moderate-profile pass does not qualify every impairment case. |

See [netcode](netcode.md) for the current algorithms, configuration and validation
commands. Before changing a mechanism, update this mapping when its Epic basis or
local difference changes; do not append a history of failed attempts.
The [clock/feedback regressions](../engine/net/src/synchronization/committed_tests.rs),
[client pacing regressions](../games/dreamwake/client/src/plugins/network/target_horizon_tests.rs)
and [client command-generation regressions](../games/dreamwake/client/src/plugins/network/feedback_causality_tests.rs)
exercise clock and input boundaries. The
[bootstrap topology regressions](../games/dreamwake/client/src/plugins/network/bootstrap_topology_tests.rs)
separate adoption without local history from bounded replay after prediction starts. The
[authority outcome regressions](../games/dreamwake/server/src/authority/outcome_tests.rs)
cover committed timing and immutable outcomes. Test results and real transport captures are
separate evidence from the reference designs.

## World coordinate integration

Dreamwake's two-element positions are **[X, Z]**, with +Y up. Adapt the reference
model's planar XY notation to XZ once at the adapter boundary. Influence cells
use floor((axis - origin) / cell_size) on X and Z, including negative positions.
Coverage includes cull radius, bounds, leave margin and permitted prefetch.
Exact distance/disclosure tests follow broad-phase gather; vertical distance
must be included when actors gain height. Do not interpret the reference TOML
`sparse_xy_influence_cells` label as an instruction to use Bevy's vertical Y.
The integration's explicit grid identifier is `sparse_xz_influence_cells`.

Update membership when influence changes even within the same center cell.
Reject non-finite coordinates, excessive footprints or allocation caps before
mutating the previous registration. Oversized actors require an explicitly
approved coarse node; there is no unbounded global fallback. Gather work visits
candidate lists and connection-known pending state, not every actor per client.
When validated influence-cell coverage is unchanged, pose/version/policy and
dormancy still advance at the committed barrier while the reverse memberships are
retained. Exact distance checks use the updated pose, including vertical motion;
an unchanged cell rectangle does not imply unchanged eligibility.

Public canonical payload reuse is bounded to one committed capture. The cache
requires the current opaque eligibility result before lookup and keys exact entity
generation, state/scene revision, schema, field selection, representation revision
and prediction permission, within the frame/tick/world/policy barrier. A cache hit
still passes through the recipient's scope authorization. Capacity exhaustion
encodes normally without retaining more bytes; every new capture clears the cache.

Dreamwake opts in only global, collision, public hero/enemy, damage, cover and
platform payloads. Owner checkpoints, source-dependent projectile/wisp/effect
views, marker parent-scope records, connection baselines and encrypted packets
remain recipient-specific. Cache counters distinguish hits, encodes, bypasses and
retained allocation bytes; reduced encoder calls do not establish lower wall time.

## Dreamwake routing and disclosure

This table defines scoped representations. The live adapter uses positively
selected public and owner DTOs. The complete offline `DreamSnapshot.state`
remains serialized despite being Rust-private, and `select_player` changes display
fields while retaining every saved actor. Never feed that offline payload to a
scoped encoder. New representation schemas must select fields positively and
reject unregistered fields/classes. Unknown event classes have no audience by default.

| Class | Route and audience | Representation and prediction policy |
|---|---|---|
| Run/session summary | Global within admitted match | Public phase, room/realm, encounter label, kills, elapsed, lucid, paused, cleared, active party count and fixed approved phase message; no arbitrary server message, RNG, allocator or future encounter state |
| Hero | Owner connection plus spatial dynamic route | Owner receives complete permitted prediction state; others receive public pose, health/shield, level, visible action and cosmetic state. Private loadout, cooldowns, rewards, XP, shards and tuning stats remain owner-only |
| Collision scene | Explicit required dependency of the admitted owner | Bounded immutable collider descriptors and movement profile, with scene revision and canonical identity. The owner, public summary and collision scope enter as one complete same-tick group. No actor state or world seed |
| Rotating platform | Spatial dynamic, plus complete required owner dependency | Stable collider generation, scene/motion revision and bounded deterministic trajectory; admitted history survives leaving render interest. See [moving bases](moving-bases.md) |
| Enemy | Spatial dynamic; explicit dormant route only when immutable | Visible pose, kind, health, committed windup/target/warning, phase, slow/flash. AI decision state, attack index and random state are server-only; interpolate by default |
| Projectile | Spatial dynamic, owner reason for admitted predicted projectile | Visible identity, permitted owner reference, pose/direction/radius/faction/essence. Full collision/lifetime/hit history only in an authorized complete prediction group |
| Wisp | Spatial dynamic and entitled owner connection | Visible pose/lifetime/essence and permitted owner reference. Orbit/fire/target state requires complete authorized dependencies for prediction |
| Delayed cast | Owner connection only when prediction is admitted | Scheduled payload remains private; publish a separately approved visible warning when game rules expose it |
| GraphicsInstance | Independently approved spatial event audience, plus entitled predicted owner | Effects from independently eligible heroes and enemies retain stable identity and geometry. Hidden-source radial pulses inside the arena use an anonymous 8 m cell proxy; hidden beams/orbs and prohibited origins are omitted. Proxy identity contains no source or command identity |
| DamageNumber | Spatial event audience | Approved amount, critical/friendly flags and visible position/lifetime. No hidden source/target identifiers or exact hidden location |
| Reward/menu action outcome | Owner connection | Reward options, selection/ready outcome and terminal action result; shared party readiness is a separate minimal semantic representation |
| Veil cover region | Spatial current state, retained after authority removal | Geometry, open state and present/absent flag; current absence survives complete authority restore and scope re-entry. No client removal command or transition schedule |
| Cover marker | Independently eligible spatial public attachment | Exact parent connection/entity generation/scope/representation, local offset and current phase. Stage until that exact present cover is published; never acquire carrier inventory visibility |
| Future props and attachments | Denied until registered | Must declare bounds, static/dynamic/dormant route, scene revision, fields and dependency behavior before admission |

`EventAudience` binds each approval to the connection and current policy revision;
anonymous projection cannot bypass an event-origin denial. Its result still goes
through the existing graph grant and scope barrier. Independently delivered
source-dependent roots remain in bounded `ClientScopes`; render assembly withholds
them until their source actor is published, including after a source exits.
This withholding preserves payload, epoch and duplicate-identity validation and
does not block the complete owner view or grant visibility to the source.
`Attachment` resolves against
published `ClientScopes`, including parent generation and scope incarnation, and
permits a standalone transform only when the game explicitly supplies one.
`RegionManifest` carries bounded current map-slot presence through ordinary full
state. Missing regions, missing slots and mismatched generations remain unavailable;
they never authorize reconstruction of a cached map default. Dreamwake's
45 authored veil slots each use a retained cover root as their region descriptor.

Initial spatial classes use the profile's 80 m base cull limit, 10 m leave margin
and 8 m maximum prefetch; effect/projectile bounds come from authoritative radius,
not client geometry. Hero/enemy/wisp registrations require a finite conservative
authoritative bound supplied by the game adapter. No guessed renderer size is
accepted as gameplay bounds. Class-specific narrower radii may be configured;
larger radii must pass the same 256-cell coverage cap. Owner/global reasons
bypass distance only. They do not grant private fields to other connections.

Observers initially consist only of the server-owned pawn position (one per
connection). The cap of two is capacity, not a grant for a second camera.
Spectator, remote camera, split-screen and client-supplied arbitrary positions
are disabled until an authenticated, ticked grant defines allowed identity,
offset, scene readiness, update rate and expiry. Grant changes revoke old fields
at the policy barrier. In the existing open cooperative arena, authorized
spatial public state does not imply an occlusion/anti-wallhack guarantee.

Required dependencies bypass distance but never disclosure. Closure uses stable
IDs, a visited set and atomic group admission. Missing, denied, oversized or
unavailable historical dependencies disable the whole prediction group. Send an
independent allowed presentation replica and continue authoritative execution;
do not copy secret AI/RNG state, invent a dependency, or truncate a group. In
particular, all-world DreamSimulation replay cannot run on a filtered snapshot.
Dreamwake uses a restricted owner model sharing the authority's owner intent and
transition functions; remote AI and hit resolution stay authoritative. New groups
need their own restricted systems and complete permitted dependencies. Replay
retention has its own bounded immutable
samples, separate from current scope membership. Revocation invalidates affected
prediction even if an old history sample exists.

`PredictionManager::replace_groups` atomically retires an exact set of current
prediction identities and accepts complete replacement `PublishedGroup` proofs
at one authoritative tick. Every manifest member has one writable group;
unaffected groups keep their state, commands, dependencies, and work accounting.
Decoded replacements coexist with old histories under the aggregate memory
budget until admission succeeds. Bounded identity high-water metadata prevents
stale group revisions, owner streams, scenes, and entity generations from
returning after a merge and split. Incompatible histories are discarded, and the
result explicitly reports an authoritative rebase: it supplies no invented
commands, dependencies, or replay. Keep the connection's existing `EventJournal`
through this operation. Post-checkpoint absence is speculative; confirmed events
and one-shot delivery receipts remain governed by that journal.

## Field classification and restoration

All authoritative saved fields are server-written. The table covers the current
saved structs in `simulation/src/state` and the engine components they contain.
“Restore” means complete same-tick checkpoint restoration, including zero/false,
empty collections and component absence; no non-default-only codec. “Owner”
means an entitled owner representation, not all members of a cooperative party.
A field being restored on the server does not authorize its delivery to clients.

Dreamwake players and enemies do not block player movement. Actor crowding may
produce visible overlap; it produces no separation impulse or owner correction.
Movement collision reads the admitted immutable environment, never delayed
remote presentation transforms. The eight-player crowding test compares shared
lane crossings and exact overlaps with a single-player authority, then replays
from five delayed owner checkpoints and requires identical motion. This policy
does not predict actor pushes or damage from remote collisions.

| Field family | Prediction / disclosure | Codec and restore contract |
|---|---|---|
| Run.tick, seed, rng, next_id | Tick public; seed/RNG/allocator server-only | Exact integer values in full server checkpoint; scoped predictor must use approved entity streams/identities instead |
| Run.phase, room, encounter_name, kills, elapsed, lucid, paused, cleared, message | Public summary; phase gates predicted stepping | Exact enums/integers/bools, finite elapsed, bounded UTF-8; restore authoritative values |
| Run.rewards, party_size, reinforcements | Rewards owner projection; party count admitted semantic summary; future reinforcement state server-only | Bounded collections and exact scalar restore; no future-state leak |
| HeroBody.id, shards, combo, hit_flash, attack_flash, attack_power, ability_power, movement_speed, critical_chance, recovery, defense; Hero.ready/rewards | ID and visible combo/flashes public; remaining stats/rewards owner; ready via minimal party policy | Exact scalar/bounded reward state; flashes remain simulation-owned until explicitly migrated to presentation |
| Hero.critical_rng (entity/domain key and counter) | Owner prediction only; never part of the public hero representation | Exact saved stream state with checked counter exhaustion; unrelated actors and cosmetic randomness cannot advance this stream |
| Hero.active | Server readiness gate, included in owner checkpoint; inactive heroes have no public actor representation | Exact bool in offline and owner checkpoints; activation commits only after initial group decode proof, and the owner decodes the active checkpoint before issuing commands |
| Health.hp/max_hp; CombatState.shield/shield_remaining/invulnerability_remaining/statuses(id,magnitude,remaining) | Public health and visible shield/status projection; owner complete defense state | Finite f32 and bounded status list; restore timers as well as amounts |
| Hero KinematicState.position/velocity/facing/stance/ground support/jump and coyote timers/dash state/movement lock/snap suppression/base state | Owner predicted; remote XZ pose, elevation, crouched stance and visible dash only | Exact canonical XYZ values with +Y up, tick counters, contact identity and complete base state; validate against the group's exact immutable collision scene and restore every field |
| Enemy MotorState.position/facing/velocity/movement_lock/dash_direction/dash_remaining/dash_cooldown/dash_speed | Server-only motion; remote pose and visible action | Finite canonical XZ f32 arrays and timers; complete offline restore |
| SavedState collision manifest | Server-owned immutable map/profile; approved owner dependency | Full offline checkpoints restore the actual bounded manifest. Live owner checkpoints bind its canonical identity and require the matching entered collision scope; replay never substitutes the renderer's current geometry |
| ActionState.target/windup/recovery/sequence/due | Owner predicted; enemy committed warning and public phase only | Exact sequence/flag and finite target/timers, including due flag |
| AbilitySlot.kind/level/modifier/cooldown/max_cooldown and Loadout vector | Owner predicted/private; visible cast is separate public effect | Bounded slots, exact enums/level and finite timers; restore all slots |
| Progression.level/xp/xp_next/pending_levels | Level public, remainder owner; server decides rewards | Exact level/count and finite XP values, restore pending levels |
| EnemyBody.id/kind/warn_radius/phase/hit_flash; Enemy.attack_index | Visible body public; attack_index server-only | Exact enum/integers and finite floats; complete server restore |
| Projectile payload.damage/essence; ProjectileState.id/owner/faction/position/previous_position/direction/speed/remaining/radius/hits_remaining/hit_ids/max_distance/source_policy/expired/pending_impacts(target_id,distance) | Public view only by default; full state only in approved owner group | Restore all saved fields, bounded hit/impact lists and optional distance; rebuild runtime owner relationship |
| Wisp payload.damage/essence; CompanionState.id/owner/faction/position/remaining/orbit_angle/orbit_radius/orbit_speed/fire_remaining/fire_interval/range/expired/pending_shot | Public view by default; full state requires permitted target dependencies | Finite floats, exact optional shot and identity; restore all fields and owner relationship |
| DelayedAction.remaining/ready/payload; CastPayload.action_sequence/presentation_slot/owner/id/kind/essence/power/origin/direction | Owner-only predicted if dependencies allowed | Restore complete delayed schedule and payload; never reallocate identity on replay |
| GraphicsInstance.id(match_epoch,owner,action_seq,slot,scope)/kind/pos/radius/age_ticks/duration_ticks | Simulation-owned, public only under effect policy | Exact stable identity and ticks, including source/control epoch, stream and generation for scoped input; replay regenerates the same identity |
| Number(DamageNumber.id/position/amount/critical/friendly/age) | Simulation-owned presentation, authorized event audience | Bounded lifetime state; finite values and stable ID; restore while part of SavedState |
| DreamSnapshot display fields, HeroView/EnemyView/ProjectileView/WispView and MemorySlot | Derived presentation projections | Rebuild from restored components; never write them back as parallel authority |
| DreamInput.movement/aim/attack/dash/casts/action_sequences | Client intent, server validated; immutable replay journal | Finite normalized axes, exact bounded flags/sequences; no live input during replay |
| Bevy Entity owner links, target indices, renderer meshes/materials/offsets | Derived runtime/presentation state | Excluded from wire; rebuild links/indices from stable IDs and keep render offsets outside simulation |

Live command/control envelopes use bounded bincode. Group manifests and scoped
headers use explicit binary fields. All positive Dreamwake payload roots
(global, owner, collision, hero, enemy, projectile, wisp, effect, damage, cover,
platform and cover marker) use
`engine_net::schema!` field codecs with explicit stable IDs, exact canonical
values, units, visibility and prediction metadata. Nested bounded sequences,
UTF-8 strings, options and tagged enums participate in the same fingerprint and
preallocation budget. Registry construction proves the maximum encoded root fits
16,374 bytes; the protocol adds bounded framing. The content handshake includes
the sorted registry fingerprint through `replication::schema_identity()`.

The tagged canonical codec remains the inspection and schema-probe format.
After exact protocol and registry negotiation, live public actors use the same
generated fields and validators in compact schema order. Fixed arrays omit
redundant counts and element lengths; bounded variable collections retain their
counts. Compact decoding still stages complete values, charges its work and
allocation budgets, and rejects trailing bytes. It is not a self-describing
format and must not be accepted before the matching content handshake. Scoped
live headers use canonical variable-length integers, retaining all identity and
tick fields while obtaining the connection epoch from the authenticated frame.

Capture and staged restore convert every field to and from game models, then run
game cross-field validation before replacement. Owner dependency policy 1 is
external: authenticated group assembly must bind the current global state and
collision identity at the common tick. A schema declaration alone does not grant
access or close that dependency graph. Public roots cannot include owner records;
private change diagnostics require an owner grant. Complete offline checkpoints
retain their separate versioned codec.
The reproducible codec probe is
`python3 scripts/qualify-schema.py --output out/network-qualification/schema`.
It compares native and Node WebAssembly output for all twelve canonical roots
and all nine compact public roots, and writes source hashes plus a browser
worker bundle. Serve that output directory, open
`schema-browser.html` in the built-in browser, and press **Run schema probe** to
verify the browser runtime. The page reports only canonical digests and a boolean
result; the generated manifest records browser verification as pending.

Decoders reject non-finite gameplay values and validate complete group manifests
before mutation. Round-trip f32 values preserve their exact value after canonical
zero normalization; no lossy quantization is silently introduced. A future
quantizer must define rounding/range behavior
and run identically before authority/prediction comparisons. This choice alone
does not prove cross-platform floating-point determinism.

## Ruleset and qualification envelope

The [arena profile](../todo/config/arena_profile.toml) supplies proposed limits;
the [scale profile](../todo/config/spatial_scale_profile.toml) is an explicitly
merged synthetic overlay. Startup must validate types, finite ranges and
cross-field budgets. Never use browser query/fragment configuration. Canonically
encode the effective configuration plus map/collision/schema revisions, game
catalog values and the following policy identifiers into a ruleset hash; reject
handshake mismatches. A TOML file's bytes or an unimplemented hash is not evidence
of negotiated compatibility.

Freeze the proposed compensated-combat policy as historical targets and dynamic
cover at validated query time, unchanged static walls always blocking, ticked
shield/invulnerability at that same query time, and an authoritative eligible
shooter at execution time. No interpolation crosses teleport, respawn or
ownership/lifecycle discontinuities. Equal-time eligible damage is batched and
allows trades. Maximum rewind is 150 ms; missing history rejects compensated
validation explicitly. Late finalized actions reject; sub-tick processing and
projectile catch-up are disabled. Dreamwake's ray and beam path implements this
policy; melee, area attacks and projectile flight use committed simulation time.
The ruleset identity automatically fingerprints the complete engine and simulation
sources, Cargo manifests, lockfile and compiler version. Target paths and build
timestamps are excluded so supported native/WASM build profiles can agree.

The supported development path is the existing Bevy 0.19.1 / Rust 1.95-or-newer
native/headless Dreamwake simulation and its existing native/browser transport
adapters, with dependency versions fixed by Cargo.lock. No new production
spatial/prediction quality envelope is qualified by this document. The initial
qualification target is an eight-player 60 Hz kinematic arena, 30 Hz owner/remote
publication target and bounded replay. Dynamic rigidbody rollback, cross-platform
bit identity, spectator timelines and unrestricted large-world scenes are
unsupported until their own tests establish them.

Reference server and minimum client hardware are **unassigned**. Qualification
must record CPU model/core budget, RAM, OS, compiler/build flags, client GPU,
resolution/refresh and browser/version where applicable before reporting latency
or capacity. Required impairment profiles and exact acceptance cases are in
[specification §22](../todo/ENGINEERING_SPEC.md#222-required-network-profiles)
and the [acceptance matrix](../todo/planning/acceptance_tests.csv).

The declared targets remain unqualified: server p99 below 8 ms and p99.9 below
the 16.667 ms tick deadline, client replay p99 below 2 ms, static correction
p95/p99 below 0.02/0.10 m, and 24-hour soak. The 100-connection/50,000-potential-
actor sparse/dense/dormant/wake/teleport workloads are separate scale gates,
not supported player counts or performance results. The [persistent resource soak workflow](network-soak.md)
records actual wall time and immutable source evidence separately from game/transport qualification.
The [game-soak workflow](network-game-soak.md) prepares persistent native UDP bots,
the [clock workflow](network-clock-qualification.md) prepares controlled clock
drift and rational deadline checks, and the [manual profile workflow](network-profile-qualification.md)
provides external impairment configuration. Their commands produce evidence;
their presence is not a qualification result.
Two separated real clients
must receive different authorized sets, recover from lost entry/exit/wake and
retain complete dependencies before the prediction vertical slice passes.
Python contract tests establish only their modeled contracts, not these gates:

```sh
python3 -m unittest discover -s todo/reference_model -v
```

Run `python3 scripts/qualify-graph.py` for the actual Rust graph, scopes, group
codec and scheduler workload. It compares sparse gathers against an independent
visibility oracle and records timings, allocation accounting, bounded entry
resets and dense overload under `out/network-qualification`. Its immediate
synthetic transport excludes socket behavior, game simulation and rendering;
those measurements require the separate process and browser workflows.

For bounded component measurements, run `python3 scripts/qualify-graph.py
--probe maintenance` or `--probe canonical`. The maintenance probe performs all
50,000 entity upserts plus 100 observer updates in each of five version-only,
within-cell-motion and cell-crossing frames; oracle work is excluded from timers.
The canonical probe compares identical bytes across sparse, shared dense, dormant
and waking layouts with five frames each. These probes have a four-minute runtime
ceiling, record source/build/hardware evidence, and remain separate from the full
scheduler/transport workload. Five observations cannot establish tail-latency or
30 Hz capacity. Distance-frequency throttling remains deferred: measured
cell-crossing maintenance dominates delivery work, and postponing public sends
cannot reduce that cost. The game retains its current cadence and critical
owner/combat freshness rules while geometry maintenance and feasible-load
scheduling remain separate qualification gates.

To measure the actual game's positive JSON encoder separately, create an output
directory and set `DREAMWAKE_CANONICAL_PROBE_OUTPUT` to an absolute CSV path, then
run `cargo test -p dreamwake_server --lib canonical_dream_json_probe -- --ignored`.
This opt-in five-frame, eight-peer probe alternates cached/uncached order and
asserts identical bytes. Its default test profile is debug; it excludes capture,
gather, owner payloads and transport. Cheap synthetic field packing can cost less
than a cache lookup, so codec-call savings alone do not justify enabling reuse for
an arbitrary payload type.

Dependency pins and source exclusions are recorded in
[network dependencies](network-dependencies.md). Any new actor or state field
must acquire an explicit row/policy before its codec can admit it.
