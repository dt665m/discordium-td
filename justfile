set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

ROOT := justfile_directory()
WEB_CLIENT := ROOT + "/game_client"

default:
    @just --list

check:
    cargo check --workspace --all-targets

test:
    cargo test --workspace

server http_bind="127.0.0.1:8080" udp_bind="0.0.0.0:5000" webrtc_bind="0.0.0.0:5001":
    cargo run --release -p game_server -- \
      --http-bind {{http_bind}} \
      --udp-bind {{udp_bind}} \
      --webrtc-bind {{webrtc_bind}} \
      --public-http-base http://127.0.0.1:8080 \
      --public-udp-addr 127.0.0.1:5000 \
      --public-webrtc-addr 127.0.0.1:5001

client http_base="http://127.0.0.1:8080":
    cargo run -p game_client -- --http-base {{http_base}}

# One-time setup for wasm client builds.
wasm-target:
    rustup target add wasm32-unknown-unknown

web-build http_base="http://127.0.0.1:8080":
    cd "{{WEB_CLIENT}}" && \
      NO_COLOR=false \
      TD_WEB_HTTP_BASE="{{http_base}}" \
      trunk build --release --cargo-profile web-release --config Trunk.toml

web-client http_base="http://127.0.0.1:8080" address="127.0.0.1" port="1420":
    cd "{{WEB_CLIENT}}" && \
      NO_COLOR=false \
      TD_WEB_HTTP_BASE="{{http_base}}" \
      trunk serve --release --cargo-profile web-release --config Trunk.toml --address {{address}} --port {{port}}

web-deploy:
    npx wrangler pages deploy /Users/dt665m/Projects/games/discordium-td/game_client/dist --project-name=discordium-td

web-play http_base="http://127.0.0.1:8080" address="127.0.0.1" port="1420":
    #!/usr/bin/env bash
    set -euo pipefail

    cd "{{ROOT}}"

    cleanup() {
      if [[ -n "${server_pid:-}" ]] && kill -0 "$server_pid" 2>/dev/null; then
        kill "$server_pid" 2>/dev/null || true
        wait "$server_pid" 2>/dev/null || true
      fi
    }
    trap cleanup EXIT INT TERM

    cargo run -p game_server -- \
      --http-bind 127.0.0.1:8080 \
      --udp-bind 0.0.0.0:5000 \
      --webrtc-bind 0.0.0.0:5001 \
      --public-http-base http://127.0.0.1:8080 \
      --public-udp-addr 127.0.0.1:5000 \
      --public-webrtc-addr 127.0.0.1:5001 \
      > /tmp/discordium-td-server.log 2>&1 &
    server_pid=$!

    for _ in {1..80}; do
      if curl -sf http://127.0.0.1:8080/healthz > /dev/null; then
        break
      fi
      sleep 0.25
    done

    echo "Server started (pid=$server_pid). Logs: /tmp/discordium-td-server.log"
    echo "Starting trunk on http://${address}:${port} (web bootstrap base: ${http_base})"
    cd "{{WEB_CLIENT}}" && \
      NO_COLOR=false \
      TD_WEB_HTTP_BASE="{{http_base}}" \
      trunk serve --release --cargo-profile web-release --config Trunk.toml --address {{address}} --port {{port}}

play:
    #!/usr/bin/env bash
    set -euo pipefail

    cd "{{justfile_directory()}}"

    cleanup() {
      if [[ -n "${server_pid:-}" ]] && kill -0 "$server_pid" 2>/dev/null; then
        kill "$server_pid" 2>/dev/null || true
        wait "$server_pid" 2>/dev/null || true
      fi
    }
    trap cleanup EXIT INT TERM

    cargo run -p game_server -- \
      --http-bind 127.0.0.1:8080 \
      --udp-bind 0.0.0.0:5000 \
      --webrtc-bind 0.0.0.0:5001 \
      --public-http-base http://127.0.0.1:8080 \
      --public-udp-addr 127.0.0.1:5000 \
      --public-webrtc-addr 127.0.0.1:5001 \
      > /tmp/discordium-td-server.log 2>&1 &
    server_pid=$!

    for _ in {1..80}; do
      if curl -sf http://127.0.0.1:8080/healthz > /dev/null; then
        break
      fi
      sleep 0.25
    done

    echo "Server started (pid=$server_pid). Logs: /tmp/discordium-td-server.log"
    cargo run -p game_client -- --http-base http://127.0.0.1:8080
