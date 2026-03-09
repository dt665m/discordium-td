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
node ./scripts/netcode-verify.mjs
```

Use headed Chromium. Headless Chromium is not reliable for this Bevy wasm client because WebGL surface creation may fail.

The verifier resolves Playwright from the shared Codex runtime used by [$playwright-interactive](/Users/dt665m/.codex/skills/playwright-interactive/SKILL.md), so this repository does not need its own `package.json` or `node_modules/`.

## What gets recorded

Server NDJSON:

- one authoritative `ServerDebugFrame` per fixed tick
- per-client world delta, acked tick, confirmed baseline tick, sent-history depth

Client NDJSON:

- batched `ClientDebugFrame` uploads
- platform (`native` or `wasm`)
- client instance id and upload sequence
- authoritative, predicted, interpolation, and rendered-actor data

## How to compare traces

- Match frames by authoritative tick first
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
cargo check --workspace --all-targets
cargo check -p game_client --target wasm32-unknown-unknown
cargo test --workspace
just check
```
