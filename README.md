# Bevy game plugins and Dreamwake

This workspace develops reusable Rust/Bevy plugins and **Dreamwake**, the canonical
cooperative action game built with them. The previous tower-defense application
has been retired; its source and the pre-consolidation prototype are recoverable
from Git checkpoint `e834361`.

The engine owns reusable mechanics, transport, diagnostics, and presentation
interfaces. Dreamwake owns its characters, named abilities and balance,
encounters, progression rules, menus, audio, and game protocol. Engine crates
never depend on a game crate.

## Run

Requires Rust 1.95 or newer. Bevy 0.19.1 is selected centrally in `Cargo.toml`.

```sh
just play                          # Native client hosting a local party
just server                        # Dedicated server, eight players by default
just client http://127.0.0.1:8080   # Join the dedicated server
```

Dreamwake starts with its procedural game graphics. Development builds include
the `debug-tools` feature by default: open F6 and toggle **Gizmos** to replace the game graphics with
presentation primitives at runtime. Gizmos start off. Build with
`--no-default-features` to omit these development controls and gizmo systems.

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
| `engine/core` | ECS action, motor, combat, loadout, projectile, summon and presentation plugins; progression helpers |
| `engine/net` | Bounded codec, generic envelopes, per-peer sequenced inboxes and clock utilities |
| `engine/server` | Generic Bevy server runtime, HTTP bootstrap and mixed UDP/WebRTC transport |
| `engine/client` | Camera, presentation contract/renderers, prediction utilities and diagnostics |
| `engine/net-debug` | Vendored packet-conditioner UI, with original licenses |
| `games/dreamwake/simulation` | Game-specific simulation, abilities, enemies, encounters and rewards |
| `games/dreamwake/protocol` | Game actions and concrete network payload types |
| `games/dreamwake/server` | Game authority and dedicated-server composition |
| `games/dreamwake/client` | Game input, snapshot adapters, menus, audio and optional art |

See [architecture and prototyping](docs/architecture.md) before adding mechanics
or another game. A new game should compose engine plugins and supply its own
rules and presentation adapter.

`dreamwake_sim::DreamwakePlugin` composes the shared gameplay used by server
authority and client prediction. `DreamwakeServerPlugin` installs game authority
on `engine_server::runtime::ServerPlugin`; dedicated and native-hosted servers
run that same composition. Complete snapshots save the engine components and
game payloads needed for replay. The saved-state layout uses protocol version 3;
rebuild clients and servers together.

The standalone [gameplay example](engine/core/examples/gameplay.rs) and
[headless tests](engine/core/tests/gameplay.rs) show engine mechanics composed
without a game dependency.

## Validate

```sh
cargo fmt --all -- --check
just check       # Dependency/naming boundaries and native workspace targets
just test        # Engine mechanics, game replay and real UDP multiplayer tests
just check-web   # Browser client target
cargo run -p engine_core --example sandbox  # Independent headless prototype
cargo run -p engine_core --example gameplay # Reusable gameplay plugin composition
```

`scripts/check-architecture.mjs` rejects engine-to-game dependencies (including
transitive workspace dependencies) and game-specific naming in engine Rust code.
Runtime tests remain necessary: generic names alone do not establish reuse.

Licensed under MIT or Apache-2.0. Embedded fonts and vendored code retain their
own included license notices.
