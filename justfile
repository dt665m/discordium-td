set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default:
    @just --list

check:
    cargo check --workspace --all-targets

test:
    cargo test --workspace

server http_bind="127.0.0.1:8080" udp_bind="127.0.0.1:5000" webrtc_bind="127.0.0.1:5001":
    cargo run -p game_server -- \
      --http-bind {{http_bind}} \
      --udp-bind {{udp_bind}} \
      --webrtc-bind {{webrtc_bind}} \
      --public-http-base http://127.0.0.1:8080 \
      --public-udp-addr 127.0.0.1:5000 \
      --public-webrtc-addr 127.0.0.1:5001

client http_base="http://127.0.0.1:8080":
    cargo run -p game_client -- --http-base {{http_base}}

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
      --udp-bind 127.0.0.1:5000 \
      --webrtc-bind 127.0.0.1:5001 \
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
