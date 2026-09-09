# Engine and game boundaries

Dreamwake is the current game; Bevy and the plugins under `engine/` are its
foundation. New games depend on the same engine crates and provide their own
game rules. No engine crate may depend on `games/`, including through a test,
build dependency, or another workspace crate.

## Ownership

Engine code describes reusable capabilities: health, action locks, motion,
shields, immunity, timed statuses, cooldowns, projectile and summon lifecycles,
targeting, progression arithmetic, presentation identities, network sessions,
input histories, cameras, and render primitives. Generic ability machinery is
allowed: a loadout can hold game-supplied ability and modifier types, and a delayed
action can carry game payloads. Named abilities, balance values, encounter rules,
reward tables, characters, and art palettes remain under `games/`.

Game code composes those capabilities. Dreamwake chooses its tick rate and
snapshot cadence, ability rules, enemy behavior, encounter phases, rewards,
interface, music, and server-authoritative acceptance rules. These are explicit
game choices, not defaults for every future prototype.

Authoritative actor state belongs in ECS components. Snapshot structs are
projections or saved state used for transport and replay; they must not become a
second independently mutable authority for health or movement. Client prediction
executes the same game simulation as the server, restores complete authoritative
state, and replays bounded pending inputs. Engine mechanics are independent ECS
components, alongside game-specific actor and ability payloads; saved state
includes both. A view field such as displayed shield is a projection of its
combat component, not another gameplay store.

## Gameplay and server composition

`dreamwake_sim::DreamwakePlugin::new(seed, lucid)` installs the shared gameplay
resources and schedules, including the initial player. `DreamSimulation::new(seed,
lucid)` wraps that plugin for both server authority and client prediction. For
explicit composition:

```rust
use bevy::prelude::*;
use dreamwake_sim::{DreamSimulation, DreamwakePlugin};

let mut app = App::new();
app.add_plugins(DreamwakePlugin::new(8192, false));
let mut simulation = DreamSimulation::from_app(app);
simulation.continue_run();
simulation.step(Default::default());
```

The public `DreamInputs` resource supplies inputs to the explicitly stepped
`DreamStep` schedule. The wrapper drives that schedule at the game's fixed 60 Hz,
independently of wall time, for authority and replay. Its isolated simulation world
is intentional; client/server outer Apps do not contain a second set of game
rules. The game explicitly composes
the engine's action, motor, combat, loadout, projectile, summon, delayed-action,
health, and presentation plugins in its simulation schedules. A new game can
select only the capabilities it needs; the engine has no Dreamwake dependency.
Public engine sets are assigned to ordered `DreamSystems` phases in one
`DreamStep` schedule. See [Bevy integration](bevy-integration.md) for source-backed
scheduling, component-access, and non-duplication contracts.

Dreamwake supplies attack/combo definitions, Essence mappings, enemy decisions,
difficulty, encounter transitions, rewards, late admission, and party policy.
The engine applies supplied timings, magnitudes, caps, and lifecycle policies.
For example, shield grants can add with or without a cap, and immunity grants
can replace a longer duration with a shorter one. Those choices preserve the
game's existing behavior. Sanctuary recovery remains 50% total maximum health
(10% room entry plus 40% sanctuary recovery); its message now matches that total.

`engine_server::runtime::ServerPlugin<A>` drives one authority through transport
polling, fixed ticks, publication, and graceful exit. It holds a single non-send
`ServerDriver<A>`, allowing that authority to own its simulation world.
`DreamwakeServerPlugin` installs the game authority, and
`dreamwake_server::build_app(shared, seed, lucid)` composes it with a headless
runner. Dedicated and native-hosted servers use this same composition.
`engine_net::session::PeerInbox<I, A>` owns sequencing, bounded command queues,
message budgets, freshness, and received/applied acknowledgements. The game
retains authenticated roster indexing and validates channels and game payloads.

## Graphics

The game converts its displayed snapshot into the engine's presentation data.
Renderers consume positions, stable identities, shapes, colors and other visual
parameters without importing game simulation types. A camera plugin provides
the camera/input basis independently of a particular renderer.

Simple prototype graphics are the default. The original procedural scene is an
optional game-side art plugin. Game networking and simulation never spawn
meshes or materials, and input never requires that legacy scene plugin.

Replacing art therefore consists of supplying a renderer/presentation adapter;
it does not require a new simulation, authority loop, or wire protocol. UI and
audio are separate game plugins, so prototypes can compose only what they need.

## Adding another prototype

Run `cargo run -p engine_core --example sandbox` for a small headless drone
example using engine health/presentation alongside Bevy transforms/timers and
ordinary focused systems. For the
gameplay components, run `cargo run -p engine_core --example gameplay`; its
[source](../engine/core/examples/gameplay.rs) composes mechanics without importing
a game. The [headless gameplay tests](../engine/core/tests/gameplay.rs) exercise
the same reusable component/plugin boundary.

The browser smoke test at `games/dreamwake/client/tests/web-smoke.mjs` opens two
WebRTC clients using different renderers against one server and captures both
views plus diagnostics. Install Playwright in a separate runtime directory, set
`PLAYWRIGHT_RUNTIME_DIR` to it, then run the script with the client URL, server URL,
and an output directory under `target/` as its three arguments. The server and
Trunk must already be running.

1. Create a game package under `games/` and depend on the engine crates it needs.
2. Compose the generic simulation plugins with game-specific systems. Keep
   scheduling dependencies explicit and world coordinates canonical: +Y up,
   -Z forward, +X right.
3. Define that game's input/actions and complete snapshot state. Use the shared
   bounded network envelopes and transport infrastructure for multiplayer.
4. Project the displayed state into the generic presentation contract and
   install a renderer plus the independent camera plugin.
5. Test simulation without graphics, snapshot restore/replay, action rejection,
   and the native/browser paths the prototype supports.

Avoid creating a universal game enum in the engine or moving a whole game's
systems into `engine/` after renaming its structs. Extract a capability when its
inputs, state ownership, and behavior are meaningful independently of the game.

## Consolidation history

Checkpoint `e834361` records the complete prototype and accumulated TD fixes
before reorganization. Dreamwake already contained local `main`'s committed
refactor; the consolidation did not start from a separate divergent history.

The retired TD application, delta protocol, worker integration, and TD-specific
debug recorder/verifier remain recoverable in that checkpoint. Dreamwake uses
its full-state prediction/replay protocol. TD-only algorithms are not silently
substituted into that protocol; reusable transport fixes and codec behavior are
preserved in the engine dependency stack and extracted infrastructure.
