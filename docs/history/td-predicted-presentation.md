# Predicted presentation contract

> Historical record from before the engine consolidation. Source paths, commands,
> and validation results below describe that version; use the current root README
> and architecture/deployment guides for this workspace.

K / Arc Burst now uses simulation-owned presentation instances. The client draws
this state after receive/reconciliation/replay, instead of spawning an explosion
from `ReliableGameEvent::AbilityCast`. That event remains for gameplay reporting.

## Adding a mechanic

1. Execute and validate the action in shared simulation. Update gameplay normally.
2. For a transient visual, create `PresentationInstance` only when accepted. Its ID
   is `(match_epoch, owner, action_seq, slot)`. Use deterministic slots for multiple
   instances per action. Ordinary continuous visuals read predicted actor state.
3. Select a presentation primitive and give its position, size and finite duration.
   Add a `PresentationKind` variant if a new primitive is necessary. The exhaustive
   renderer match forces a deliberate visual implementation.
4. Rendering goes in `simulation_presentations.rs`. The common lifecycle matches
   identities, advances animation from simulation age without restarting on replay,
   blends position corrections, fades missing state, and cleans up on disconnect or
   epoch changes. Do not add a network receive handler for the visual.
5. Test accepted and rejected actions, restore/replay identity, duplicate commands,
   expiry/reset, and snapshot patch propagation. New gameplay still needs tests for
   its own rules; infrastructure cannot infer a mechanic's intended appearance.

## Data path

`Simulation::presentations` → full snapshots / patches → snapshot reconstruction
→ `Simulation::apply_snapshot` → input replay → one client renderer.

Active instances are part of the complete simulation state reconstructed from
replication patches. The simulation advances ages and removes expired instances
on both client and server. Protocol 13 requires both ends rebuilt; see the
[replication contract](td-netcode.md).

The renderer retains missing instances invisibly for one second to tolerate brief
reconciliation changes, then releases them. Corrections within an instance's life
never rewind an already shown animation. A sufficiently late reintroduction after
retirement is not guaranteed invisible; this is not a permanent event log.

## Scope and boundaries

Arc Burst is migrated. Existing basic-attack, hit, tower and tentative-death paths
remain legacy presentation code; the new contract is the required path for new
transient mechanics and their eventual migration. It does not yet replace the
legacy HP watermark used by impact feedback.

The presentation lifecycle is separate from server hit validation. Arc Burst now
uses the bounded historical policy below; other mechanics retain their existing
hit rules.

## Server history foundation

`game_server::state_history::StateHistory` records 31 authoritative frames at
30 Hz. Each frame has a round epoch, tick, and separate hero/enemy identity and
position lists. `at_tick` requires an exact retained frame and rejects wrong-round
or unavailable history, including future ticks. `enemies_in_radius` is a read-only
historical center-overlap query matching today's Arc Burst geometry rule.

The history is server-only. K now sends the tick of its predicted target view in
`ClientAction::view_tick`. Before an ordered K action executes, the server accepts
only a non-future tick at most six ticks (200 ms) old, additionally limited to
measured half-RTT plus two ticks of sampling/scheduling allowance. No interpolation
delay is added because this client predicts enemies rather than rendering their
historical snapshots. Invalid, absent or unavailable ticks use current-world hits.

The caster origin and ability range always come from current authoritative
simulation after the movement barrier. History must show a continuous caster
trajectory within a movement allowance; caster discontinuities fall back to current
state. Targets absent in intervening frames or with a per-tick displacement above
two world units are excluded from the historical candidate set. This threshold is
a conservative discontinuity guard, not an enemy speed or full anti-cheat system.
Damage applies once to still-existing current enemies and never rewinds health,
AI, mana or cooldowns. The temporary query context expires after the simulation
step and is not replicated to client prediction.

`lag_compensation_query` recorder events record the sequence, requested view tick,
and historical candidate count or fallback reason. Tests cover forged future/old
or low-latency timestamps, wraparound, wrong rounds, discontinuities, removed
identities, actual historical damage, cooldown and duplicate-command enforcement.

This is a first pass: exact ticks only, no sub-tick interpolation, no historical
blockers/LOS, and no client-controlled caster positions. RTT is an estimate and
asymmetric routes or prediction drift can cause fallback. Supporting other abilities
requires an explicit timing/collision policy; projectiles are not instant queries.

Research: Blizzard's [GDC ability presentation](https://media.gdcvault.com/gdc2017/Presentations/Reed_Dan_NetworkingScriptedWeapons.pdf)
shows local-client rollback/replay; [Valve's networking documentation](https://developer.valvesoftware.com/wiki/Source_Multiplayer_Networking)
describes server historical hit validation. These are distinct operations.
