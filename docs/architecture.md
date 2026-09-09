# Engine and game boundaries

Dreamwake is the current game; Bevy and the plugins under `engine/` are its
foundation. New games depend on the same engine crates and provide their own
game rules. No engine crate may depend on `games/`, including through a test,
build dependency, or another workspace crate.

## Ownership

Engine code describes reusable capabilities: health, motion, cooldowns,
simulation ticks, presentation identities, network sessions, bounded messages,
input histories, cameras, and render primitives. It does not know characters,
abilities, room counts, rewards, game titles, or art palettes.

Game code composes those capabilities. Dreamwake chooses its tick rate and
snapshot cadence, ability rules, enemy behavior, encounter phases, rewards,
interface, music, and server-authoritative acceptance rules. These are explicit
game choices, not defaults for every future prototype.

Authoritative actor state belongs in ECS components. Snapshot structs are
projections or saved state used for transport and replay; they must not become a
second independently mutable authority for health or movement. Client prediction
executes the same game simulation as the server, restores complete authoritative
state, and replays bounded pending inputs.

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
example using health, movement, cooldown, and presentation plugins. It has no
game dependencies and exercises movement, damage, repair, and effect expiry.

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
