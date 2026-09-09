# Building and deploying the canonical game

The current application is built from `games/dreamwake/`. The engine is a set of
libraries; it is not a separately named game or deployment. This reorganization
does not deploy or restart any existing service.

## Native builds

```sh
cargo build --release --locked -p dreamwake_client -p dreamwake_server
```

Outputs are `target/release/dreamwake` and `target/release/game_server`. The server
always runs the canonical game; the old `--dreamwake` switch is unnecessary.
Game options are `--seed`, `--lucid`, and the shared network configuration options
reported by `game_server --help`.

```sh
target/release/game_server --max-clients 8 \
  --http-bind 0.0.0.0:8080 \
  --udp-bind 0.0.0.0:5000 \
  --webrtc-bind 0.0.0.0:5001 \
  --public-http-base https://YOUR_SERVER \
  --public-udp-addr YOUR_IP:5000 \
  --public-webrtc-addr YOUR_IP:5001 \
  --cors-allowed-origins https://YOUR_CLIENT
```

## Configuration migration

Shared server environment variables now use `ENGINE_` in place of `TD_`:
`ENGINE_HTTP_BIND`, `ENGINE_HTTP_TLS_CERT`, `ENGINE_HTTP_TLS_KEY`,
`ENGINE_UDP_BIND`, `ENGINE_WEBRTC_BIND`, `ENGINE_PUBLIC_HTTP_BASE`,
`ENGINE_PUBLIC_UDP_ADDR`, `ENGINE_PUBLIC_WEBRTC_ADDR`,
`ENGINE_CORS_ALLOWED_ORIGINS`, and `ENGINE_MAX_CLIENTS`.
Command-line network flags keep their existing unprefixed spellings.

Update service environment entries when installing the new binary. Existing
TLS, CORS, and publicly advertised addresses must be preserved explicitly; a
renamed configuration variable must not silently select a local development
default in production. The old TD-specific server UI, debug bridge, and recorder
flags are retired with that application.

The extracted actor state changes the game wire contract. Rebuild and deploy
client and server together; protocol identity rejects pre-consolidation clients.
Keep a matching previous client/server pair for rollback.

## Browser

Install Trunk and the `wasm32-unknown-unknown` target. From the root:

```sh
just web-build https://YOUR_SERVER
```

Publish the contents of `target/web-release/` to the chosen static host.
`GAME_WEB_HTTP_BASE` is the compile-time endpoint used by the game client.
The query parameter `server=` overrides it for a playtest. Prototype graphics
are the default; `renderer=legacy` selects the optional procedural art plugin.

Before publishing, check compressed and uncompressed artifact sizes against the
host's limits. After publication, verify an actual WebRTC connection as well as
the server's `/healthz` endpoint. Native UDP should be checked separately.

The [2026-09-08 deployment record](history/deployment-2026-09-08.md) preserves
the existing host, service, release IDs, and rollback locations. Its old build
commands and environment names describe that release, not this workspace.
