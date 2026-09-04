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

`debug_recorder=1` also enables the browser bridge and uploads chunked frame batches to `/debug/client-frames` with retry-aware `fetch` POSTs.

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
- write a JSON report to `target/netcode-verifier/YYYY-MM-DD/<run_id>/report.json`

The wrapper prepares Playwright in an OS-temporary runtime, so the repo does not need a local `package.json` or `node_modules/` for browser verification. Set `PLAYWRIGHT_RUNTIME_DIR` to override that location.

Headless Chromium is not reliable for this Bevy wasm client in the current setup because WebGL surface creation can fail there.

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
