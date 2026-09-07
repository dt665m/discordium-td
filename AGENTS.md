# AGENTS.md

This file defines project-specific guidance for coding agents working in this repository.

## Scope

- Applies to the entire workspace rooted at `discordium-td/`.
- Favor consistency and correctness over ad-hoc local fixes.

## Bevy 0.18 Core Conventions

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

## System Ordering and Dependencies

- Explicitly order dependent systems using `.chain()` or explicit schedules.
- Recommended high-level order per frame:
  - Input capture
  - Network receive/apply snapshot
  - Prediction/reconciliation
  - Gameplay visual sync
  - FX / UI updates
- Fixed-step gameplay commands should run in `FixedUpdate`.
- Do not rely on incidental ordering from registration order when dependency is real.

## Networking and Prediction

- Authoritative state must originate from server simulation.
- Client prediction may smooth movement/rotation, but must reconcile to server snapshots.
- Keep predicted-only state isolated and reset safely when authoritative actor disappears/rejoins.

## Combat and Movement

- Movement lock during attack windup/recovery is intentional for readability.
- Facing determines directional attacks; keep facing updates explicit and deterministic.
- Enemy AI should stop at attack range and commit attack before moving again.

## Refactoring Guidance

- Keep `main.rs` thin; put logic in plugins/modules.
- Prefer small focused systems and helper functions over giant monolithic systems.
- Reuse existing components/resources before introducing new ones.

## What to Search First

When unsure, search these topics in Bevy 0.18 docs/examples:

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
- Keep generated audit and verification reports in `target/reports/` (gitignored), not in tracked documentation. Do not commit these reports.


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
