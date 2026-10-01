# Unnamed Game Engine

This workspace develops reusable Rust/Bevy Game Engine and Server-side Network Stack

## Run

Requires Rust 1.95 or newer and `just`. Bevy 0.19.1 is selected centrally in
`Cargo.toml`.

```sh
just play                          # Native client hosting a local party
just server                        # Dedicated server, eight players by default
just client http://127.0.0.1:8080   # Join the dedicated server
```

Dreamwake starts with procedural game graphics. F3 shows performance metrics and
F6 opens network tools. The default-enabled `debug-tools` feature adds **Gizmos**
and autoplay controls; build with `--no-default-features` to omit those extras.

For the browser, install Trunk and the WASM target, start a server, then run:

```sh
just wasm-target
just web-dev
```

Open [the local browser client](http://127.0.0.1:1420). To select another server,
run `just web-dev http://SERVER_ADDRESS:8080`. The server address is configured at
build time. Browser clients use WebRTC; native clients use UDP.

## Code ownership

| Location | Responsibility |
| --- | --- |
| `engine/core` | System, abilities, combat, physics, spawn, progression and simulation graphics plugins |
| `engine/net` | Bounded codec, generic envelopes, per-peer sequenced inboxes and clock utilities |
| `engine/server` | Generic Bevy server runtime, HTTP bootstrap and mixed UDP/WebRTC transport |
| `engine/client` | Camera, graphics contract/renderers, prediction utilities and network tools |
| `engine/net-debug` | Vendored packet-conditioner UI, with original licenses |
| `games/dreamwake/simulation` | Game-specific simulation, abilities, enemies, encounters and rewards |
| `games/dreamwake/protocol` | Game actions and concrete network payload types |
| `games/dreamwake/server` | Game authority and dedicated-server composition |
| `games/dreamwake/client` | Game input, snapshot adapters, menus, audio and optional art |

## Guides

- [Play Dreamwake](docs/dreamwake.md): hosting, controls, party flow and builds.
- [Architecture](docs/architecture.md): ownership, plugin composition and adding a game.
- [Bevy integration](docs/bevy-integration.md): ECS and scheduling contracts.
- [Networking](docs/netcode.md): authority, transport, snapshots and reconciliation.
- [Predicted graphics](docs/predicted-graphics.md): simulation-owned effect lifecycles.
- [Deployment](docs/deployment.md): dedicated server operations and browser releases.

## Validate

```sh
cargo fmt --all -- --check
just check       # Dependency/naming boundaries and native workspace targets
just test        # Engine mechanics, game replay and real UDP multiplayer tests
just check-web   # Browser client target
cargo run -p engine_core --example sandbox  # Independent headless prototype
cargo run -p engine_core --example gameplay # Reusable gameplay plugin composition
```

The [architecture check](scripts/check-architecture.mjs) enforces plugin entry
points and engine/game boundaries. The [gameplay example](engine/core/examples/gameplay.rs)
and [headless tests](engine/core/tests/gameplay.rs) demonstrate independent engine use.

Licensed under MIT or Apache-2.0. Embedded fonts and vendored code retain their
own included license notices.
