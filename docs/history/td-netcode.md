# Networking and prediction contract

> Historical record from before the engine consolidation. Source paths, commands,
> and validation results below describe that version; use the current root README
> and architecture/deployment guides for this workspace.

The shared simulation owns gameplay. The server runs it at 30 Hz; clients restore
complete authoritative snapshots and replay bounded local input. Both native UDP
and browser WebRTC use Renet channels through renet-cross.

The shared crates live under `shared/`: `protocol` defines the wire contract,
`simulation` supplies the ECS components and gameplay plugin groups, and
`replication` supplies stable `NetId` identities and world-local entity indexing.
The server installs the simulation plugins in its main Bevy world. Client
prediction uses the same plugins in an isolated world through the `Simulation`
facade. Snapshot restore updates surviving entities and removes obsolete
simulation entities without clearing unrelated entities or resources.

Simulation phases have explicit dependencies. Their small coordinating systems
run sequentially; sufficiently large independent actor queries use Bevy's compute
pool. Collisions and damage retain deliberate ordering. Spatial grids, navigation
data, and query caches are derived accelerators; actor components own gameplay
state. Cross-platform lockstep determinism is not required.

## Replication ownership

`game_server::replication::ReplicationHistory` owns one shared history of canonical
serialized world states and changes between frames. `ClientNetState` holds each
connection's last acknowledged complete version and metadata for outstanding
sent versions (tick, baseline tick and payload length). These ACK records
do not retain packet payloads after shared history evicts them. The movement ACK
is already part of each hero, so world payloads do not need a duplicate
recipient-specific field.

A recovery update unions changes after that client's acknowledged frame and reads
**current values** for those changes. Receiving an intermediate update is not a
prerequisite for applying a later update from the same baseline. Only a client ACK
for a sent, fully reconstructed version advances its known version. Unknown,
repeated and older ACKs cannot advance or regress it.

Equivalent `(baseline frame, current frame)` requests share an encoded packet.
Payloads and history are released as clients acknowledge them. History is also
bounded to 64 frames and a 32 MiB accounting budget, with a maximum baseline age
of 60 ticks. Slow clients therefore cannot pin history indefinitely. A new,
expired or unavailable baseline uses a full update; full state is also chosen
when its compressed payload is smaller. Client snapshot history retains 64
accepted versions for patch reconstruction.

This follows the change-history, client-ghost and reusable-packet design described
for Overwatch's Statescript in Dan Reed's [GDC 2017 slides, pages 44–48](https://media.gdcvault.com/gdc2017/Presentations/Reed_Dan_NetworkingScriptedWeapons.pdf#page=44).
Our change sets use stable world/hero/enemy/tower records and changed-byte bitsets
within each record. Recovery for all requested baselines shares one backward pass
through retained history. Wire patches use relative IDs, bounded varints and
per-record full values, byte masks or sparse ranges; floats remain exact. An entity's variable-length data cannot shift other entities. Changes
are lossless and schema-complete: floats are not quantized, and restoring a patch
produces the exact server state. A heavily changed world can still make a full
update cheaper.

The wire codec uses bounded DEFLATE framing for larger messages and raw framing
for small inputs. Decoding checks the inflated size before allocation, rejects
trailing data, and enforces 64-byte action / 2,048-byte movement budgets on server
input. Snapshot decoding has a separate 1 MiB limit. Protocol **13** requires
coordinated client and server builds; debug JSON schema is **3**.

Replication remains a complete-world contract, so payload size and prediction cost
still grow with the number of changing units. Compression and shared caching are
not a guarantee that arbitrary unit counts fit one datagram. Size, real packet
loss, and exact reconstruction must be measured when increasing game limits;
Renet requires every fragment of an unreliable message before delivering it.

## Timing and input recovery

Client input capture and transport receive/application run before Bevy's fixed
loop; prediction executes in `FixedUpdate`. On the server, a network plugin owns
a dedicated worker for transport polling, Renet, and per-client replication.
A bounded `rtrb` SPSC ring hands decoded inputs to `PreUpdate` without a second
input timer. The worker decodes directly from Renet's `Bytes`, moves the resulting
commands through the ring, and handles replication ACKs itself. There is no
payload `to_vec()` or second movement-bundle decode. The ECS consumer uses Bevy's
`SyncCell` for exclusive access, without an inbox mutex. Only scalar ring APIs
are exposed; chunk/iterator/manual-commit APIs are deliberately excluded.

Each staging pass captures the published count and handles at most 256 entries,
with a soft 500-microsecond deadline checked between entries. These initial
safeguards are not measured optimal values. A message contains at most 2 KiB of
input and expands to at most 16 moves and 64 actions. New arrivals cannot extend
the captured pass. The ring holds `max_clients * 50 + 1` entries: existing quotas
of 16 reliable and 32 unreliable messages per peer, two lifecycle slots per peer,
and one metrics slot. Quotas live on the worker and reset only when a fixed tick
advances the admission epoch; repeated staging passes do not replenish them.
Invalid messages and ACKs also consume quota. A full ring leaves messages in
Renet and pauses receive polling while the worker continues output/timers and
its normal sleep. Metrics are sampled once per tick and deferred if space is
unavailable. ECS records staged-entry counts, deferred passes and peak backlog.

Staging applies connection lifecycle in FIFO order and routes commands into the
existing simulation input buffers. The fixed tick executes those inputs,
validates lag compensation, runs the shared simulation schedule, and stages
complete output in a resource. `PostUpdate`
exports the latest state once per app frame through an eight-batch FIFO. If a
frame runs several catch-up ticks, their reliable deliveries stay in order while
obsolete intermediate snapshots are coalesced. The network worker applies the
same rule to a bounded batch of queued app frames after a worker stall, encoding
only the newest world while preserving all reliable deliveries. Replication can
reconstruct from retained published ticks even when intermediate ticks were not
sent. Reliable output is never silently
discarded: exhausted per-peer retention disconnects that peer, and exhausted
worker output retention surfaces a pipeline failure. Transport access shared with
HTTP signaling stays outside the simulation tick.

The headless app polls fixed-time boundaries every millisecond. Its small frame
coordination schedules use Bevy's sequential executor; large simulation queries
still use the compute pool. The worker polls transport independently with a
one-millisecond receive timeout; queued ECS output wakes that
wait immediately. Completed state drives packet sends directly, so it cannot
wait for an unrelated 30 Hz send phase. Idle ACK/retransmit traffic keeps a 30 Hz
wall-clock cadence even when simulation stalls. After a state send, the idle timer
allows two milliseconds of scheduling jitter before sending an ACK-only packet,
avoiding duplicates just before the next state. Catch-up frames export one latest
snapshot rather than a burst of stale states. Client sends retain their own
bounded 30 Hz cadence.

Movement and reliable actions have independent sequence watermarks. Actions retain
their original movement barrier through local execution, snapshot restore and
replay. Both restored and newly captured actions enter the same ordered action
queue. A movement ACK cannot discard a reliable action.

The server executes one movement per tick and retains at most three movement
frames from a recovered burst. Older movement time is retired; the consumed
watermark acknowledges the skipped inputs too. It never executes multiple walking
steps to catch up. Actions waiting on those movements become eligible after their
barrier is retired. This intentionally trades stale predicted displacement for
bounded input delay after delivery stalls.

Client replay and forward prediction stop at 15 ticks of speculative lead. Reliable
actions do not create additional movement time. Terminal match phases discard
unacceptable input and wait for the server's next epoch rather than predicting a
new match. Round transitions and per-hero respawn generations reset correction
history. Enemy wave/ordinal identities distinguish spawns independently of
speculative allocation order. Rendered world IDs validate actor kind and spawn
identity before reusing an entity. Newcomers/respawns hold their first position
until the delayed timeline reaches that life, avoiding a jump from latest state
back onto interpolation.

Lag-compensation history retains each hero's server-owned respawn generation.
A rewind across a life boundary falls back to the current world even if death
and respawn occurred at the same position.

Remote heroes interpolate on server-tick spacing, not packet-arrival spacing.
The playback clock adjusts gradually for jitter and resynchronizes after outages.
Enemy prediction remains part of the existing whole-world combat contract; it is
bounded by the same replay horizon. Once prediction is initialized, snapshot
sampling only computes remote hero positions; enemy interpolation remains the
uninitialized fallback. HUD reads of the local hero use the simulation's identity
index instead of extracting the entire world. See
[predicted presentation](td-predicted-presentation.md).

## Verification

Run `just check`, `cargo test --workspace`, and
`node --test scripts/netcode-analysis.test.mjs`. The server replication tests cover
lost updates and ACKs, shared payloads, retirement, expiry, wraparound, structural
changes and scaling. Client tests cover interpolation at 30/60/120 FPS, replay,
match/respawn discontinuities and ordered actions.

With a debug-enabled server and wasm client, run:

```bash
./scripts/run-netcode-verify.sh --clients 2 --scenario-ms 20000
```

The headed browser verifier checks each client's authoritative state, prediction
lead, remote movement coverage and continuity. `--device-scale-factor 1` or `2`
can override the platform default. Use the server conditioner to add latency,
jitter and loss. Alternatively, `--condition-client` enables 150 ms each way,
25 ms jitter and 5% loss after connecting, keeping handshake setup unconditioned.
`--stress` additionally injects a one-second client outage and
a 150 ms browser frame stall. Capture health and identity must be checked before interpreting
results. Generated verification reports belong in `target/reports/`.

For repeatable bandwidth, CPU, memory and loss/recovery comparisons, see the
[netcode capacity eval](td-netcode-eval.md). The
[server ingress experiment](server-ingress-benchmark.md) separates byte-copy,
decoding and queue costs from live server measurements. The
[client presentation follow-up](client-presentation-benchmark.md) measures
indexed HUD reads and skipping unused enemy interpolation.

## Transport receipts and Dreamwake tick scheduling

The game and `renet-cross` use unmodified registry Renet 2.0.0. There is no
vendored Renet, Cargo patch for Renet, or alternate protocol core. Renet owns
transport acknowledgments, reliable retransmission and delivery order.

Receive/handshake polling runs every 1 ms. Renet packet generation runs once per
60 Hz server tick, after simulation and any due snapshot have been queued. It
continues in menus and paused gameplay, independently of whether a snapshot or
new gameplay message exists. Catch-up steps produce one flush of the latest state,
not a burst of historical flushes. The browser flushes in `PostUpdate`, limited to
60 Hz. This follows the upstream frame-oriented usage of Renet: frequent socket
polling does not require calling `send_packets` on every poll.

The F6 loss estimate and RTT come directly from upstream Renet. Its outgoing
packet loss estimate includes ACK-only packets and depends on acknowledgments
returning within its measurement window. It is not a count of physical network
drops or rejected gameplay commands. No custom denominator, ACK suppression, or
statistics correction is applied in the game or transport wrapper.

Dreamwake's shared `TICK_HZ` defines the 60 Hz simulation timestep. The server
publishes state every three completed server steps (20 Hz), from the latest
completed state after bounded catch-up. Publication has no separate wall-clock
deadline that can drift relative to the simulation. The server step clock still
runs when the gameplay phase is paused or in a menu, so connections and applied
command progress continue to be published.

The existing `ack_input` and `ack_action` fields describe simulation progress for
reconciliation: receipt by Renet does not imply execution by the authority. They
are snapshot fields, not an application transport ACK/retransmission loop.


### WebRTC SCTP retransmission-limit correction

During local transport iteration the workspace uses sibling `../renet-cross` and
patches `sctp-proto` to `../sctp-proto` (upstream 0.10.4, commit
`215565ad7aa80d3c160048350a2e6ddd8524920c`, with the local partial-reliability fix).
Renet itself remains unmodified registry 2.0.0.

The SCTP dependency marked `maxRetransmits=0` DATA abandoned on its first send.
Later SACKs advanced FORWARD-TSN over live packets and generated a growing
FORWARD-TSN/SACK exchange during ordinary lossless traffic. In production this
exceeded the server's per-peer UDP receive budget and dropped incoming traffic.
The correction abandons only at a would-be retransmission, as required by
[RFC 7496 section 3.1](https://www.rfc-editor.org/rfc/rfc7496.html#section-3.1),
shares abandonment across fragments, and retires unsent tails without leaking
stream buffer credit. Receive budgets and Renet's loss calculation are unchanged.

For bounded native packet tracing, opt into the log target
`renet_cross::packet_trace=trace`. It emits at most 100,000 encrypted packet
fingerprints per process across all peers, covering accepted sends, incoming
messages and local quota/input/send errors. It does not record packet payloads.
Leave the target disabled in normal operation; it is a temporary correlation aid,
not a replacement for transport counters or an IP packet capture.
