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

## Validation

```bash
cargo fmt --all
cargo check --workspace --all-targets
cargo test --workspace
just check
```
