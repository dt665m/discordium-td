# AGENTS.md

This file defines project-specific guidance for coding agents working in this repository.

## Scope

- Applies to the entire workspace rooted at `discordium-td/`.
- Favor consistency and correctness over ad-hoc local fixes.

## Engine and game ownership

- `engine/` contains reusable Bevy plugins and supporting services. It must not
  depend on a package under `games/`, even through tests or build dependencies.
- Keep engine components and plugins named for capabilities: health, movement,
  actions, combat, cooldowns, loadouts, projectiles, summons, presentation,
  prediction, transport, and sessions. Generic ability machinery and progression
  arithmetic belong in the engine when driven by game-supplied data. Game names,
  characters, named ability/combo definitions, balance, Essence mappings,
  encounters, reward/progression rules, and art styles belong under `games/`.
- Compose shared gameplay through `dreamwake_sim::DreamwakePlugin`; server
  authority and client prediction use its `DreamSimulation` wrapper. Save engine
  components alongside game payloads in complete authoritative snapshots rather
  than maintaining duplicate gameplay state in display views.
- Keep generic server scheduling/lifecycle in `ServerPlugin<A>` and generic
  sequencing/queue/budget/freshness mechanics in `PeerInbox<I, A>`.
  `DreamwakeServerPlugin` owns game admission, validation, snapshots, and party
  policy. Dedicated and native-hosted production paths must use that composition.
- `games/dreamwake/` is the canonical game. The original tower-defense game is
  retired and recoverable from Git checkpoint `e834361`.
- Graphics are replaceable plugins consuming presentation data. Input/camera and
  gameplay must not require a particular mesh, material, or art plugin.
- Run `just architecture` when changing crate dependencies or engine boundaries.
  See `docs/architecture.md` for the ownership contract.
- Do not wrap or reimplement capabilities already provided by Bevy. Use its
  math/geometry, transforms, timers, change detection, scheduling, and lifecycle
  APIs directly. Keep extra machinery only for explicit gameplay or transport
  requirements; see `docs/bevy-integration.md` for the retained exceptions.

## Browser configuration

- Do not build browser URL parameter options into the game. This includes query
  parameters and fragment options for configuration, gameplay, rendering,
  debugging, or test automation. Use in-game controls or build/deployment
  configuration instead.

## Bevy 0.19 Core Conventions

- Keep Bevy's version centralized in `[workspace.dependencies]`; the workspace targets 0.19.1 and Rust 1.95 or newer.
- Consult the [Bevy 0.19 release blog](https://bevy.org/news/bevy-0-19/) and [0.18 to 0.19 migration guide](https://bevy.org/learn/migration-guides/0-18-to-0-19/) for API and convention changes.

- Coordinate system:
  - `+Y` is up.
  - Forward is `-Z`.
  - Right is `+X`.
- Keep simulation/world logic in canonical world space.
- Treat camera orientation as presentation.
- If controls are camera-relative, transform input from camera basis to world basis in one place only.

## Input and Directionality Rules

- Do not stack multiple axis flips in different systems.
- Keep a single source of truth for input mapping (`capture_input`-style system).
- If movement feels mirrored or rotated:
  - Verify camera transform and up vector first.
  - Verify world-to-scene conversion second.
  - Verify input projection last.
- Prefer adding temporary axis debug visuals over guessing sign flips.

## ECS Patterns (Required)

- Put game state in components/resources; avoid parallel ad-hoc maps unless strictly needed for indexing.
- Use reusable components for shared mechanics (e.g., facing, attack profile, attack state, health/mana).
- Prefer data-driven systems over branching by entity id or player id.
- Keep rendering-only effects in client-only components/systems.
- Server remains authoritative for gameplay state transitions.
- Resources are now components on dedicated entities. Do not derive both `Resource` and `Component`; scope broad entity queries so they do not accidentally include resources. `World::clear_entities` also clears resources.
- Use `insert_non_send`, `get_non_send_mut`, and `remove_non_send` for non-send data instead of the deprecated `*_non_send_resource` APIs.

## Scene and UI Composition

- Prefer BSN (`bsn!`, `bsn_list!`) scene functions for reusable UI and static hierarchies; use `spawn_scene` or a scene function's `.spawn()` system. The game interface in `games/dreamwake/client/src/ui.rs` is an example.
- Keep dynamic simulation/presentation lifecycles in their existing ECS systems; scene composition does not replace simulation ownership.
- Use `FontSize::Px` for fixed-size text in Rust structs, or `px(...)` in BSN. Use `FontSource` for font selection and the current `TextLayout::justify`, `linebreak`, and `no_wrap` constructors.
- `Assets::get_mut` returns `AssetMut`; bind it as `mut` and pass `&mut asset` to helpers. Only mutate assets when their values actually change so change detection can avoid unnecessary GPU work.
- Lights use `shadow_maps_enabled`; contact shadows are a separate opt-in feature. Custom render passes belong in the `Core3d`/`Core2d` schedules using explicit system ordering.
- When selecting Bevy feature collections, opt into `ui` and `audio` explicitly if using `3d`/`2d` without the default feature set.

## System Ordering and Dependencies

- Explicitly order dependent systems using `.chain()` or explicit schedules.
- Expose public system sets from reusable plugins, and assign them to the game's
  ordered phases. Keep independent systems unordered within a phase. Normal
  dependency edges apply deferred commands; do not substitute ignore-deferred
  edges where a consumer requires newly spawned state.
- Request only component access a system actually uses, and avoid mutating idle
  components. Prefer Bevy change-detection helpers over parallel dirty flags.
- Recommended high-level order per frame:
  - Input capture
  - Network receive/apply snapshot
  - Prediction/reconciliation
  - Gameplay visual sync
  - FX / UI updates
- Fixed-step gameplay commands should run in `FixedUpdate` or an explicitly
  stepped simulation schedule shared by authoritative execution and replay.
- Do not rely on incidental ordering from registration order when dependency is real.
- Window close/exit systems run in `Last` in Bevy 0.19. Run graceful disconnect and other `AppExit` cleanup after `bevy::window::ExitSystems` so it executes in the final frame.

## Networking and Prediction

- Authoritative state must originate from server simulation.
- Client prediction may smooth movement/rotation, but must reconcile to server snapshots.
- Keep predicted-only state isolated and reset safely when authoritative actor disappears/rejoins.
- Received input/action sequences are not applied acknowledgements. Preserve
  epoch checks, bounded queues, and one action per player per simulation tick.
- Bump the game protocol identity when changing serialized authoritative layout,
  and rebuild clients and servers together.

## Combat and Movement

- Movement lock during attack windup/recovery is intentional for readability.
- Facing determines directional attacks; keep facing updates explicit and deterministic.
- Enemy AI should stop at attack range and commit attack before moving again.

## Refactoring Guidance

- Keep `main.rs` thin; put logic in plugins/modules.
- Prefer small focused systems and helper functions over giant monolithic systems.
- Reuse existing components/resources before introducing new ones.

## What to Search First

When unsure, search these topics in Bevy 0.19 docs/examples:

- Transforms and directions:
  - `Transform`, `GlobalTransform`, `forward`, `right`, `looking_at`
- Scheduling and ordering:
  - `Update`, `FixedUpdate`, `SystemSet`, `.chain()`, `in_set`, `before`, `after`
- ECS data flow:
  - `Component`, `Resource`, `Query`, `Commands`, `ChildOf`
- Camera/input:
  - camera-relative movement, world-space movement
- UI diagnostics:
  - Bevy dev tools FPS overlay

## Validation Before Finishing

- For Rust changes, run formatting and the checks/tests covering the affected crates and behavior. Use `just check` when it covers the needed checks; do not duplicate equivalent commands.
- Broaden to workspace checks/tests for cross-crate contracts or merge/release validation. Documentation-only edits need relevant link/example validation rather than a game build.
- Fix failures caused by this change and rerun affected checks before finishing.


## Predicted mechanic presentation

- New transient mechanic visuals must be simulation-owned `PresentationInstance`
  state (or existing predicted actor components), not spawned from network receive
  handlers. See `docs/predicted-presentation.md`.
- Allocate identities from match epoch, actor, input sequence and a stable slot;
  do not allocate a fresh random ID on replay.
- The shared simulation owns acceptance, lifetime and gameplay. The client renderer
  owns meshes, materials and correction blending. Extend the common presentation
  lifecycle rather than adding mechanic-specific prediction/confirmation paths.
- Cover local execution, snapshot restore/replay, rejection, expiry and replication
  in tests when adding a new presentation primitive.
