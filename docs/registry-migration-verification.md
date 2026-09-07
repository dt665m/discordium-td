# Registry migration verification — 2026-09-07

The accumulated netcode, prediction, presentation and diagnostics changes use
registry `renet-cross 0.6.0` and `bevy-net-debug 0.1.0`. Client and server
conditioner handles are supplied when transports are constructed. The local
renet-cross vendor override is removed. The pre-existing str0m patch remains.
Gameplay protocol is 11; rebuild both ends together.

## Build and regression results

All commands passed against the final Rust sources and lockfile:

- `cargo fmt --all -- --check`
- `cargo check --locked --workspace --all-targets`
- `cargo test --locked --workspace`: client 23, server 13, shared 7,
  simulation 33, and native transport integration 3 tests passed.
- `cargo test --locked -p game_server --features ui`: 14 server tests and
  3 transport integration tests passed.
- `cargo check --locked -p game_client --target wasm32-unknown-unknown`
- `node --test scripts/netcode-analysis.test.mjs`: 4 passed.
- Locked Trunk web-dev bundle built successfully with the local server URL.

Native integration covers server delay, jitter/loss/outage recovery, duplicate
actions, and walking attacks recovered through redundant movement bundles.

## Browser evidence

Built-in browser WebRTC gameplay ran against the rebuilt server on port 18081
and web-dev bundle on port 1421. Server delay stayed at 150 ms per direction.
K produced its simulation-owned burst. F3 showed approximately 308 ms RTT,
advancing snapshots, and zero baseline misses, decode errors, transport errors,
or browser queue drops. Its browser warning/error log query returned no entries.

F6's 300 ms target correctly added zero delay because the measured baseline
already exceeded 300 ms. Adding 10 ms per direction changed live RTT; a one-second
outage incremented the directional outage counters. Off restored client
conditioning to disabled and the connection continued receiving patches.
Off does not remove server delay or flush the simulation's movement backlog:
the later capture still showed 26 pending inputs and about 842 ms input ACK latency.

The existing headed verifier ran a separate five-second sampled gameplay scenario:
25/25 samples matched authoritative server state, zero errors, two tick-lag
warnings (7 and 8 ticks), maximum render/prediction distance 0.3919 world units.
The local report is `/tmp/discordium-registry-verification.json`.

Chromium's harness logs included a preload-integrity warning, a resource 404,
and `ERR_ABORTED` notices for recorder POSTs. These are retained in the report;
they are not a clean-console result. The same harness session nevertheless wrote
57 client frames, reported 11 successful uploads and zero upload failures/drops.
The built-in browser session also reported zero upload failures/drops. Server
recorder health had zero dropped records and zero write failures, with its run
budget not reached. Run ID: `1788748273485-14945`.

These are local regression and smoke tests. They do not establish long-session
Internet behavior, recovery of input latency after every outage, production
capacity, or cross-architecture deterministic simulation.
