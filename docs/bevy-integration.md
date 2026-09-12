# Bevy integration and scheduling

The workspace uses Bevy 0.19.1. Use Bevy's ECS, geometry, transforms, timers,
relationships, scheduling, rendering and app lifecycle directly. Engine plugins
add gameplay and transport policy.

## Native build profiles

Ordinary native development uses optimization level 1 for workspace code and
level 3 for dependencies, following [Bevy's setup guidance](https://bevy.org/learn/quick-start/getting-started/setup/#compile-with-performance-optimizations).
This retains development assertions while avoiding an unoptimized ECS and
renderer. Native network qualification uses captured release builds of both
client and server; see [captured game-soak inputs](network-game-soak.md#captured-inputs).
Record the build profile, replication radius, rendered window size and sampled
frame durations together. A connection check with a small spatial fixture does
not establish performance at the normal gameplay replication radius.

## State and lifecycle

- Use `Mut`/`Changed` and `set_if_neq`; avoid dirtying idle components or maintaining
  parallel dirty flags.
- Keep authoritative planar movement in `MotorState`. Client transforms are
  derived graphics state. World axes are +Y up, -Z forward and +X right.
- `SimulationStep` supplies a fixed replay delta without advancing wall time.
  `SystemPlugin` does not create a clock or scheduler.
- Serializable gameplay countdowns retain their f32 arithmetic and reduction
  rules. Changing their representation requires checking tick boundaries and
  snapshot compatibility. Use Bevy `Timer` for ordinary app timers.
- `DelayedAction<T>` stores a replayable payload and sticky readiness. Bevy
  deferred commands handle world operations, not saved gameplay continuations.
- Spawn ownership uses native `linked_spawn` relationships. Rebuild runtime
  entity links from stable game IDs after snapshot restoration.

## Shared simulation schedule

[DreamwakePlugin](../games/dreamwake/simulation/src/plugin.rs) exposes `DreamStep`
and public `DreamSystems` phases. The game assigns engine sets to this sequence:

```text
graphics age → canonical inputs → actor timers → begin tick → ambient age
  → player actions → delayed timers/casts → companion targets → companions/shots
  → enemy actions → projectile targets → projectiles/impacts
  → depletion markers → deaths/XP → encounter transition → final health sync
```

Independent component timers remain unordered. Target refresh, RNG consumption,
damage and other dependent work have explicit edges. Stable game IDs determine
iteration order wherever outcomes depend on order.

`Active` and `Combat` are conditional parent sets. Their `run_if` results apply
throughout one schedule run. Inactive runs freeze; active noncombat runs age only
graphics and ambient state. Both health passes are required: deaths consume
current depletion markers, and encounter revival clears them before snapshots.

Use normal `.chain()`, `.before()` and `.after()` edges when consumers require
state created through `Commands`; Bevy inserts `ApplyDeferred` barriers. Do not
substitute ignore-deferred edges. Ordering applies within one schedule, and plugin
registration order is not system execution order. See the
[Bevy scheduling API](https://docs.rs/bevy_ecs/0.19.1/bevy_ecs/schedule/trait.IntoScheduleConfigs.html).

## Client and server schedules

Client `PreUpdate` orders receive/reconciliation, keyboard actions, UI buttons,
action application and input capture. Buttons and capture follow `UiSystems::Focus`
to consume current interaction state. `FixedUpdate` predicts, `Update` adapts and
renders, and `PostUpdate` flushes network packets.

Camera systems run in `Update`:

```text
GraphicsSet::Adapt → CameraSystems::Target → CameraSystems::Follow
  → CameraSystems::Zoom → GraphicsSet::Render
```

The [game camera plugin](../games/dreamwake/client/src/plugins/camera/mod.rs) owns
framing and targets; the engine owns the rig and smoothing. [Input capture](../games/dreamwake/client/src/plugins/input/capture.rs)
projects keyboard/gamepad axes into world space once. Cursor projection already
produces world coordinates. Graphics correction must not mutate gameplay state.

World labels update after `CameraUpdateSystems` and before `UiSystems::Content`.
Use `TransformHelper` to obtain the current camera transform, including ancestors,
when transform propagation has not yet run.
The engine's [client UI plugin](client-ui.md) exposes `UiSet::Billboards` and
`UiSet::Widgets` for shared world projection and meter updates at this boundary.

The server driver orders receive, bounded simulation steps, publication and send.
Its `TickClock` runs an immediate first tick, caps catch-up at six steps, discards
excess elapsed time and publishes only the latest completed state. This transport
policy uses real elapsed time independently of the client's virtual-time fixed
schedule. Client and server disconnect cleanup runs in `Last` after `ExitSystems`.

## Validation

[Schedule tests](../games/dreamwake/simulation/src/tests/scheduling.rs) cover ordering,
restore/replay, phase freezes and final health state. [Change-detection tests](../engine/core/tests/change_detection.rs)
cover timer transitions and idle state. Client tests cover input/UI ordering,
projection and graphics correction.

For browser changes, run `just check-web` and the [browser smoke test](../games/dreamwake/client/tests/web-smoke.mjs)
against running client/server builds. It takes the client URL and output directory;
set `PLAYWRIGHT_RUNTIME_DIR` to an installed Playwright runtime. Keep native windows
visible during screenshot capture; occluded Metal surfaces can yield black images.
