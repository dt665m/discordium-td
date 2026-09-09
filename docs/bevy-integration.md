# Bevy integration and scheduling contracts

This audit targets Bevy **0.19.1**, including the installed dependency source, the
[0.19 release notes](https://bevy.org/news/bevy-0-19/), and the
[migration guide](https://bevy.org/learn/migration-guides/0-18-to-0-19/).
The engine extends Bevy with reusable gameplay and networking policy; it is not
an alternative ECS, scheduler, math library, timer framework, or renderer.

## What belongs to Bevy

| Capability | Integration |
| --- | --- |
| ECS access and execution | Native components, resources, queries, plugins, schedules, system sets, and deferred commands. No engine scheduler or component registry. |
| Change detection | Bevy `Mut`/`Changed` and `set_if_neq`; idle action/motor/status/loadout systems avoid unnecessary mutable dereferencing. |
| Vector and primitive geometry | Bevy `Vec2`, `Circle::closest_point`, and `Segment2d::closest_point`. Array adapters retain the snapshot format and explicit input/tiny-sweep policies. |
| Ordinary transforms and timers | Bevy `Transform`, `GlobalTransform`, `Timer`, and ordinary focused systems, demonstrated by the headless drone example. The redundant `MovementOwner`/`CooldownOwner` plugin adapters were removed. |
| Rendering, cameras, UI, audio | Bevy owns projection, propagation, rendering, layout, focus, text, assets, and audio playback. Engine/game plugins supply scene data and presentation policy. |
| App loop and shutdown | `ScheduleRunnerPlugin`, `Time<Real>`, `AppExit`, and message readers/writers. No parallel application loop or exit-event framework. |

The following remain deliberately explicit:

- **Authoritative gameplay components:** Bevy does not supply this game's health,
  shield, committed attacks, cooldown-modifying combat, hit-once projectiles,
  summon ownership, or progression rules. Their state must survive snapshots.
- **Planar simulation coordinates:** `MotorState` is authoritative, serializable
  gameplay state, not a duplicate scene transform. Client `Transform`s are
  derived presentation state; the server does not run scene propagation.
- **Replay step:** `SimulationStep` supplies a fixed delta, not accumulated wall
  time. `DreamStep` can execute once for an authoritative command or repeatedly
  for reconciliation without advancing the outer application clock.
- **Gameplay countdown fields:** these retain the existing f32 subtraction,
  reduction/scaling rules, and wire representation. They are not a general
  replacement for [Bevy's timers](https://docs.rs/bevy/latest/bevy/time/struct.Timer.html).
  Migrating them to duration-based elapsed timers would change tick boundaries
  and require a deliberate gameplay/protocol migration.
- **Delayed gameplay payloads:** `DelayedAction<T>` is saved data with stable
  identity and sticky readiness, not a queue of closures. Bevy delayed commands
  are suitable for ordinary deferred world operations, not replay snapshots.
- **Transport cadence:** `TickClock` combines an immediate first tick, at most six
  catch-up steps, discarded overdue wall time, send cadence, and one coalesced
  publication of the latest completed state. It does not replace the Bevy app
  clock; [Bevy's fixed clock](https://docs.rs/bevy_time/latest/bevy_time/struct.Time.html)
  follows virtual time, including pause and speed changes, which is different
  from transport polling policy.

## Shared simulation schedule

[DreamwakePlugin](../games/dreamwake/simulation/src/plugin.rs) exposes one
`DreamStep` schedule and public `DreamSystems` integration points. The former
private nested schedules and exclusive dispatch wrappers are not needed in the
production simulation. Engine plugins expose their own public sets, and the
game assigns those sets to its phases.

```text
presentation age
  → canonical inputs → actor timers → begin tick → ambient age
  → player actions → delayed timers/casts
  → summon targets → summons/shots
  → enemy actions → projectile targets → projectiles/impacts
  → depletion markers → deaths → encounter transition → final health sync
```

The four actor timer systems use independent components and have no ordering
edges between them. Combat, target refresh, RNG consumption, and damage remain
ordered because their results depend on earlier work. Stable actor/projectile
IDs determine game iteration order; ECS allocation or worker order does not.
This enables parallel work where valid without claiming a measured speedup.
The final binary's Bevy feature selection determines its default executor:
dedicated server builds currently use single-threaded execution, while native
client builds enable multithreading. Bevy's multithreaded executor adds task-pool
coordination; benchmark representative server workloads before enabling it just
to distribute a few small timer systems. The app runner controls the loop, not
this feature choice.

`Active` and `Combat` are shared conditional parent sets. Bevy evaluates a set's
`run_if` once per schedule run, so a transition out of combat cannot accidentally
skip the final health synchronization. Inactive runs freeze; active noncombat
runs age only presentation/ambient state. Both health passes are intentional:
deaths need current depletion markers, and encounter-clear revival must remove
those markers before the completed snapshot.

Use normal `.chain()`/`.before()`/`.after()` dependency edges when consumers need
entities or components created through `Commands`. Bevy inserts the required
`ApplyDeferred` barriers. `chain_ignore_deferred` is not interchangeable here.
Ordering across two different schedules does nothing, and plugin registration
order is not execution order. These contracts are stated in
[Bevy's scheduling API](https://docs.rs/bevy_ecs/0.19.1/bevy_ecs/schedule/trait.IntoScheduleConfigs.html)
and implemented in `bevy_ecs/src/schedule/config.rs`.

## Client and server integration

Client `PreUpdate` explicitly orders receive/reconciliation, keyboard actions,
UI button routing, action application, and input capture. Buttons and capture
follow `UiSystems::Focus`, so they read current `Interaction` state. `FixedUpdate`
then predicts; `Update` adapts and renders presentation; `PostUpdate` flushes
network packets. Same-frame UI actions are no longer held until the next frame.

Camera follow runs in `Update` with explicit ordering:
`PresentationSet::Adapt` → `CameraSystems::Follow` → `PresentationSet::Render`.
Dreamwake owns framing, follow targets, and response settings; the engine owns
the reusable camera rig and uses Bevy `StableInterpolate::smooth_nudge` for focus
and zoom easing. Keyboard/gamepad movement and gamepad aim map screen axes once
through the actual camera basis into canonical XZ space. Mouse aim projects
directly onto the ground plane; autopilot input already uses world coordinates.
Renderer-only pose smoothing derives visual transforms without changing
authoritative or predicted gameplay state.

Legacy world labels update after `CameraUpdateSystems` and before
`UiSystems::Content`. Bevy's UI layout runs before transform propagation, so
placing labels after propagation was too late for that frame's layout. A single
current camera transform is computed per placement system with
[TransformHelper](https://docs.rs/bevy/latest/bevy/transform/helper/struct.TransformHelper.html),
including its ancestors, rather than reimplementing propagation. Verified in
`bevy_ui/src/lib.rs` and `bevy_transform/src/helper.rs`.

The server intentionally performs receive, bounded simulation steps, publish,
and send in that order inside one transport driver. Splitting these into
unordered systems would break the protocol contract. One non-send driver owns
the isolated simulation world; Bevy supplies thread-affinity scheduling.
Both server and client cleanup run in `Last` after `ExitSystems`, before Bevy's
runner checks `AppExit` following `app.update()`. Verified in
`bevy_app/src/schedule_runner.rs` and `bevy_window/src/lib.rs`.

## Regression coverage

- [Simulation schedule tests](../games/dreamwake/simulation/src/plugin_tests.rs):
  strict ambiguity detection, snapshot-for-snapshot comparison with the former
  nested sequence, restore/replay, delayed casts, summons, phase freezes,
  encounter revival, final depletion cleanup, and party death.
- [Change-detection test](../engine/core/tests/change_detection.rs): active,
  expiration, and idle transitions for the reusable timer components.
- Client UI/scene tests: same-frame button routing and attack suppression;
  current camera ancestry/projection visible before UI content/layout.
- Existing deterministic full-run, multiplayer, networking, and real transport
  tests remain part of workspace validation.

Native screenshot checks must keep the window visible. On the tested macOS/Metal
path, Bevy 0.19.1 can save a black PNG when the surface is occluded: no surface is
acquired, the screenshot copy is skipped, but the prepared buffer is still read.
A temporary always-visible window probe confirmed that native gameplay and
rendering remain correct. This is a capture constraint, not a reason to change
normal window behavior or add a custom rendering/screenshot subsystem.
