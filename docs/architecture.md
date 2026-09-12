# Engine and game boundaries

`engine/` provides reusable Bevy capabilities. `games/dreamwake/` supplies game
rules and composes those capabilities. Engine crates must not depend on a game
crate, including through build, test, or transitive workspace dependencies.

## Ownership

The engine owns generic mechanics, transport, input history, cameras, graphics
contracts and reusable client UI. Dreamwake owns characters, named abilities,
balance, Essence mappings, encounters, XP awards and curves, rewards, menus, audio
and art.

Authoritative gameplay belongs in ECS components. Complete snapshots save those
components alongside game payloads; views project this state rather than keeping
another mutable gameplay model. Server authority and client prediction both use
`DreamwakePlugin` through the [DreamSimulation wrapper](../games/dreamwake/simulation/src/simulation.rs).

## Core capabilities

Each capability registers its plugins in `plugins/<capability>/mod.rs`, with
components, policies and systems alongside it. The [core plugin tree](../engine/core/src/plugins/mod.rs)
exposes:

| Plugin | Responsibility |
| --- | --- |
| `SystemPlugin::new(dt)` | Supplies the game-selected `SimulationStep` delta |
| `AbilitiesPlugin` | Committed actions; typed `LoadoutPlugin` and `DelayedActionPlugin` are opt-in |
| `CombatPlugin` | Health, shields, statuses, damage, projectiles and companions |
| `PhysicsPlugin` | Authoritative planar movement, bounds and spatial helpers |
| `SpawnPlugin` | Optional simulation lifetimes and Bevy ownership cleanup |
| `ProgressionPlugin` | Actor XP/levels using a supplied curve, with upgrade arithmetic helpers |
| `GraphicsPlugin` | Serializable effect identities and simulation lifetimes, without GPU dependencies |

Games choose schedules and order the public system sets. Combat can disable
projectile candidate collection when effects must resolve sequentially against
live targets. See [Bevy integration](bevy-integration.md) for scheduling rules.

Optional mechanics remain component-driven:

- `Meter<K>` bounds current value and capacity and rejects invalid or unaffordable
  spending atomically. `MeterPlugin<K, S>` regenerates actors carrying both
  `Meter<K>` and `Regeneration<K>` using the supplied rate and simulation delta.
- `ProgressionPlugin::new(schedule, curve)` consumes actor-local
  `PendingExperience` and records `ExperienceResult`. `Progression::gain` provides
  the same arithmetic synchronously. Games supply XP awards, thresholds and rewards.
- `Lifetime` expiry uses Bevy commands. `DespawnWithOwner` and `OwnedSpawns` form
  a `linked_spawn` relationship for recursive cleanup. Runtime `Entity` links
  must be rebuilt from stable game IDs when restoring snapshots.

## Game and host composition

[DreamwakePlugin](../games/dreamwake/simulation/src/plugin.rs) installs the core
capabilities, typed ability plugins and game systems. `DreamInputs` supplies one
step's inputs; `DreamStep` is explicitly stepped at 60 Hz for authority and replay.
The [state module](../games/dreamwake/simulation/src/state/mod.rs) owns saved actor
state and restoration.

```rust
use bevy::prelude::*;
use dreamwake_sim::{DreamSimulation, DreamwakePlugin};

let mut app = App::new();
app.add_plugins(DreamwakePlugin::new(8192, false));
let mut simulation = DreamSimulation::from_app(app);
simulation.continue_run();
simulation.step(Default::default());
```

The [engine server plugin](../engine/server/src/plugins/server/mod.rs) owns polling,
fixed ticks, publication and shutdown around one non-send `ServerDriver<A>`.
The [game server plugin](../games/dreamwake/server/src/plugins/server/mod.rs) owns
admission, validation, simulation and party policy. Dedicated and native-hosted
servers use the same `dreamwake_server::build_app` composition. See
[networking](netcode.md) for sequencing and snapshot rules.

[Engine client plugins](../engine/client/src/plugins/mod.rs) group camera, graphics,
UI and network tools. [Game client plugins](../games/dreamwake/client/src/plugins/mod.rs)
group camera, graphics, input, audio, UI, network and diagnostics. Input owns one
mapping into world space; camera framing is independent of art. The engine's
client [UI plugin](client-ui.md) provides screen-facing world anchors, basic meter and label scenes,
and an optional diagnostics panel with frame metrics. Games supply meter values,
colors, typography, menus and extra diagnostics rows. These presentation features
live in `engine_client`, keeping `engine_core` usable by headless simulations.
Game diagnostics owns telemetry and development controls.

`DreamGraphicsPlugin` adapts snapshots into `engine_client::graphics::GraphicsFrame`.
The client's `build_app()` leaves the renderer selectable; `run()` installs
`DreamScenePlugin`. Renderer meshes, materials and correction blending remain
separate from [simulation-owned effects](predicted-graphics.md).

## Adding capabilities or games

Keep reusable mechanics driven by game-supplied data, and preserve complete
snapshot restore/replay. Start from the independent [gameplay example](../engine/core/examples/gameplay.rs)
and [headless tests](../engine/core/tests/gameplay.rs); add a game package only for
its rules and payloads. Run `just architecture` when changing plugin boundaries
or dependencies, and validate affected behavior with the relevant tests.
