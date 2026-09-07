# Discordium TD

Server-authoritative Bevy 0.18 tower defense prototype using `renet-cross` (`~/Projects/games/net`) for mixed native/web transport.

## Prerequisites

- Rust toolchain (`rustup`, `cargo`)
- [`just`](https://github.com/casey/just)
- [`trunk`](https://trunkrs.dev/) for web client builds/serving
- Wasm target for web builds:

```bash
rustup target add wasm32-unknown-unknown
```

## Local Setup (Native)

Start server + native client:

```bash
just play
```

Run separately:

```bash
just server
just client
```

Default local endpoints:

- HTTP bootstrap/signaling: `http://127.0.0.1:8080`
- UDP game transport: `127.0.0.1:5000`
- WebRTC transport: `127.0.0.1:5001`

## Local Setup (Web Client)

Start server + Trunk web client:

```bash
just web-play
```

Or run only the web client (server must already be running):

```bash
just web-client
```

Build release wasm bundle:

```bash
just web-build
```

For incremental browser development, start the server separately and run:

```bash
just web-dev
# Or point it at another server:
just web-dev http://127.0.0.1:18081
```

This uses the `web-dev` Cargo profile: optimized dependencies, incremental game
code, and no release LTO or wasm-opt pass. Output goes to `target/web-dev/dist`.
The first build still compiles the dependencies; subsequent edits reuse them.
The larger wasm bundle is intended for local iteration. Use `web-build` for
deployment. In one local Chrome test, a game-code edit rebuilt in about 31 seconds
and two clients ran with a median frame duration around 8.6 ms; these are local
measurements, not performance guarantees for other machines or longer sessions.

## Simulating a bad connection

The client uses the Bevy UI from `bevy-net-debug 0.1` with registry `renet-cross 0.6`.
Press **F3** for grouped client diagnostics or **F6** for network simulation controls.
The compact HUD shows wave and team life; connection warnings and active lag
simulation appear only when relevant. F3 contains a compact network overlay; detailed events remain in captures.
Opening either panel hides the other without changing simulation settings.
F3 is translucent and includes 15-second history plots for RTT, ACK jitter, packet
loss, receive rate and send rate. Plots sample every 250 ms, keep labeled automatic
scales ("Max" is the top of the plot, not a performance target), and show gaps for stale/disconnected data. ACK jitter measures variation in
input acknowledgement delay, including server queuing; it is not raw path jitter.
The layout takes inspiration from the [Renet Visualizer README demo](https://github.com/lucaspoffo/renet/tree/master/renet_visualizer),
implemented here with Bevy UI only. Outbound movement bundles, snapshot ACKs and queued reliable actions flush on an
independent 30 Hz real-time cadence. Simulation still runs at 30 Hz; receive processing
runs each frame. Missed send slots coalesce instead of producing catch-up bursts.
ACKs repeat to retain recovery from packet loss.
The server UI shows instance details, aggregate metrics, a per-client table, recorder
health, and separate simulation controls with directional packet counters.
Press **F6** again to hide the network conditioner. Conditioning starts off;
hiding the panel does not disable it. Connect, allow the baseline RTT to settle,
then select **~300 ms RTT** (or 150 ms). The panel also supports custom per-direction
delay, jitter, packet loss, a one-second outage, Off, and baseline recalibration.
Each client controls its own connection on both native UDP and browser WebRTC.

Target RTT subtracts the measured baseline and adds half the difference in each
direction. The baseline freezes while conditioning is active. Actual RTT remains
visible and can differ because of scheduling and jitter. Recalibrate turns
conditioning off; allow its settling interval before selecting a target again.
The settings persist across reconnects, while session telemetry and queued packets
reset. Recorder frames include the conditioner configuration, baseline, queue
sizes, and separate simulated-loss, outage, overflow, and transition-drop counters.

This exercises Renet/netcode packets, acknowledgements, retransmission, and
connection timeouts. Browser ICE/DTLS/SCTP establishment is outside this conditioner;
a hosted or wire-level network test is still needed for those layers.

### Server-side conditioning

Server conditioning uses registry `renet-cross 0.6`. Shared conditioner handles
are installed through transport configuration at construction, covering the
handshake and subsequent gameplay. The client UI uses the same retained handle.

To simulate distance for all players, keep client conditioning Off and launch:

```bash
# Adds 150 ms each way: approximately 300 ms on top of real RTT.
cargo run -p game_server -- --net-delay-ms 150 --debug-recorder

# Same startup configuration with the built-in Bevy server control panel.
cargo run -p game_server --features ui -- --ui --net-delay-ms 150 \
  --net-jitter-ms 10 --net-loss-percent 1 --debug-recorder
```

The flags also work headlessly and have environment equivalents:

| Flag | Environment variable | Default |
| --- | --- | --- |
| `--net-delay-ms` | `TD_NET_DELAY_MS` | `0` |
| `--net-jitter-ms` | `TD_NET_JITTER_MS` | `0` |
| `--net-loss-percent` | `TD_NET_LOSS_PERCENT` | `0` |

Delay and jitter accept 0–5000 ms; loss accepts a finite percentage from 0–100.
The server UI can change these at runtime, select +150/+300 ms added RTT,
turn impairment Off, or trigger a one-second outage. Presets retain jitter/loss.
Each UDP/WebRTC peer has independent bounded queues. Startup settings apply to
future connections too. Real RTT remains in the existing per-client metrics;
server presets add delay rather than estimating every client's distance from one
shared baseline. Server frames record configuration and aggregate queue/drop
counters; recorder events capture startup configuration and UI changes.

## Debug Bridge and Recorder

For network, interpolation, reconciliation, and long-session desync debugging, both the server and client can expose a structured debug bridge. The optional recorder extends that bridge with durable NDJSON logs so a long lifecycle does not overwrite the important frames in memory.

Enable the server bridge:

```bash
cargo run -p game_server -- --debug-bridge
```

Enable the server recorder as well:

```bash
cargo run -p game_server -- --debug-recorder
```

Recorder mode also enables the bridge automatically and writes each run into its own dated directory under `target/debug-recorder/YYYY-MM-DD/<run_id>/` by default. Override the base log directory with:

```bash
cargo run -p game_server -- --debug-recorder --debug-log-dir /tmp/discordium-debug
```

Server debug HTTP routes:

- `http://127.0.0.1:8080/debug/bridge`
- `http://127.0.0.1:8080/debug/recorder`
- `POST http://127.0.0.1:8080/debug/client-frames`

Enable the native client bridge:

```bash
cargo run -p game_client -- --debug-bridge
```

Enable the native client recorder:

```bash
cargo run -p game_client -- --debug-recorder
```

When recorder mode is enabled, the native client batches `ClientDebugFrame` uploads to the server's `/debug/client-frames` endpoint in a background thread. If you use the in-client embedded server flow, `--debug-recorder` is forwarded to that embedded server automatically.

For the wasm client, the same bridge is enabled with a query parameter because runtime clap parsing is not available in the browser:

- `http://127.0.0.1:1420/?debug_bridge=1`

Enable the wasm recorder with:

- `http://127.0.0.1:1420/?debug_recorder=1`

`debug_recorder=1` also enables the browser bridge and uploads chunked frame batches to `/debug/client-frames` with bounded, time-limited `fetch` POSTs. Failed batches are counted and discarded; gameplay never waits for an upload.

When enabled in the browser, the latest client frame is published to:

- `window.__discordiumDebugBridge`

The browser bridge also accepts a simple command channel for automation:

- `window.__discordiumDebugBridgeCommand = "connect_dev"`

The client bridge includes:

- latest authoritative world tick and message type
- local predicted tick and reconciliation offset
- rendered actor positions after interpolation/smoothing
- authoritative and predicted hero/enemy/tower snapshots

The server bridge includes:

- authoritative per-client world snapshots
- last acked tick, confirmed patch baseline tick, and sent-history depth

Recorder output inside each run directory:

- `server.ndjson`: one authoritative server frame per fixed tick
- `client.ndjson`: uploaded client frame batches with source platform, instance id, and upload sequence

The `/debug/recorder` endpoint returns the active run id, run directory, and concrete output paths for the current server process.

## Realm and instance diagnostics

Diagnostics remain opt-in and use JSON schema version 2, independently of the gameplay protocol.
Run matching client/server builds when recording. Each process gets a unique process-session ID;
each client connection attempt gets a new session ID and sends its captures to the selected server,
including the embedded server on port 18080.

Set these deployment labels when starting each realm instance:

```bash
TD_REALM_ID=asia-1 TD_INSTANCE_ID=match-123 TD_BUILD_REVISION=my-build-id \
  cargo run -p game_server -- --debug-recorder
```

`TD_REALM_ID` defaults to `local`; `TD_INSTANCE_ID` defaults to the generated process-session ID.
`TD_BUILD_REVISION` is read at server startup and can also be supplied at compile time for native
and wasm clients. It should be a commit SHA or deployment build ID; `unknown` means it was not supplied.
No credentials or bootstrap tokens are added to capture metadata.

- **F3** toggles the client panel: Renet RTT/loss/bandwidth, connection state, snapshot age,
  input-ack latency/jitter, baseline misses, decode/transport errors, and
  interpolation/reconciliation state. Recorder health remains in captures rather than a UI card.
- **F4** marks the next debug frame (`marker: true`) when the bridge or recorder is enabled.
- **`just server-ui`** shows per-client network stats. For recording plus the server panel, use
  `cargo run -p game_server --features ui -- --ui --debug-recorder`.
- Browser captures also report local send-backpressure, receive-overflow, and invalid-size drops.
  These are separate from Renet's packet-loss estimate; native captures use `null` for unavailable
  browser counters. Packet loss is a fraction; RTT/age/duration fields ending in `_ms` are milliseconds.
  Legacy `rtt_ema` and `jitter_ema` remain seconds and measure input acknowledgement latency.
  A null `input_ack_age_ms` means those estimates have not been measured in this connection attempt.

Every recorder run contains:

| File | Contents |
| --- | --- |
| `metadata.json` | Realm, instance, process-session, build, schema/protocol, tick rate, recording limits |
| `server.ndjson` | Authoritative tick snapshots, per-client network stats, capture time, simulation-step duration |
| `client.ndjson` | Session-labelled client batches, capture time/frame duration, network/replication/recorder health, marked frames |
| `events.ndjson` | Server startup, client connection/disconnection reasons, transport-update errors, client IDs and ticks |
| `health.json` | Recorder write/drop counters and file-limit status, refreshed about once a second |

Use `(process_session, client_id, tick)` to correlate server data, and `session_id` plus `frame_index`
to distinguish client reconnects. Client monotonic capture time is relative to client application
startup; server elapsed time is relative to server diagnostics startup. These clocks are not synchronized:
match authoritative ticks first. Server `recorded_at_unix_ms` is enqueue/receipt time, not disk-write time.
Client `capture_elapsed_ms` preserves when the frame was observed even if uploads arrive later.

Client capture is sampled at 10 Hz. Uploads are limited to 256 KiB, queued client frames to 32,
native queued uploads to eight batches, and each HTTP attempt to three seconds. Browser uploads
allow one request in flight. There are no persistent upload retries; drops are visible in counters
and sequence gaps. Frame indices can also skip intentionally when a new session starts.

Server writes use a nonblocking 128-record queue and a **256 MiB total NDJSON limit per run**.
Set `TD_DEBUG_MAX_BYTES` to a positive byte count to change that limit. When full, new records are
rejected and `limit_reached` becomes true in `/debug/recorder`, `health.json`, and the server panel.
HTTP 202 means queued, not guaranteed written; inspect recorder health to establish capture completeness.
Recordings are best effort: queued data and the final partial batch may be lost on process/browser exit.
Old run directories are not automatically deleted; retention belongs in the instance's log collection setup.

Keep debug HTTP routes behind internal/admin routing. These opt-in routes expose game state and accept
capture uploads; this foundation does not add access control or realm orchestration.

## Netcode Verifier

With the debug bridge enabled on the server and the wasm client served on `127.0.0.1:1420`, run:

```bash
./scripts/run-netcode-verify.sh
```

The verifier will:

- open the browser in headed mode
- connect through the debug bridge command channel
- drive a short movement/combat scenario
- compare client authoritative snapshots against matching server authoritative frames
- require connected, advancing client/server samples and flag stale snapshots
- write a JSON report to `target/netcode-verifier/YYYY-MM-DD/<run_id>/report.json`
- preserve a partial failure report and bounded browser console/error history if setup or sampling fails

The wrapper prepares Playwright in an OS-temporary runtime, so the repo does not need a local `package.json` or `node_modules/` for browser verification. Set `PLAYWRIGHT_RUNTIME_DIR` to override that location.

Headless Chromium is not reliable for this Bevy wasm client in the current setup because WebGL surface creation can fail there.

Run the verifier's regression tests without a browser:

```bash
node --test scripts/netcode-analysis.test.mjs
```

For recorder-backed verification, start the server with `--debug-recorder`, run the wasm client with `?debug_recorder=1`, then run the verifier. That gives you a live mismatch report plus durable client/server traces for later inspection.

## Cloudflare End-to-End TLS (HTTP Handshake)

The server supports TLS for the HTTP bootstrap/signaling endpoint used during connect/handshake.

### Important transport note

Cloudflare in this setup is used for HTTPS bootstrap/signaling only.

- `TD_PUBLIC_HTTP_BASE` should be your Cloudflare HTTPS hostname.
- `--public-udp-addr` and `--public-webrtc-addr` are still direct, publicly reachable socket addresses for gameplay transport.

### Step-by-step in Cloudflare

### 1) Add DNS record for bootstrap host

In Cloudflare dashboard:

1. Open your zone.
2. Go to `DNS` -> `Records`.
3. Add an `A` record:
   - Name: `game` (or your preferred subdomain)
   - IPv4 address: your server public IP
4. Set Proxy status to `Proxied` (orange cloud).
5. Save.

Example hostname: `game.example.com`

### 2) Set SSL mode to strict

In Cloudflare dashboard:

1. Go to `SSL/TLS` -> `Overview`.
2. Set encryption mode to `Full (strict)`.

### 3) Create an Origin Certificate

In Cloudflare dashboard:

1. Go to `SSL/TLS` -> `Origin Server`.
2. Click `Create Certificate`.
3. Use Cloudflare-generated private key (recommended).
4. Add hostnames:
   - `game.example.com`
   - optionally `*.example.com`
5. Choose validity period.
6. Create certificate.
7. Copy/save:
   - Origin certificate PEM
   - Private key PEM

### 4) Install cert and key on the game server

Example:

```bash
sudo mkdir -p /etc/ssl/cloudflare
sudo tee /etc/ssl/cloudflare/origin-cert.pem >/dev/null <<'EOF'
<paste certificate PEM here>
EOF
sudo tee /etc/ssl/cloudflare/origin-key.pem >/dev/null <<'EOF'
<paste private key PEM here>
EOF
sudo chmod 600 /etc/ssl/cloudflare/origin-key.pem
sudo chmod 644 /etc/ssl/cloudflare/origin-cert.pem
```

### 5) Open required firewall ports on your server

At minimum:

- `443/tcp` for HTTPS bootstrap
- `5000/udp` for UDP transport
- `5001/udp` for WebRTC transport

### 6) Run server with TLS enabled

```bash
TD_HTTP_TLS_CERT=/etc/ssl/cloudflare/origin-cert.pem \
TD_HTTP_TLS_KEY=/etc/ssl/cloudflare/origin-key.pem \
TD_PUBLIC_HTTP_BASE=https://game.example.com \
TD_CORS_ALLOWED_ORIGINS=https://play.example.com,https://discordium-td.pages.dev \
cargo run -p game_server -- \
  --http-bind 0.0.0.0:443 \
  --udp-bind 0.0.0.0:5000 \
  --webrtc-bind 0.0.0.0:5001 \
  --public-udp-addr <PUBLIC_IP>:5000 \
  --public-webrtc-addr <PUBLIC_IP>:5001
```

Notes:

- `TD_HTTP_TLS_CERT` and `TD_HTTP_TLS_KEY` must both be set together.
- If TLS is enabled and `TD_HTTP_BIND` is omitted, default bind becomes `0.0.0.0:443`.
- `TD_PUBLIC_HTTP_BASE` should be your public HTTPS URL.
- `TD_CORS_ALLOWED_ORIGINS` is optional comma-separated CORS allowlist for browser clients on other origins.
- If `TD_CORS_ALLOWED_ORIGINS` is unset (or `*`), server allows any origin.
- `--public-udp-addr` and `--public-webrtc-addr` must be publicly reachable addresses for gameplay transport.

### 7) Verify HTTPS from outside

```bash
curl -I https://game.example.com/healthz
```

Expected: HTTP `200` or similar healthy response from your bootstrap server.

### 8) Point clients to HTTPS bootstrap

Native client:

```bash
just client http_base=https://game.example.com
```

Web client (Trunk):

```bash
just web-client http_base=https://game.example.com
```

### 9) Troubleshooting hosted WebRTC

If hosted play is worse than localhost or server logs are flooded with SCTP parse warnings:

- Keep Cloudflare proxy only on the HTTPS bootstrap host (`TD_PUBLIC_HTTP_BASE`).
- Use a direct public socket for `--public-webrtc-addr` (IP:port), not a proxied Cloudflare hostname.
- Confirm UDP firewall/NAT forwarding for the WebRTC port (example: `5001/udp`).
- Consider a less common high UDP port for WebRTC in production to reduce background internet scanner traffic.
- Server default logging now suppresses noisy SCTP parser warnings from unrelated malformed packets.

## Validation

```bash
cargo fmt --all
cargo check --workspace --all-targets
cargo test --workspace
just check
```

## Netcode audit

The [September 2026 audit](docs/netcode-audit-2026-09-06.md) records protocol fixes,
regressions and remaining validation limits. Wire protocol 11 requires rebuilding
both server and clients; older binaries are incompatible.


### Input timeline and prediction

Movement is generated by fixed simulation steps and sent on an independent 30 Hz
real-time cadence. Reliable actions carry the last preceding movement sequence.
Unacknowledged actions also travel in redundant movement bundles, so channel ordering
or a lost reliable packet cannot let later movement bypass an unseen action.
The server consumes at most one movement per tick and releases an action only after
its preceding movement boundary. Both input queues are bounded at 64 entries;
movement bundles contain at most 16 moves and 64 actions (2 KiB message limit).

Snapshots restore deferred actions, movement queues and separate acknowledgment
watermarks. Local basic-attack effects use attack sequence IDs to avoid duplicates
when prediction and server confirmation race. Inputs carry a match epoch, preventing
late packets from a previous round from executing after reset.

See [the input timeline audit](docs/input-timeline-audit.md) for the code map,
regression coverage, and remaining browser/latency limitations.
