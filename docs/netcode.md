# Networking and authority

The networking foundation is independent of the game. `engine_net` provides
bounded serialization/compression, generic input/action/state envelopes, channel
configuration, sequence comparison, and a configurable fixed-step clock.

`engine_server` constructs the HTTP session service and mixed UDP/WebRTC
transport. Its `ServerDriver<A: Authority>` owns transport polling, connection
events, fixed-step scheduling, publication cadence and packet generation. Games
implement admission, input acceptance, simulation steps and snapshot publication.
Embedders can call `poll(elapsed)`; dedicated servers can use `run()`.

## Canonical game

Dreamwake supplies concrete input/action/snapshot types in its protocol crate and
implements server authority in its server crate. Native clients connect over UDP;
browser clients connect over WebRTC through the same session service. Native
hosting invokes that same server composition in a thread and connects over UDP.

The game simulates at 60 Hz and publishes compressed full snapshots at 20 Hz.
Transport receives are polled more frequently, but generated game packets follow
the fixed clock. Catch-up is capped at six steps per poll and publication sends
only the latest completed state. Polling faster does not increase simulation time.

Inputs carry an epoch and sequence. The authority validates ownership, bounds
and finite directions, queues reliable edge actions, and acknowledges actions
only after application. Old epochs and stale revisions do not replace current
client state. Missing held input times out; reconnects restore authoritative state.

Clients restore the complete snapshot, prune acknowledged inputs, and replay a
bounded pending history through the same game simulation. Generic replay iteration
is shared; deciding which game actions are still pending belongs to the game
adapter. A displayed game snapshot feeds both UI and the presentation adapter.

Snapshots include separate authoritative actor health and latent state needed for
replay. This consolidation changes the saved-state wire layout and increments the
game protocol identity to version 2. Pre-consolidation clients must be rebuilt.
The engine itself has no hardcoded game protocol ID or game action enum.

Dreamwake evaluates attacks in the current server state. The retired TD delta
replication and historical hit-rewind algorithms are not active in this game.

## Diagnostics and checks

F6 opens the shared client network tools. Each game supplies its telemetry adapter;
the conditioner acts on real transport packets. The engine's server configuration
also supports packet conditioning. TD-specific recorder/bridge tooling is retired.

`cargo test --workspace` includes codec bounds, sequence wrap, fixed-clock cadence,
engine-driver tests, game action validation, snapshot restore/replay, multiplayer
UDP ownership/ACKs, eight-player state, loss/rejoin, and complete cooperative runs.
These checks do not establish large-party performance or deployed browser quality.
Use `just check-web` for WASM compilation and verify an actual browser session when
changing transport, graphics, or web configuration.

See [deployment](deployment.md) for the environment variable migration and
[the historical netcode record](history/td-netcode.md) for the retired TD path.
