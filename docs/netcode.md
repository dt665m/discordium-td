# Networking and authority

`engine_net` provides typed timelines, bounded codecs, immutable target-tick
commands, persistent interest graphs, scoped replication, byte scheduling and
transactional prediction. Games supply simulation rules, representation policies,
payloads, readiness requirements and admission policy. `PeerInbox<I, A>` remains
available for simpler integrations; Dreamwake uses the target-tick command inbox.

The [network integration contracts](network-contracts.md) define persistent graph
gathering, scoped delivery and immutable target-tick input.
The graph and scope primitives live in `engine_net::interest` and
`engine_net::replication`. Dreamwake's live server and client use those APIs;
the offline complete-world checkpoint has a separate purpose and codec.

`just network-plan` reads the supplied implementation backlog in milestone order.
`just network-foundation` runs the available kernel/simulation contracts and the
versioned replay runner. `just network-qualification` also runs workspace and WASM
checks. Reports, logs and replay fixtures are generated under
`out/network-qualification`, with the original 157 acceptance requirements,
source/lock identities and machine information. Production gates require their
own real-process, impairment, browser, hardware and soak evidence.

The F3 action row shows the owner's immutable sample/view times, target tick,
terminal verdict and the first observed checkpoint/presentation timeline. A
timeline reaching a tick alone does not prove an effect was displayed. Native
`--network-trace <new-file>` includes recent owner action records with public
scope diagnostics. Server `--action-trace <new-file>` enables restricted local
shot inspection, including historical geometry and shield credits; it has no
HTTP or peer endpoint. Join the two artifacts by server instance and action key.
Files must be new, are limited to 8 MiB each, and use owner-only permissions on Unix.
The default is one file. Native qualification can opt into a finite number of
segments and a slower sample interval using the [game-soak workflow](network-game-soak.md).
Output exhaustion or an I/O failure produces an explicit error and a nonzero
client exit instead of silently losing the rest of the trace.
Server traces retain at most 4,096 records within an 8 MiB memory budget; exports
report gaps when older records are evicted. Disable tracing to clear its memory.

`python3 scripts/qualify-replay.py` builds the same bounded verifier for native
and browser WASM. It records synthetic combat and wall/dash fixtures, checks
native replay and continuation after every checkpoint restore, and writes a
local browser page with hashed artifacts. Serve the reported output directory
over loopback HTTP and use its Run verification button in the built-in browser.
The browser compares full canonical checkpoints and reports the first divergent
field. A native pass leaves browser verification pending; these fixtures do not
qualify other architectures, every gameplay trace, transport, or input latency.

## Server execution

The [engine server plugin](../engine/server/src/plugins/server/mod.rs),
`ServerPlugin<A: Authority>`, registers polling and shutdown around one non-send
`ServerDriver<A>`. The driver owns connection events, simulation dispatch,
publication cadence and packet generation. `ServerElapsed` can override elapsed
time for embedded hosts and deterministic tests; otherwise polling uses `Time<Real>`.
Time must not move backwards. `run_app` returns recorded failures after exit.

[DreamwakeServerPlugin](../games/dreamwake/server/src/plugins/server/mod.rs) supplies
game authority. Dedicated and native-hosted servers use
`dreamwake_server::build_app(shared, seed, lucid)`. Native clients use UDP; browser
clients use WebRTC through the same HTTP session service.

Dreamwake simulates at 60 Hz and offers authorized scoped state at 20 Hz.
Catch-up is capped at four simulation steps per poll; publication sends the latest
completed state. Rational deadlines derive from initialized tick zero. Scheduling
debt remains visible through `ServerDriver::health`, and an overloaded driver
rejects new admissions until it recovers. `SharedNet::health.report()` also
extrapolates the rational clock while the driver is stalled. `GET /readyz` returns
200 when ready or 503 while starting, overloaded or stopped, with committed tick,
remaining debt and poll age. New bootstrap requests receive 503 before credential
verification while readiness is closed. `/healthz` remains the HTTP liveness check;
existing signaling remains available. Games receive `Authority::update_health` to
gate creation of new matches; Dreamwake rejects Start/Restart during overload and
keeps the current match running. Games can implement `Authority::step_tick`
to consume canonical tick metadata. Socket polling frequency does not increase
simulation time or discard overdue authoritative ticks.

Netcode handshake confirmations retry at their transport cadence until the peer
confirms the connection. Application payload sends cannot postpone those retries;
this lets a lost initial confirmation recover while reliable game messages are
already queued. The owned dependency correction is recorded in the
[network dependency register](network-dependencies.md).

Dreamwake paces each connection at 160,000 bytes per second with a 16,000-byte
burst; 2,400 bytes of that burst are reserved for transport control. Its graph
scheduler uses the same rate. The reusable engine's generic defaults remain
60,000 bytes per second and a 6,000-byte burst.
The `--egress-bytes-per-second`, `--egress-burst-bytes` and
`--egress-control-reserve-bytes` flags override these values. Pacing uses monotonic
wall time and charges retransmissions and conditioned duplicates at the final
send boundary. Native UDP accounting includes IP and UDP headers. WebRTC counts
encrypted logical packets; browser-managed framing and congestion prevent an
equivalent physical-byte guarantee. `SharedNet::egress_stats` and
`peer_egress_stats` expose these two bases separately. These transport limits are
independent of the graph scheduler's estimated wire-byte allowance.

Dreamwake orders its reliable control channel before unreliable state in Renet's
packetizer. The transport reserve protects netcode control packets; application
control messages are still charged as encrypted payload. At each 20 Hz
publication, `Finalized` includes only committed receipts not already queued on
the reliable channel, in batches of at most 32. Finalization and action outcomes
each have a separate four-batch application receipt window. A validated
`FinalizedAck` or `OutcomesAck` acknowledges an exact sent batch endpoint; future
or unsent endpoints are rejected. Unrelated clock and lifecycle controls do not
consume either window. Unsent work coalesces in bounded authority storage.
Exhaustion requires recovery rather than omitted receipts. These delivery
acknowledgements do not mean a new command was executed or a checkpoint decoded.

The client combines validated public-state decode receipts in batches of up to
eight, flushing the final partial batch before the same receive pass ends. Owner
group receipts and baseline repairs are sent immediately. The server handles at
most 64 admitted messages per peer per tick and leaves excess reliable messages
queued for the next tick, so a legal burst of decode receipts cannot disconnect
the peer.

State publication reserves bytes for unsent control and timer-due retransmissions,
plus ACK work. Reliable messages already sent and awaiting acknowledgement do
not consume a fresh reservation every frame. Final egress still charges every
physical send, including retransmissions, under the hard transport pacer.
Owner-group fragment prefixes fit both transport allowance and the graph
scheduler's current refilled credit. This prevents optional traffic from
repeatedly consuming credit needed by an oversized frozen-group candidate.

The server's once-per-second `Dreamwake egress` log reports cumulative application
bytes/messages for control and state, finalization batches/receipts/deferred
publications, Renet queued message counts and retained payload bytes, and the
transport's independent native UDP/IP or WebRTC logical-byte counters. Healthy
samples use info level; new pacing drops or a full finalization control window
use warn level. Renet queue statistics include sent reliable messages awaiting
transport ACKs and unsent unreliable messages; they are not delivery proofs.

## Input and acknowledgement invariants

- Gameplay commands bind a connection epoch, command stream, ownership epoch,
  sequence and immutable target tick. Validate channels, counts and finite
  directions before admission. Redundant bundles retain the original commands.
  Each committed tick permits at most eight dequeued input bundles, reserving
  eight records per bundle against the 64-record admission budget. A stalled
  receive loop leaves excess bundles in the bounded transport queue for later
  ticks; duplicate records still consume work and message flood limits still apply.
- Hardware sampling journals held intent and one-shot edges separately. Missing
  commands use a bounded held-input grace; substitute ticks do not repeat edges.
  The atomic owner checkpoint includes the committed held-input state and missing
  streak used by that policy. Local substitution forecasts start from that same
  checkpoint and use the shared policy without creating network commands.
  Packets carry recent immutable commands. When authenticated committed progress
  advances, up to 12 older retained commands inside the admission window
  also receive a bounded retry, including frames that generate new input.
  Commands already packed in that pass are excluded. A repeated committed tick
  grants no additional retry; resending consumes no journal samples or sequences.
- Finalized input receipts follow committed simulation ticks. They are separate
  from transport receipt, decoded scoped-state receipts and action outcomes.
  A high received sequence cannot acknowledge a command that never executed.
  Arrival feedback uses the minimum slack among newly accepted or newly late
  commands since publication. The bounded admission audit excludes duplicate
  late deliveries while retaining first late arrivals below a newer sequence.
  Each sample identifies its command and arrival tick. The client changes lead
  only from commands that could have benefited from its current lead policy;
  earlier samples remain lateness evidence. Cumulative unique-observation and
  late-command counts remain visible even when a sample cannot tune lead.
  Advancing finalization alone does not establish that a sample measures a new
  policy.
- Menu actions use a separate bounded reliable sequence and at most one menu
  action per player per tick. An unavailable choice is still finalized.
- Epoch resets clear pending commands and acknowledgements. Stale generations,
  scope epochs and representation revisions cannot replace current client state.

Dreamlance and channeled beams capture an immutable estimated sample time E,
displayed remote pose time R and exact decoded owner reference at the hardware
edge. Command target C and first accepted server arrival A remain separate.
These stamps and combat history use committed simulation ticks; server scheduling
debt does not convert arrival into a later wall-clock tick.
The server intersects the retained reference, clock and arrival bounds with a
150 ms rewind policy; it does not subtract RTT again. E and R carry a `u16`
fraction in units of 1/65,536 tick, rounded downward once at capture. The remote
combat cohort must agree on its exact displayed time, including starvation
freezes; sharing only an integer tick is insufficient.

Movement remains one fixed step. For a hardware fire edge assigned command C,
the muzzle is reconstructed between the authoritative combat poses S[C−1] and
S[C] using E's captured fraction. Command lead means E and C need not be the same
tick; the fraction names the phase within C's command interval. Ammo, cooldown
and current eligibility are checked at C. Targets and cover use R's exact
adjacent history endpoints and fraction. Interpolation requires unchanged scene,
actor generation, segment and collision shape (including stance); missing or
changed endpoints reject without spending ammo. A zero fraction selects its
exact endpoint. Discrete defense state belongs to the left endpoint until the
next fixed tick, and accepted damage still commits against the current lifecycle.

The adapter rejects a requested view that would need policy/history clamping.
Adjacent server ticks mapping to the same paused gameplay tick select that one
exact gameplay pose; missing or nonadjacent mappings reject. Beam pulses retain
their fixed-tick cadence and current-tick muzzle, using the latest accepted
fractional target view. Fractional stamps grant no extra movement, cooldown,
resource or per-player action opportunities.
Stationary combat actors and cover receive periodic timestamped state so their
displayed history can remain coherent with moving targets.

The shared simulation stages equal-time damage before applying it in stable
action order, permitting trades. Historical shield episodes keep monotonic spent
credits; a late hit cannot consume a newly activated shield or spend an old shield
twice. Veil shutters use their historical moving geometry for rays and beams;
projectiles sweep their current geometry before applying an impact. Each shutter
rests for 90 ticks and travels for the final 30 ticks of its 120-tick phase. The
saved schedule and committed combat tick determine one height for both public
presentation and collision. Normal phase changes preserve interpolation continuity;
removal still ends the collision segment. Held beam samples never inherit the
missing-input grace of ordinary held movement. Terminal action outcomes are
reliable and idempotent, independently of input finalization. Retained client
lookups survive reconnects within one server
instance and clear when the authenticated instance identity changes.

## Admission and readiness

`engine_server::start_with_admission` uses renetcode secure connect tokens and a
game-supplied `SessionAdmission` verifier. Dreamwake uses registered Traveler
profiles for the `dreamwake/default` service and match. A profile proves possession
of a saved random secret; it does not establish an external account identity.
Tokens bind protocol, endpoint,
client ID, expiry and application grant. Native and browser clients require secure
bootstrap and cannot fall back to an insecure connection. Bootstrap delivery
requires HTTPS or loopback HTTP; trusted LAN HTTP requires an explicit server flag.

The Welcome binds content identity and owner identity. The client validates both
before Ready. A joining hero remains inactive until the required initial owner
group has exact decode receipts. Activation commits on a simulation tick, then
the client receives and proves the active checkpoint before enabling commands.
Pending players cannot move, receive rewards, take damage or block party readiness.
The advertised owner identity survives ticks between Welcome and Ready while
its connection grant remains absent. Initial synchronization has a ten-second
absolute deadline. If an immutable initial owner group expires after two seconds
or a loading client outlives the bounded unsent finalization audit before it can
acknowledge application messages, a never-activated owner can receive a fresh
stream and Welcome, sharing the
resynchronization limit of three requests per sliding thirty-second window.
The bounded window survives stream replacement; healthy long sessions can recover
again after earlier attempts expire. Recovery preserves the original deadline
and does not change health, invulnerability or participation. Expired groups and
old stream proofs cannot activate the replacement stream. This retires the old
audit namespace rather than skipping missing receipts. Already participating
owners retain strict finalization continuity and cannot use this initial retry.

### Stable Traveler IDs

Open **Traveler ID / Settings** from the lobby or pause menu. Disconnect before
editing the ID, then use **Save & Join**. Valid IDs are canonical decimal integers
from `1` through `9223372036854775807`; Dreamwake reserves the high bit for world
entities. The client and server enforce the same parser, including for native
`--player-id` arguments. Browser persistence stores numeric IDs as strings to
preserve values beyond JavaScript's exact integer range.

First registration binds an ID to a random 256-bit reconnect secret. The client
saves that secret locally and never displays or replicates it. The server stores
only its BLAKE3 digest. Knowing an ID does not authorize control. A second live
connection for the same valid profile is rejected without evicting the first.
Reconnection requires the saved secret and receives fresh connection and ownership
epochs; old commands cannot control the rejoined traveler.

Disconnect retains the current run's health, loadout, progression and reward state
in an inactive hero. The absent hero cannot participate in combat or hold up party
readiness; reconnect does not heal it or refresh invulnerability. Owned transient
effects are retired. Starting a new run resets the connected party and prunes
absent participants. Identity registration survives server restart; current-run
gameplay state remains in memory.

Native profiles default to the platform user configuration directory under
`Dreamwake/travelers.json`; `--profile-file` selects an isolated test profile.
Browser profiles use local storage for the game's origin. Saves hold an exclusive
browser lock while reading and merging the latest profiles, so concurrent tabs
cannot replace each other's credentials. Each tab remembers its selected ID in
session storage and restores it on reload. A new tab initially selects the last
used profile; choose a different ID to run a second traveler. Registration defaults
to `Dreamwake/server-identities.bin` in the server user's data directory;
`--identity-store` selects a deployment file. One server process owns that file
at a time. Back up it and the client profiles to retain reconnect capability.
The client caps saved profiles at 64 and the server caps registrations at 4096;
live and retained current-run participant limits are separate.

## Scoped state and prediction

The server captures one committed simulation state and prepares the persistent
graph once per replication frame. Each connection gathers from global, owner and
spatial nodes under the game's disclosure policy. Only approved representations
reach encoding. Per-connection scope entry, exit, death, dormancy and decoded
receipts retain their own identities and independent capacity limits. Scheduler
priority and budget operate after eligibility and cannot grant disclosure.

Current component capture is shared with the full checkpoint writer. Live
publication validates current identities, collision, populations and nested
state, without cloning or round-tripping the offline combat archive. Full
checkpoint creation and restore retain the complete archive checks.
`cargo run -p dreamwake_sim --example replication_capture` runs a bounded
120-capture probe with two heroes, populated enemies and 32 history frames;
its timing excludes the rest of the server and does not qualify throughput.

Public actor views contain approved presentation data. Private loadouts,
progression and latent owner state appear only in that owner's checkpoint.
World RNG, allocation state, future encounters and complete server state are
absent from live public/global payloads. The owner, global summary, collision
scene and any required rotating base form one group, transferred in bounded
application fragments and published
atomically. Each member uses a bounded full or delta encoding against an exact
retained decode receipt in the same scope and baseline generation. All
reconstructed payloads and group identities must validate before any replica or
baseline is published and before a combined receipt is sent. Remote actors use
complete scoped states with compact headers: canonical variable-length integers
preserve every identity and tick, and the authenticated frame supplies the
connection epoch. Public actor fields use their agreed schema order without
repeating field IDs, fixed-array lengths or schema fingerprints in each update.
The content handshake must match the exact schema registry before this encoding
is accepted. Every value and the usual decode bounds remain validated. Protocol
identity changes require rebuilding clients and servers together.

State compression is configured at server startup with `--compression auto|off`
and `--compression-min-bytes N`. The corresponding environment variables are
`DREAMWAKE_COMPRESSION` and `DREAMWAKE_COMPRESSION_MIN_BYTES`; these also configure
servers started by the native client's host action. Defaults are `auto` and `0`: consider every
serialized state body immediately and use lossless DEFLATE level 1 only when its
five-byte compression framing plus compressed body is smaller than the one-byte
raw framing plus raw body. The minimum counts the serialized DTO and its
eight-byte vector length, before compression framing; it is not a UDP packet
size. No messages wait for aggregation. `off` always emits raw bodies, and both
forms retain the same bounded decoder and exact values. These options apply to
public actors and owner/global/collision payloads in dedicated and native-hosted
servers. For example, `--compression auto --compression-min-bytes 256` skips
compression attempts below that body size.

This uses DEFLATE with a configurable eligibility policy following Unreal's
documented minimum-size and immediate packet-processing approach. See
[Unreal's Oodle minimum-size setting](https://dev.epicgames.com/documentation/unreal-engine/unreal-engine-console-variables-reference)
and [Oodle Network's independent UDP processing](https://www.radgametools.com/oodlenetwork.htm).

Client retirement proposals retain their promised states until a reliable server
fence arrives. Missing bases request a rate-limited reset; a new generation needs
full seeds, and repeated loss cannot advance generations indefinitely. Exit,
revocation and identity changes close the relevant caches. Member caches share
explicit resident and staging limits separate from prediction history. Application
frame limits remain distinct from transport datagram and WebRTC overhead
qualification.

Reordered state is discarded as obsolete only when a retained scope, publication
or baseline fence proves it has been superseded. It cannot refresh connection
freshness, change displayed state or produce a decode receipt. Malformed payloads,
conflicting metadata and clock regressions remain hard decode failures.
A complete validated owner group for exactly the next baseline generation may
wait for its reliable reset under the same context. Waiting grants no retention
proof and does not refresh connection freshness.

Client prediction runs a restricted owner model using the same movement and
attack-acceptance functions as authority. It does not simulate remote AI or
restore the complete world. Complete group identities authorize bootstrap;
correction restores tick K and replays K+1 onward under aggregate memory and
work limits. Missing history, revoked dependencies and excess work request
bounded recovery. Before the first authored command, with no retained command or
substitute transitions, a fresh complete owner group may advance a pure
authoritative checkpoint without replay, including when the group topology
changes. Identity, dependency, memory and generation validation still apply.
Retained local transitions continue to require bounded replay. At bootstrap, or
when the predicted frontier has
fallen behind the estimated simulation tick, the client forecasts substitutes through the tick
before the fresh command target and authors input only for that target. These
forecast transitions retain replay history and consume the usual work and memory budgets;
they do not allocate command sequences, drain the hardware journal or create
action edges. When the frontier remains at or ahead of estimated simulation
time, ordinary contiguous batches retain their historical hardware deadlines.
Timestamped clock replies anchor a separate estimate of the
server's committed simulation tick; delayed checkpoints and finalization receipts
advance its proven lower bound without pretending they describe receipt time.
Adaptive command lead remains limited to 12 ticks, and the server independently
enforces its 12-tick admission window. The client bounds speculation to the
existing 32-tick replay budget beyond the latest proven committed tick. Replies
that repeat a stalled tick cannot refill that budget. Once exhausted, assignment
waits without consuming input edges or command sequences; the wall-clock estimate
and server debt remain visible. Repeated finalization controls do not apply
arrival-slack feedback twice. Attack sample stamps and remote display time use
the same simulation clock, including fractional tick phase. Newly assigned
commands use dependencies from the latest admitted checkpoint; already recorded
commands retain their original dependency
frames for replay. This refreshes bounded platform samples without rewriting
historical reads. Starfall flight and reversible graphics use the admitted owner
state; remote damage and hit resolution remain authoritative. The
[moving-base contract](moving-bases.md) describes platform dependency and rider
display time. Presentation consumes `DreamPresentation`, while offline replay
uses the complete `DreamCheckpoint` codec. See
[predicted graphics](predicted-graphics.md) for simulation-owned effect identity.

The current protocol identity is defined in the [game protocol crate](../games/dreamwake/protocol/src/lib.rs).
Bump it when serialized authoritative layout changes, and rebuild clients and
servers together. The engine must not contain a game protocol ID or game action enum.

## Diagnostics and validation

F3 shows passive performance and network metrics; F6 opens shared network tools.
Both are available in every build. Packet conditioning acts on real
transport traffic; see the [conditioner guide](../engine/net-debug/README.md) for
controls and RTT calibration. Server configuration also supports conditioning.

| Metric | Meaning |
| --- | --- |
| Frame time / FPS | Bevy's smoothed frame duration; FPS is 1,000 divided by milliseconds |
| Mean / worst frame time | Available samples from the last 120 frames |
| RTT | Renet transport round-trip estimate, separate from gameplay acknowledgement age |
| Snapshot / acknowledgement / reconciliation age | Milliseconds since the corresponding event |
| Pending inputs / replay work | Input-frame counts |
| Tick lead | Displayed tick minus latest server tick |
| Prediction stalled | Latest snapshot is over 300 ms old |
| Correction / reconcile shift | Client-view displacement in world units during reconciliation; positions may represent different ticks |

The renderer smooths correction targets without feeding those transforms back
into simulation. Remote pose history has a separate delayed presentation cursor.
Graphs leave unavailable samples as gaps; instrumentation must not substitute a
transport ACK for a decoded-state or finalized-input metric.

Run the affected short crate tests during development. Full campaigns, extended
matrices and soaks are manual handoffs after implementation and integration;
provide their exact commands, frozen inputs, duration, criteria and artifact paths.
Transport or browser changes also require WASM compilation and a real browser
session; unit tests do not establish deployed connectivity or performance.

`scripts/playtest-spatial.py --profiles moderate --duration 120` is a single
development spatial check with two native release clients and an explicit 12 m
replication radius. It builds the paired native release artifacts before launch.
Travelers alternate between
80-metre side waypoints and the center on the shared gameplay clock. Qualification
requires at least three observed 32-metre cells per Traveler, authoritative
same-tick separation, no fresh remote disclosure while far, a newer scope on
return, and bounded recovery through the end. The larger demo also contains 45
replicated props and 45 ambient enemies. An enemy acquires a living Traveler
within 24 metres and keeps pursuing that target outside the detection radius.
It reacquires a nearby target when its previous target leaves or is defeated.
Windup and recovery still lock movement, and the retained target survives full
checkpoint restore.
Enemy replication still follows each connection's spatial grant. Native traces
include only decoded public enemy scopes, with their positions and sample ticks.
F3 shows disclosed prop count and the current cell, and the F6
**Graph cells** control draws the grid. Use `--no-build --binary-dir PATH` for an
already frozen paired build with its matching release provenance; see
[native capture preparation](network-game-soak.md#captured-inputs).
The normal 80 m two-client workload needs its own short integration check before
manual endurance validation. The full profile matrix is an extended manual run.
Add `--pause-at 45` to the single 120-second check to stop its left client process
for three seconds. The runner requires an active new connection epoch within ten
seconds of resuming, with the same process and server instance and an advancing
authoritative checkpoint. It records the signals and before/after observations
in `processes.json`. Window focus, visibility, size and reported OS occlusion are
included in the native trace to help investigate rendering stalls.

See [deployment](deployment.md) for server configuration and release instructions.
