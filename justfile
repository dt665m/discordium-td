set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

ROOT := justfile_directory()
WEB_CLIENT := ROOT + "/games/dreamwake/client"

default:
    @just --list

# Dreamwake is the canonical game. The engine crates do not choose a game.
play:
    cargo run -p dreamwake_client -- --host

dreamwake:
    cargo run -p dreamwake_client

client http_base="http://127.0.0.1:8080":
    cargo run -p dreamwake_client -- --http-base {{http_base}}

server max_clients="8":
    cargo run -p dreamwake_server -- --max-clients {{max_clients}}

dreamwake-server max_clients="8":
    just server {{max_clients}}

dreamwake-build:
    cargo build --release -p dreamwake_client

wasm-target:
    rustup target add wasm32-unknown-unknown

web-dev http_base="http://127.0.0.1:8080" address="127.0.0.1" port="1420":
    cd "{{WEB_CLIENT}}" && NO_COLOR=false GAME_WEB_HTTP_BASE="{{http_base}}" trunk serve --cargo-profile web-dev --dist "{{ROOT}}/out/dreamwake-web-dev" --address {{address}} --port {{port}}

dreamwake-web address="127.0.0.1" port="1421":
    just web-dev http://127.0.0.1:8080 {{address}} {{port}}

web-build http_base="http://127.0.0.1:8080":
    cd "{{WEB_CLIENT}}" && NO_COLOR=false GAME_WEB_HTTP_BASE="{{http_base}}" trunk build --release --cargo-profile web-release --dist "{{ROOT}}/target/web-release"

dreamwake-web-build http_base="https://dsdp.datab.fun":
    cd "{{WEB_CLIENT}}" && NO_COLOR=false GAME_WEB_HTTP_BASE="{{http_base}}" trunk build --release --cargo-profile web-release --dist "{{ROOT}}/target/dreamwake-pages"

fmt:
    cargo fmt --all

architecture:
    node scripts/check-architecture.mjs

check: architecture
    cargo check --workspace --all-targets

test:
    cargo test --workspace

check-web:
    cargo check -p dreamwake_client --target wasm32-unknown-unknown
