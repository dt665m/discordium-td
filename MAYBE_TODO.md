# MAYBE_TODO

Quality findings raised by a `/simplify` review of `dcd212b` ("fix: prediction")
and deliberately **not** applied in that pass. Each was verified against the tree
at the time of writing; none is a blocker. Line numbers are from `dcd212b` plus
the cleanup commit that followed it, so re-locate before acting.

Ordered roughly by value.

---

## 1. `apply()` runs a second projection on top of the engine's

**Where:** `games/dreamwake/client/src/plugins/network/presentation.rs`, `apply()`

`RemoteBuffer::sample` already projects the pose, then `apply()` projects it
*again* for `active_ahead - pose.projected_seconds`, re-running the ballistic
advance and the visual clocks. It does this because `RemoteBuffer::new` rejects
any `extrapolation_cap > 100ms` (`engine/net/src/interpolation/mod.rs`), and
projectiles want unbounded deterministic flight. The two passes are reconciled
by `projected_seconds`, a counter smuggled through the payload.

**Deeper fix:** make the cap a policy decision rather than one global `Config`
number. Add to `PosePolicy`:

```rust
fn cap(&self, value: &P, configured: Duration) -> Duration { configured }
```

and use `policy.cap(&latest.value, self.config.extrapolation_cap)` at the clamp
sites in `sample()`. `DreamPosePolicy::cap` returns `Duration::MAX` when the
sample carries deterministic ballistic motion, `configured` otherwise. That
deletes the top-up block in `apply()` entirely, along with the
`projected_seconds` field, its accumulation in `project_pose`, its reset in
`interpolate`, and its `valid()` entry.

**Why it matters:** today the engine reports `Frozen(ExtrapolationLimit)` for a
projectile that `apply()` is in fact still advancing, so `metrics().frozen` /
`metrics().extrapolated` and `combat_view_time()`'s `pose_time` describe a pose
nobody renders. Any future change to how the engine projects must be mirrored by
hand in `apply()`.

**Same area, smaller:** the six flat fields `deterministic_motion`,
`motion_active`, `projectile_direction`, `projectile_speed`,
`projectile_max_distance`, `remaining` are really one
`enum Motion { Velocity([f32; 2]), Ballistic { .. } }`. That turns
`project_pose`'s if/else into a match and removes the defaulted fields from
`receive()`'s per-variant arms.

**Skipped because:** cross-crate redesign of the interpolation contract, far
larger than the diff under review.

---

## 2. Cast reservation table duplicates `execute_memory`

**Where:** `games/dreamwake/simulation/src/systems/player.rs` (the
`projectile_reservation` / `wisp_reservation` / `delayed_reservation` block) vs
`games/dreamwake/simulation/src/systems/memories.rs` (`execute_memory`)

The capacity pre-check re-encodes "what does this Memory spawn" — per
`MemoryKind` and `EssenceKind` — next to the executor that already encodes the
same facts. Two sources of truth for one piece of authored ability data.

**Deeper fix:** `dcd212b` already established the right home for exactly this
kind of value: `MemoryKind::displacement(essence)` in `catalog.rs`, documented as
a "game-supplied value" that admission consumes "without knowing individual
ability implementations". Add a sibling:

```rust
fn reservations(self, essence: Option<EssenceKind>) -> Reservation { .. }
```

and have `player_actions` destructure it.

**Before doing this, resolve a discrepancy.** The two copies do not currently
agree, and it is not obvious which is intended:

- `execute_memory` spawns `2` wisps for `Twin` and `1` otherwise.
- `wisp_reservation` reserves `2` for `Twin | Echo` and `1` otherwise.

The `Echo` difference may well be deliberate — Echo also books a
`delayed_reservation`, so reserving two wisps could be accounting for the
delayed second one up front. That needs an author's answer, not a guess.

**Skipped because:** consolidating requires deciding which number is correct,
which is a behaviour decision rather than a cleanup. Getting it wrong either
falsely rejects a cast as `Budget` or overruns `MAX_OWNER_WISPS`, which is the
bound `validate_for` rejects on.

---

## 3. Owner-transient collection implemented twice, and already drifted

**Where:** `games/dreamwake/simulation/src/replication/owner.rs` (`capture_world`)
vs `games/dreamwake/simulation/src/replication/capture.rs`
(`CommittedReplication::owner_checkpoint`)

Both build `OwnerTransients` from the same source type
(`snapshot_capture::CapturedState`) with the same four per-owner filters for
`projectiles` / `wisps` / `effects` / `delayed`.

**They have already diverged:** authority filters effects with
`effect.age_ticks < effect.duration_ticks`; prediction filters only on
`id.owner`.

**Deeper fix:** one `OwnerTransients::collect(&CapturedState, owner)` used by
both, so the checkpoint definition is a single thing.

**Skipped because:** the divergence is a correctness question for
`/code-review`, not a cleanup, and the fix touches the authority publication
path.

---

## 4. `OwnerPredictionRunner::restore_world` re-implements `install_snapshot`

**Where:** `games/dreamwake/simulation/src/replication/owner.rs` vs
`games/dreamwake/simulation/src/simulation.rs`
(`DreamSimulation::install_snapshot`)

Despawn-all-`DreamOwned`, insert `Run` / `CollisionWorld` / `ActionTransactions`
/ `CombatBatch` / `CombatTraceState`, spawn hero plus the projectile / wisp /
effect / delayed component tuples, insert `MotionEnvironment::committed` — the
same sequence in two places.

**Deeper fix:** give `SavedProjectile` / `SavedWisp` a `spawn(&mut World)` the
way `SavedHero` / `SavedEnemy` already have (`state/mod.rs`), and factor the
despawn loop.

**Why it matters:** adding a marker component or a new latent entity kind
silently applies to authority replay but not owner replay.

**Skipped because:** large refactor of authority-shared code.

---

## 5. The owner's beam is produced by two paths and merged by id

**Where:** `games/dreamwake/simulation/src/replication/presentation.rs`

`result.presentations.extend_from_slice(owner.graphics())` already contains the
beam — the `beam_graphics` system spawns a `GraphicsInstance` in the replayed
world, `capture_current` collects it, `capture_world` keeps it. So
`owner.beam_graphics()`, recomputed independently from `hero.ray` via
`RayState::graphics`, needs a find-and-replace-or-push to avoid a duplicate.

**Deeper fix:** now that `RayState::graphics` is tick-aware and retires itself,
the replayed effect entity is the single source. Drop the `beam_graphics()`
append and let `owner.graphics()` carry it, keeping `beam_graphics()` only for
`beam_graphics_action()`, which needs just the `GraphicsId`. If the checkpoint's
`combat_identity.tick` genuinely gives a fresher beam than `run.tick`, do that
reconciliation once inside `OwnerPredictionState::graphics()`.

**Why it matters:** two derivations of one visual that must agree on id, aim,
elevation and `age_ticks`. The replace-or-push hides a disagreement instead of
preventing it, and it is the only entry in `presentations` that is not simply
appended, so any future ordering or dedup rule has to special-case it.

**Skipped because:** behaviour-sensitive.

---

## 6. `prediction_dependencies` decodes every visible scope, once per predicted tick

**Where:** `games/dreamwake/client/src/plugins/network/session.rs`

The function loops `self.entities` calling `decode_replica` on **every** public
scope (decompress + bincode alloc + `decode_compact`) just to find `Enemy`
variants, then sorts and keeps the nearest `MAX_PREDICTION_TARGETS`. It is
called from inside the tick loop in `generate_commands`
(`mod.rs`, `FixedUpdate`), so the whole decode pass repeats for every tick in
`current+1..=target` — normally 1, but up to the full catch-up gap allowed by
`preflight_prediction_commands`.

`presentation()` decodes the identical payloads a second time in the same
`FixedUpdate`.

**Cheaper:** hoist the scan out of the per-tick loop — build the sorted
candidate list once per call and per-tick apply only the `end_tick > at` filter
(which is the sole tick-dependent part). Better still, cache decoded actors
keyed by `(entity, state.version)` when scope state is applied, which removes
both passes.

**Skipped because:** the real fix is a decode cache, which needs mutable state
behind a `&self` method — a design change rather than a cleanup.

---

## 7. Runner allocations on every prediction step

**Where:** `games/dreamwake/simulation/src/replication/owner.rs`

Each runs once per predicted tick *and* once per replayed frame in `reconcile`:

- **`restore_world`** calls `world.query_filtered::<Entity, With<DreamOwned>>()`,
  which constructs a fresh `QueryState` (full archetype match pass plus bitset
  and `Vec` allocations) every call, then `.collect::<Vec<_>>()` allocates again.
  Since `restore_world` spawns every `DreamOwned` entity itself, keep a
  `spawned: Vec<Entity>` on the runner and despawn from that, clearing and
  reusing the allocation. Or store a `QueryState` on the runner and call
  `update_archetypes`.
- **`capture_world`** calls `capture_current()`, whose first act is a deep clone
  of `CollisionManifest { colliders: Vec<StaticCollider>, .. }`. `capture_world`
  only reads `heroes` / `projectiles` / `wisps` / `effects` / `delayed`; the
  manifest is dropped unread. `CollisionWorld::manifest_arc()` already exists —
  making `CapturedState.collision` an `Arc<CollisionManifest>` turns the clone
  into a refcount bump.
- **`step_owner`** (`client/.../model.rs`) rebuilds a `Vec<PublicEnemyView>` of
  up to 64 entries from the dependency map, which `step_with_targets` then
  re-validates and copies again into `PredictionTarget` components. A scratch
  `Vec` on the runner would avoid the per-step allocation.

**Skipped because:** each changes runner ownership or a shared struct's shape;
the `Arc` one touches a capture type the authority path also uses.

---

## 8. `owner_checkpoint` clones projectile heap state it immediately clears

**Where:** `games/dreamwake/simulation/src/replication/capture.rs`

`owner_checkpoint` `.cloned()`s whole `SavedProjectile`s (each
`ProjectileState` carries `hit_ids: Vec<u64>` and
`pending_impacts: Vec<ProjectileImpact>`), and `OwnerTransients::canonicalize`
then `.clear()`s both. Same shape in `capture_world` on the prediction side.

**Skipped because:** an empty `Vec` clone does not allocate, so the waste only
appears for projectiles that have actually recorded hits — and the fix requires
enumerating every `ProjectileState` field at the construction site, which is more
fragile than the allocation is costly. Revisit if `ProjectileState` grows.

---

## 9. `step_restricted*` wrappers are now test-only

**Where:** `games/dreamwake/simulation/src/replication/owner.rs`

`step_restricted` → `step_restricted_with_ray` → `step_restricted_combat` →
`step_restricted_combat_with_bases` → `OwnerPredictionRunner::new()` +
`runner.step(..)`. After `dcd212b` no production caller remains — the client
holds its own long-lived runner. Every remaining call site is a test, and each
one constructs an entire `DreamSimulation` Bevy `App` for a single step.

**Deeper fix:** keep the runner as the single entry point and give the tests one
helper — `replication/tests.rs` already does exactly this — then delete the four
wrappers.

**Skipped because:** roughly 30 test call sites across seven files; the churn
outweighs the gain. At minimum these are worth a doc note steering any future
production caller to the reusable runner, since the hidden
`OwnerPredictionRunner::new()` makes per-step world construction invisible at the
call site.

---

## 10. Byte budgets that must move together are unrelated literals

- `OwnerState::retained_bytes() = 64 * 1024` (`client/.../model.rs`) and
  `80 * 1024` twice in `client/.../session.rs` (staging, and `checkpoint_bytes`)
  are related only by a comment. One `const OWNER_CHECKPOINT_BYTES` next to
  `COLLISION_HISTORY_BYTES` would carry it.
- `protocol/src/live/baselines.rs`: `512 * 1024` appears as both
  `GROUP_RESIDENT_BYTES`'s per-member term and `limits().resident_bytes`. These
  previously agreed implicitly via `BaselineLimits::default()`.

**Skipped because:** low value, and the client/session pair may be intentionally
different numbers rather than one budget written twice — needs a quick check.

---

## Checked and explicitly discarded

Recorded so nobody re-raises them:

- **`PosePolicy::project_latest`** looks like a special-cased third entry point,
  but it is a genuine general extension: the `samples.len() < 2` branch has no
  earlier sample or interval to hand `extrapolate`, the default returns `None` so
  existing policies are unchanged, and calling `extrapolate` with a degenerate
  zero interval would make history-estimating policies divide by zero.
- **`SimulationScope::OwnerPrediction`** gating `D::Deaths` / `D::Finish`, and
  the `PredictionTarget` component, are deliberate capability boundaries (no
  `Health` / `Combat`, so replay cannot apply damage), not shortcuts.
- **`OwnerProjectileWire`'s field-by-field copy** looks like duplicated serde,
  but its doc comment shows the omission of `hit_ids` / `pending_impacts` is a
  deliberate disclosure boundary.
- **`BoundedBytes`' `FieldValue` / `CompactValue` impls** parallel
  `BoundedString`, but the differing text-budget and UTF-8 accounting make the
  parallel type the codebase's established shape.
- **`Mutex<OwnerPredictionRunner>` in `OwnerModel`** is forced by the `&self`
  trait signature.

---

## Unrelated: pre-existing test failures

`cargo test --workspace --no-fail-fast` fails 8 tests on `dcd212b` itself. They
are not caused by the cleanup pass and are listed here only so they are not
mistaken for it:

- `dreamwake_client`: `combat_cohort_ignores_friendly_poses_but_refuses_target_starvation`,
  `combat_time_keeps_fraction_and_rejects_same_tick_starvation_mismatch`
- `dreamwake_protocol`: `complete_enemy_replicas_use_exact_compact_fields`,
  `maximum_declared_schema_envelopes_roundtrip_without_compression`
- `dreamwake_sim`: `shared_capsule_sweeps_dash_at_perimeter_and_restores_replay_exactly`,
  `eight_overlapping_and_crossing_players_do_not_change_owner_collision_or_replay`,
  `restricted_owner_replays_shared_motion_dash_attack_without_world_state`,
  `shared_owner_prediction_covers_every_memory_and_essence_lifecycle`
