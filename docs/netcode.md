# Networking and authority

`engine_net` provides typed timelines, bounded codecs, immutable target-tick
commands, persistent interest graphs, an acknowledged per-connection entity state
stream, compact schema deltas and a rollback predictor. Games supply simulation
rules, representation policies, payloads and admission policy. `PeerInbox<I, A>`
remains available for simpler integrations; Dreamwake uses the target-tick
command inbox.

The [network integration contracts](network-contracts.md) define graph
gathering, disclosure and immutable target-tick input. The graph lives in
`engine_net::interest`; the state stream in `engine_net::replication::stream`;
the predictor in `engine_net::prediction::rewind`. The offline complete-world
checkpoint has a separate purpose and codec.

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
burst; 2,400 bytes of that burst are reserved for transport control. The reusable
engine's generic defaults remain 60,000 bytes per second and a 6,000-byte burst.
The `--egress-bytes-per-second`, `--egress-burst-bytes` and
`--egress-control-reserve-bytes` flags override these values. Pacing uses monotonic
wall time and charges retransmissions at the final send boundary. Native UDP
accounting includes IP and UDP headers; WebRTC counts encrypted logical packets.
`SharedNet::egress_stats` and `peer_egress_stats` expose the two bases separately.

Renet carries controls on its reliable-ordered channel and every state and input
packet on its unreliable channel, with Renet's stock resend timer. Controls are
packetized first. State and input messages are built to fit one Renet packet
(1,100-byte application frames), so a lost datagram loses one message and nothing
waits behind it. WebRTC data channels are unordered with a 150 ms
`maxPacketLifeTime`, and their MTU carries one netcode packet in one SCTP chunk:
SCTP drops a packet it could not deliver in time instead of queueing newer traffic
behind it. SCTP keeps its stock RFC congestion control. The SCTP timed-reliability
fix this relies on is recorded in the [dependency register](network-dependencies.md).

Publication stops where the connection's egress allowance ends
(`SharedNet::peer_egress_allowance`), as Unreal's `IsNetReady` does: state that
does not fit waits for the next publication and then goes out with its newest
value, so the server never queues stale state behind the transport. The browser
drops a send that would take the data channel's `bufferedAmount` past 64 KiB;
Renet resends reliable controls and the next input packet repeats unacknowledged
commands. Nothing else sheds, throttles or holds back traffic.

Stock Renet's serialized packet bound is 1,300 bytes. Netcode adds at most 25
bytes and base IPv6/UDP adds 48, for a 1,373-byte native data-packet allowance.

The server's once-per-second `Dreamwake egress` log reports cumulative control and
state bytes and messages, state entries, publications cut short by the allowance,
unacknowledged stream packets and the transport's native UDP/IP or WebRTC
logical-byte counters. New pacing or backend drops raise it to warn level; a
backend drop is a send the transport refused outright.

## Input and acknowledgement invariants

- Gameplay commands bind a connection epoch, command stream, ownership epoch,
  sequence and immutable target tick. Validate channels, counts and finite
  directions before admission. The server handles at most 64 messages per peer
  per tick and leaves the rest queued in Renet for the next tick.
- Hardware sampling journals held intent and one-shot edges separately. Missing
  commands use a bounded held-input grace; substitute ticks do not repeat edges.
  The atomic owner checkpoint includes the committed held-input state and missing
  streak used by that policy. Local substitution forecasts start from that same
  checkpoint and use the shared policy without creating network commands.
- Every fixed tick the client sends one unreliable input packet: its stream
  acknowledgement and up to six commands — the newest, the three most recent
  pending and the three oldest pending. Pending commands repeat until a state
  packet reports them finalized, so a lost packet costs nothing and old commands
  are never starved by new ones. There is no retry timer.
- Finalized input follows committed simulation ticks and rides in every state
  packet's owner status, with the applied menu sequence and arrival feedback.
  It is separate from transport receipt, state acknowledgement and action
  outcomes. A high received sequence cannot acknowledge a command that never
  executed. Arrival feedback uses the minimum slack among newly accepted or newly
  late commands since publication. The client changes lead only from commands
  that could have benefited from its current lead policy; earlier samples remain
  lateness evidence.
- Menu actions use a separate bounded reliable sequence and at most one menu
  action per player per tick. An unavailable choice is still finalized.
- A new connection epoch clears pending commands and acknowledgements. Stale
  epochs and entity generations cannot replace current client state.

Dreamlance and channeled beams capture an immutable estimated sample time E,
displayed remote pose time R and the state packet that carried the owner state
displayed at the hardware edge. Command target C and first accepted server arrival A remain separate.
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
reliable controls, acknowledged with `OutcomesAck` and idempotent, independently
of input finalization. Retained client
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

The Welcome binds content identity, owner identity and the global and owner
entity IDs. The client validates them before Ready and builds its collision
scene locally from the content identity's scene revision. A joining hero stays
inactive until the client acknowledges a state packet carrying its owner state;
activation then commits on a simulation tick. Pending players cannot move,
receive rewards, take damage or block party readiness, and receive only their
owner state and the match summary until activation commits. A fresh
authenticated stream displaces a peer that never committed activation for the
same profile.

Nothing in the session asks the server to start over. A lost packet is covered
by the next one, a stalled client resumes from the newest state, and the
connection ends only on authentication or protocol failure.

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

## Replicated state and prediction

The server captures one committed simulation state and prepares the persistent
graph once per publication. Each connection gathers from global, owner and
spatial nodes under the game's disclosure policy; only approved representations
reach encoding. The owner checkpoint, the match summary and every relevant actor
are entities in one per-connection stream.

Current component capture is shared with the full checkpoint writer. Live
publication validates current identities, populations and nested state without
cloning the offline combat archive. `cargo run -p dreamwake_sim --example
replication_capture` runs a bounded 120-capture probe; its timing excludes the
rest of the server and does not qualify throughput.

Public actor views contain approved presentation data. Private loadouts,
progression and latent owner state appear only in that owner's checkpoint.
World RNG, allocation state, future encounters and complete server state are
absent from live payloads.

At each 20 Hz publication the server sends an entity only when its state differs
from the newest state the client acknowledged, and not when that exact state is
already in flight and not yet known lost. Packets carry the owner first, then
the match summary, then actors nearest first; actors farther than 16 m go out on
every other publication, the relevance-based cadence of Unreal's replication
graph. Each entry is either whole or the changed fields against the newest
acknowledged state, which the entry names by packet sequence. Removals repeat
until a packet carrying them is acknowledged.

The client acknowledges with the newest packet sequence and a 64-packet received
bitmask in every input packet. A packet older than that window, or one whose
named base the client no longer holds, is ignored and covered by a later packet.
A sent packet missing from an acknowledgement that covers it is lost; its
entities become eligible again and go out with their current values. Nothing is
retransmitted as such, and a lost packet costs nothing until it is reported.

Public schemas quantize positions to 1/1,024 m, directions to 1/4,096, speeds to
1/256 m/s and durations to 1/1,024 s, and encode them as zig-zag variable-length
integers whose deltas are differences. The owner checkpoint is not quantized:
prediction restores it exactly, so a quantized owner would differ from the
authority by up to the step on every correction. Booleans cost only their
change-mask bit. Compact encoding follows the schema registry's field order and
is accepted only after the content handshake matches the registry.

A typical session needs tens of KB/s per client. Protocol identity changes
require rebuilding clients and servers together.

Client prediction restores the admitted owner and its transient components into
a reusable `DreamSimulation` runner. The same `DreamwakePlugin` systems execute
movement, skills, delayed actions, companions, projectiles and graphics on the
server and during owner replay. Authority-only systems retain ownership of enemy
decisions, contacts, damage resolution and encounter progression. The newest
owner state wins: the predictor restores it at its tick and replays retained
local input after it. A state older than the retained 256-tick history, or
ahead of the prediction, becomes the new base. Nothing in correction ends or
resets the session.

At bootstrap, or when the predicted frontier has fallen behind the estimated
simulation tick, the client forecasts substitutes through the tick before the
fresh command target and authors input only for that target. Forecast
transitions do not allocate command sequences, drain the hardware journal or
create action edges.
Timestamped clock replies anchor a separate estimate of the
server's committed simulation tick; delayed owner states and finalization status
advance its proven lower bound without pretending they describe receipt time.
Adaptive command lead saturates at `MAX_COMMAND_LEAD_TICKS`, and the server
independently enforces the wider `MAX_COMMAND_HORIZON_TICKS` admission window.
Both live in `engine_net::types` so the two sides cannot drift. Targets are
chosen against the client's estimated tick while the window is measured from the
server's finalized tick, so the window sits above the lead cap and an ordinary
finalization lag does not push a legitimately led command outside it. Latency is
a quality axis, never an admission criterion: a round trip past what lead can
cover makes commands arrive late, which the player sees and can play through. It
never fails the clock or ends the session, because starting over cannot shorten
the path. The client
bounds speculation to its retained prediction history (256 ticks, about four
seconds) beyond the latest proven committed tick. Replies that repeat a stalled
tick cannot refill that budget. Once exhausted, assignment waits without
consuming input edges or command sequences; the wall-clock estimate and server
debt remain visible.

A client that stops running for a while (a garbage collection pause, shader
compilation, a background tab) recovers in place, never through a server
resync. The clock estimate jumps to the time that passed instead of slewing,
and a large offset step re-anchors on the newest sample. The command head moves
forward; the skipped ticks had no input, and the server fills them with the same
substitutes the client forecasts. A finalized checkpoint ahead of the prediction
becomes the new base however far ahead it is, because every command before it
is already finalized. Replay and forecast work are bounded by retained history; a forecast gap longer
than the frame's budget is covered over several frames. Predicted presentation events restart their journal
when they cannot follow a rebase. Lead only shrinks after eight arrival samples
that all kept more than two ticks of slack, because a late command is always a
misprediction. A feedback sample is reported once, so it is
never applied twice. Attack sample stamps and remote display time use
the same simulation clock, including fractional tick phase. Newly assigned
commands use dependencies from the latest owner state; already recorded commands
retain their original dependency frames for replay. This refreshes bounded platform samples without rewriting
historical reads. Skills and reversible graphics use the admitted owner state;
remote damage and hit resolution remain authoritative. The
[moving-base contract](moving-bases.md) describes platform dependency and rider
display time. Presentation consumes `DreamPresentation`, while offline replay
uses the complete `DreamCheckpoint` codec. See
[predicted graphics](predicted-graphics.md) for simulation-owned effect identity.

Each newly authored or forecast frame also retains up to 64 nearest independently
disclosed enemy poses as optional companion targeting inputs. Samples newer than
the frame are excluded. Replay uses those recorded values rather than live
replicas; an entity leaving the view removes the target from subsequent frames.
They supply neither an authoritative damage decision nor enemy AI.

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
transport ACK for a state acknowledgement or finalized-input metric.

Run the affected short crate tests during development. Full campaigns, extended
matrices and soaks are manual handoffs after implementation and integration;
provide their exact commands, frozen inputs, duration, criteria and artifact paths.
Transport or browser changes also require WASM compilation and a real browser
session; unit tests do not establish deployed connectivity or performance.

`just netcode-harness` (`scripts/netcode-harness.py`) is the player-facing gate
for the network layer. It builds the native release pair, then runs one server
and two autoplaying rendered clients per scenario, over both native UDP and the
browser's WebRTC data channel path (`--transport webrtc` negotiates through the
same session and SDP endpoints a browser uses). Scenarios are a clean link, a WAN
path (75 ms ± 10 ms and 1% loss per direction), the same path with 0.3% loss, and
the clean and lossy paths with a 500 ms stall every 5 s (`--inject-stall 500@5`).
The stall blocks the client between frames, as the browser's garbage collection
and shader compilation hitches do. UDP runs use the server's conditioner. WebRTC
runs impair the client's raw datagrams beneath DTLS and SCTP
(`--link-impairment DELAY_MS:JITTER_MS:LOSS_PERCENT`), because conditioning above
the data channel leaves SCTP's congestion control on an ideal link. Each client measures
its own display (`session_health` in the trace). Any disconnect, reconnect
or teleport fails the scenario, as do more than six
visible corrections per connected minute. A correction is a frame whose owner
displacement exceeds dash speed × 1.25 over the frame plus one tick by 0.10 m; a
teleport exceeds it by a metre. A client's p95 round trip must also stay within
the path's own round trip plus 150 ms (plus the stall, for stall scenarios),
because a sender outrunning its path shows as queueing long before anything
disconnects. These gates read only `DreamView` and
`DreamConnection`, so they stay valid whatever prediction and replication design
runs underneath. Artifacts go to `out/netcode-harness/RUN/`; `--transports`,
`--scenarios` and `--duration` select a subset.

`scripts/playtest-spatial.py --profiles moderate --duration 120` is a single
development spatial check with two native release clients and an explicit 12 m
replication radius. It builds the paired native release artifacts before launch.
Travelers alternate between
80-metre side waypoints and the center on the shared gameplay clock. Qualification
requires at least three observed 32-metre cells per Traveler, authoritative
same-tick separation, no fresh remote disclosure while far, a fresh entry on
return, and continuous activity through the end. The larger demo also contains 45
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
for three seconds. The runner requires the same connection epoch to be active
again within ten seconds of resuming, with the same process and server instance
and an advancing authoritative checkpoint. It records the signals and before/after observations
in `processes.json`. Window focus, visibility, size and reported OS occlusion are
included in the native trace to help investigate rendering stalls.

See [deployment](deployment.md) for server configuration and release instructions.
