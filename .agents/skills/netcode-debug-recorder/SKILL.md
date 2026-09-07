---
name: netcode-debug-recorder
description: Use when debugging Discordium TD server-authoritative netcode, interpolation, reconciliation, lag hiding, or long-session desyncs across native and wasm clients. Enables the shared debug bridge, recorder endpoints, durable NDJSON capture, and the Playwright-backed verifier.
---

# Netcode Debug Recorder

Use this skill when the task is to inspect or verify client/server netcode behavior instead of only changing gameplay code.

## When to use it

- Reproducing desyncs, rubber-banding, or incorrect reconciliation
- Comparing server-authoritative state to client-applied snapshots
- Inspecting interpolation or render smoothing against authoritative ticks
- Recording long sessions where the in-memory bridge history would be overwritten
- Running the browser verifier against the Bevy wasm client

## Core workflow

1. Enable the server recorder.

```bash
cargo run -p game_server -- --debug-recorder
```

Optional log directory override:

```bash
cargo run -p game_server -- --debug-recorder --debug-log-dir /tmp/discordium-debug
```

2. Enable a client recorder.

Native client:

```bash
cargo run -p game_client -- --debug-recorder
```

Wasm client:

- open `http://127.0.0.1:1420/?debug_recorder=1`

`debug_recorder` implies the debug bridge on both server and client. For browser runs, it also exposes `window.__discordiumDebugBridge`, accepts `window.__discordiumDebugBridgeCommand = "connect_dev"`, and uploads chunked debug batches over `fetch`.

3. Inspect live state if needed.

- `GET /debug/bridge` returns the bounded in-memory server history
- `GET /debug/recorder` returns the active `run_id`, run directory, and NDJSON output paths
- `POST /debug/client-frames` ingests browser/native client frame batches

4. Run automated verification for the wasm client when appropriate.

```bash
./scripts/run-netcode-verify.sh
```

Use headed Chromium. Headless Chromium is not reliable for this Bevy wasm client because WebGL surface creation may fail.

The wrapper prepares Playwright in an OS-temporary runtime, so this repository does not need its own `package.json` or `node_modules/`. Set `PLAYWRIGHT_RUNTIME_DIR` to override that location.

## What gets recorded

Server NDJSON:

- one authoritative `ServerDebugFrame` per fixed tick
- per-client world delta, acked tick, confirmed baseline tick, sent-history depth

Client NDJSON:

- batched `ClientDebugFrame` uploads
- platform (`native` or `wasm`)
- client instance id and upload sequence
- authoritative, predicted, interpolation, and rendered-actor data

## Capture identity and health

- Set `TD_REALM_ID`, `TD_INSTANCE_ID`, and `TD_BUILD_REVISION` for realm instances.
- Read `metadata.json` and `health.json` before drawing conclusions from incomplete captures.
- Schema version 2 adds server process identity, client session IDs, capture timing, transport metrics,
  replication counters, and recorder drop/failure counters. `events.ndjson` captures server lifecycle events.
- Client F3 displays metrics; F4 marks the next sampled frame. Capture rate is 10 Hz.
- F6 toggles the renet-cross conditioner panel; hiding it does not disable impairment.
- For a 300 ms experiment, connect with conditioning Off, allow baseline RTT to settle, then select ~300 ms RTT. Verify observed `network.rtt_ms`, not just the preset.
- Client frame `conditioner` records settings, baseline, queues, and distinct simulated/overflow/outage/transition drops. Older captures may omit this optional field.
- Server-side alternative: `cargo run -p game_server -- --net-delay-ms 150 --debug-recorder` adds 150 ms each way to every UDP/WebRTC client. Keep client conditioning Off. `--net-jitter-ms` and `--net-loss-percent` add jitter/loss; the optional server UI has runtime controls.
- Server frame `conditioner` and `server_conditioner_startup`/`server_conditioner_changed` events identify artificial conditions. Server presets are added RTT, not baseline-adjusted targets.
- Use Off after an experiment. Browser conditioning does not cover ICE/DTLS/SCTP establishment.
- Client uploads are best effort, size-bounded and timeout-limited; failures are counted, not retried indefinitely.
- The server defaults to 256 MiB of NDJSON per run; override with `TD_DEBUG_MAX_BYTES`.
- A successful upload means queued; inspect write failures and `limit_reached` in recorder health.
- Keep these opt-in debug endpoints on internal/admin routes.

## How to compare traces

- Match frames by authoritative tick first, scoped to the server process session; distinguish client reconnects by client `session_id`
- For server-side per-client data, also match by `client_id`
- Compare:
  - server authoritative world delta
  - client authoritative snapshots
  - client predicted state
  - client rendered positions after smoothing
- Use recorder logs for long sessions and `/debug/bridge` for quick live inspection

## Repository specifics

- Default recorder output directory: `target/debug-recorder/YYYY-MM-DD/<run_id>/`
- Verifier report output: `target/netcode-verifier/YYYY-MM-DD/<run_id>/report.json`
- Browser bridge global: `window.__discordiumDebugBridge`
- Browser command hook: `window.__discordiumDebugBridgeCommand`
- Embedded local server mode in the native client forwards `--debug-recorder` automatically

## Validation

Before finishing code changes in this area, run:

```bash
just check
cargo check --locked -p game_client --target wasm32-unknown-unknown
just test
node --test scripts/netcode-analysis.test.mjs
```
