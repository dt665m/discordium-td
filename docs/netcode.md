# Networking and authority

The networking foundation is independent of the game. `engine_net` provides
bounded serialization/compression, generic input/action/state envelopes, channel
configuration, sequence comparison, a configurable fixed-step clock, and
`PeerInbox<I, A>` for per-peer held input and reliable action queues.

`engine_server` constructs the HTTP session service and mixed UDP/WebRTC
transport. `ServerPlugin<A: Authority>` registers polling and exit systems around
one non-send `ServerDriver<A>`. The driver owns connection events, fixed-step
dispatch, publication cadence and packet generation. Games implement admission,
payload validation, simulation steps and snapshot publication. Production runs
through Bevy's headless schedule runner; embedded apps can install the same
plugin. `ServerElapsed` supplies deterministic elapsed time for tests, otherwise
polling uses `Time<Real>`. Time cannot move backwards. Exit cleanup releases
admission and game ownership and disconnects transports; `run_app` returns
recorded failures after the app exits. Direct `poll(elapsed)` remains available.

## Canonical game

Dreamwake supplies concrete input/action/snapshot types in its protocol crate and
implements server authority in `DreamwakeServerPlugin` in its server crate.
`dreamwake_server::build_app(shared, seed, lucid)` composes that plugin with the
generic runtime. Native clients connect over UDP;
browser clients connect over WebRTC through the same session service. Native
hosting invokes that same server composition in a thread and connects over UDP.

The game simulates at 60 Hz and publishes compressed full snapshots at 20 Hz.
Transport receives are polled more frequently, but generated game packets follow
the fixed clock. Catch-up is capped at six steps per poll and publication sends
only the latest completed state. Polling faster does not increase simulation time.

Inputs carry an epoch and sequence. The game's authority validates ownership,
channels, bounds, and finite directions. The engine inbox rejects old epochs and
replayed sequences, enforces game-supplied message/queue limits, and consumes at
most one reliable action per player per tick. Received sequence numbers and
applied acknowledgements are separate. The game acknowledges actions after
processing, including unavailable menu choices; an epoch reset clears pending
commands and acknowledgements. Freshness is engine-owned, while the game chooses
which held fields to neutralize after its timeout. Stale revisions do not replace
client state, and reconnects restore authoritative state.

Clients restore the complete snapshot, prune acknowledged inputs, and replay a
bounded pending history through the same `DreamwakePlugin` simulation wrapped by
`DreamSimulation`. Generic replay iteration
is shared; deciding which game actions are still pending belongs to the game
adapter. A displayed game snapshot feeds both UI and the presentation adapter.

Snapshots include authoritative engine components for health, actions, movement,
combat/status, loadouts, projectiles, summons, and delayed execution, together
with game payloads and presentation instances. The component extraction changes
the saved-state wire layout and increments the game protocol identity to version
3. Rebuild clients and servers together; version 2 peers are incompatible.
The engine itself has no hardcoded game protocol ID or game action enum.

Dreamwake evaluates attacks in the current server state. The retired TD delta
replication and historical hit-rewind algorithms are not active in this game.

## Diagnostics and checks

F6 opens the shared client network tools. Each game supplies its telemetry adapter;
the conditioner acts on real transport packets. The engine's server configuration
also supports packet conditioning. TD-specific recorder/bridge tooling is retired.

`cargo test --workspace` includes codec bounds, sequence wrap, fixed-clock cadence,
engine plugin timing/shutdown and inbox tests, game action validation, snapshot restore/replay, multiplayer
UDP ownership/ACKs, eight-player state, loss/rejoin, and complete cooperative runs.
These checks do not establish large-party performance or deployed browser quality.
Use `just check-web` for WASM compilation and verify an actual browser session when
changing transport, graphics, or web configuration.

See [deployment](deployment.md) for the environment variable migration and
[the historical netcode record](history/td-netcode.md) for the retired TD path.
