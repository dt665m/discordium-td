# Networking and authority

`engine_net` provides bounded encoding, channel configuration, sequence comparison,
transport cadence and `PeerInbox<I, A>` for held input and reliable action queues.
Games supply message types, admission rules and payload validation.

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

Dreamwake simulates at 60 Hz and publishes compressed full snapshots at 20 Hz.
Catch-up is capped at six simulation steps per poll; publication sends the latest
completed state. Socket polling frequency does not increase simulation time.

## Input and acknowledgement invariants

- Inputs and actions carry an epoch and sequence. Validate ownership, channels,
  bounds and finite directions before applying them.
- The engine inbox rejects old epochs and replayed sequences, enforces supplied
  message/queue limits and consumes at most one reliable action per player per tick.
- Received sequences and applied acknowledgements are distinct. Dreamwake
  acknowledges actions after processing, including unavailable menu choices.
- Epoch resets clear pending commands and acknowledgements. The engine tracks
  freshness; the game chooses held-input fields to neutralize on timeout.
- Stale revisions must not replace client state. Reconnects restore authority.

Dreamwake resolves attacks against the current server state.

## Snapshots and prediction

Clients restore a complete authoritative snapshot, prune acknowledged inputs and
replay bounded pending history through the same `DreamwakePlugin` and
`DreamSimulation` used by authority. The game decides which actions remain pending;
the resulting displayed snapshot feeds UI and graphics.

Snapshots include authoritative health, actions, movement, combat/status, loadouts,
projectiles, companions and delayed execution alongside game payloads. The
`presentations` field contains `GraphicsInstance` effects. Preserve all state needed
for replay, including cooldowns, hit history, readiness and stable identities.
See [predicted graphics](predicted-graphics.md) for effect lifecycle rules.

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

The renderer smooths correction targets. Graphs leave unavailable samples as gaps;
delta-baseline and interpolation rows are N/A for Dreamwake's full-snapshot replay.

Use `cargo test --workspace` for codec, sequencing, authority, replay and multiplayer
coverage. Transport or browser changes also require WASM compilation and a real
browser session; unit tests do not establish deployed connectivity or performance.

See [deployment](deployment.md) for server configuration and release instructions.
