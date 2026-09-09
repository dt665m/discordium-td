# Bevy game plugins and Dreamwake

This workspace develops reusable Rust/Bevy plugins and **Dreamwake**, the canonical
cooperative action game built with them. The previous tower-defense application
has been retired; its source and the pre-consolidation prototype are recoverable
from Git checkpoint `e834361`.

The engine owns reusable mechanics, transport, diagnostics, and presentation
interfaces. Dreamwake owns its characters, abilities, encounters, progression,
menus, audio, and game protocol. Engine crates never depend on a game crate.

## Run

Requires Rust 1.95 or newer. Bevy 0.19.1 is selected centrally in `Cargo.toml`.

```sh
just play                          # Native client hosting a local party
just server                        # Dedicated server, eight players by default
just client http://127.0.0.1:8080   # Join the dedicated server
```

The default renderer uses simple prototype shapes. The previous procedural art is
an optional renderer, selected with `--renderer legacy`; changing the renderer
does not change the simulation, protocol, or input mapping.

For the browser, install Trunk and the WASM target, start a server, then run:

```sh
just wasm-target
just web-dev
```

The browser uses WebRTC; native clients use UDP. See the [play guide](docs/dreamwake.md)
for controls and the [deployment guide](docs/deployment.md) for packaging.

## Code ownership

| Location | Responsibility |
| --- | --- |
| `engine/core` | Renderer-independent ECS components and simulation plugins |
| `engine/net` | Bounded codec, generic network envelopes, sequence and clock utilities |
| `engine/server` | Shared HTTP bootstrap and mixed UDP/WebRTC server infrastructure |
| `engine/client` | Camera, presentation contract/renderers, prediction utilities and diagnostics |
| `engine/net-debug` | Vendored packet-conditioner UI, with original licenses |
| `games/dreamwake/simulation` | Game-specific simulation, abilities, enemies, encounters and rewards |
| `games/dreamwake/protocol` | Game actions and concrete network payload types |
| `games/dreamwake/server` | Game authority and dedicated-server composition |
| `games/dreamwake/client` | Game input, snapshot adapters, menus, audio and optional art |

See [architecture and prototyping](docs/architecture.md) before adding mechanics
or another game. A new game should compose engine plugins and supply its own
rules and presentation adapter.

## Validate

```sh
cargo fmt --all -- --check
just check       # Dependency/naming boundaries and native workspace targets
just test        # Engine mechanics, game replay and real UDP multiplayer tests
just check-web   # Browser client target
cargo run -p engine_core --example sandbox  # Independent headless prototype
```

`scripts/check-architecture.mjs` rejects engine-to-game dependencies (including
transitive workspace dependencies) and game-specific naming in engine Rust code.
Runtime tests remain necessary: generic names alone do not establish reuse.

Licensed under MIT or Apache-2.0. Embedded fonts and vendored code retain their
own included license notices.
