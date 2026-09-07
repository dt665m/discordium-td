# Networking and prediction audit: input timeline

This document records successive implementation stages. Current build and runtime
results are in [registry migration verification](registry-migration-verification.md).

This follow-up audits the path from user input through transport, authoritative
simulation, snapshots, client replay and visual feedback. It supersedes the protocol-8
wire format described in the earlier audit. Protocol 9 requires rebuilding both ends.

## Code map

| Responsibility | Location |
| --- | --- |
| Keyboard projection and action capture | `game_client/src/plugins/game/input.rs` |
| Independent 30 Hz send cadence, movement/action redundancy | `game_client/src/plugins/game/outbound.rs` |
| Receive processing, snapshot acceptance, ACK pruning, round transitions | `game_client/src/plugins/game/receive.rs` |
| Fixed-step prediction, rollback/replay, attack effect identity | `game_client/src/plugins/game/prediction.rs` |
| Snapshot history, delta reconstruction and interpolation | `game_client/src/plugins/game/snapshots.rs` |
| Transport creation, session cleanup, ECS registration, rendering | `game_client/src/plugins/game.rs` |
| Wire types, sequence comparisons, snapshot state | `game_shared/src/lib.rs` |
| Server message limits and snapshot baselines | `game_server/src/app.rs` |
| Authoritative action barriers and movement queues | `game_sim/src/input.rs` |
| Gameplay simulation, snapshot restore and round reset | `game_sim/src/lib.rs` |

## Findings and changes

1. **Attacks overtook preceding movement.** Independent ACK watermarks protected
   delivery but did not preserve gameplay ordering. Actions now carry their preceding
   movement boundary. The server waits for that movement to be consumed, without
   executing extra movement steps in a tick.
2. **Cross-channel delivery could bypass the action.** Every movement bundle repeats
   unacknowledged actions, while reliable delivery remains available. The server deduplicates
   and orders queued actions by sequence. Both channels use the same ingestion path.
3. **Deferred input was absent from rollback state.** Snapshots now carry pending actions
   and their boundaries. Restored simulations do not execute a replayed duplicate immediately.
4. **The movement queue could discard part of a 16-input bundle.** Its old ten-entry cap
   was smaller than one valid bundle. It now retains up to 64 moves, still consuming one per tick.
5. **Local attack feedback waited for authority.** Basic-attack effects now run from local
   prediction. A separate attack sequence ID deduplicates confirmation and replay; confirmation
   can supply feedback if it wins the race at low latency. Hits and damage remain authoritative.
6. **Old-round packets could survive reset.** Actions and bundles carry a match epoch.
   Server ingestion rejects old epochs; client epoch changes clear prediction, pending input,
   timing samples and position smoothing. Reliable packets already queued are harmless when late.

## Other paths inspected

- Fixed input generation and send timing are distinct; receive still runs each frame.
  Missed send slots coalesce, with no send catch-up burst.
- Movement and action acknowledgment watermarks remain separate and wrap-aware.
- Reconnect constructs a new runtime and clears prediction, snapshot history, smoothing,
  diagnostic history and recorder session state.
- Snapshot acceptance rejects duplicates/stale ticks before applying events or reconciliation.
  Delta reconstruction uses retained server baselines; invalid ACKs cannot advance server baselines.
- Decode sizes, message counts, action/input queues, recorder queues and browser send buffering
  remain bounded. Nonfinite movement and invalid action dependencies are rejected.
- Server ordering remains commands, one movement per player, then gameplay advancement.
  Client replay uses the same Simulation implementation.

## Regression coverage

- Walking 1, 5, 10 and 16 ticks before attack: each partial acknowledgment preserves the
  predicted stop position, including a restored server movement/action backlog.
- An early action waits for all preceding movement and executes once despite duplicates.
- Attack effects deduplicate both predicted-first and authority-first feedback.
- Action dependencies support sequence wrap and reject future/self dependencies.
- Pending actions are bounded and cannot survive player removal or round reset.
- Native transport test: 150 ms each direction, 25 ms jitter and 5% loss, with reliable
  attack delivery deliberately omitted; redundant bundles recover the attack at the expected position.
- Existing transport outage, sequence wrap, duplicate input, baseline, replay equivalence,
  match reset and recorder tests remain part of the workspace suite.

## Limits

This is an audit with regression coverage, not a claim that all possible netcode defects
have been eliminated. Browser suspension still pauses engine-driven networking. Remote
players' unknown future input and authoritative collisions can still require corrections.
The input ACK timing metric includes local queueing and server processing; it is not raw RTT.
Ability/hit events still come from authority; only basic-attack presentation was predicted here.
Extreme loss beyond retained input history can require correction. Cross-architecture bit-exact
simulation, long Internet soaks and production-sized realm load have not been established.

## Follow-up: predicted presentation

The charge and special-skill HUD now read the reconciled local hero, matching the
actor animation. Unit-bar fills update every visual frame instead of only when
`WorldView` changes; that snapshot-only run condition was hiding predicted damage
and regeneration. Enemy impact particles and reactions now also follow forward
local simulation steps, including lethal-hit particles, with authoritative
snapshot fallback. Replay itself does not spawn effects.

`game_client/src/plugins/game/presentation.rs` contains the HUD selection and
impact deduplication helpers. Impact deduplication uses an enemy's lowest observed
HP across reconciliation, relying on the current rule that enemies do not heal.
This only controls cosmetic feedback: authoritative correction may restore HP.
A rejected predicted hit can therefore suppress a later impact at the same HP
threshold. Explicit damage-event identities are needed before adding healing or
requiring exact per-attack confirmation; this is not a confirmed-hit marker.
Ability burst particles still follow the reliable server event.

Validation: workspace checks and tests (including charge progress without a new
snapshot and impact replay/confirmation deduplication), WASM check, recorder
analysis tests, and rebuilt browser client. Server delay remains 150 ms each way.

### Charge stages and predicted deaths

The power circle exclusively displays predicted power-up charge. Startup, mana
refill, and recovery do not substitute their progress or labels into the pie.
A first-charge regression checks that stage transitions do not cause a false
fast fill and reset.
A predicted enemy removal no longer despawns its existing render entity while the
latest authoritative world still contains it. Its position is held and its health
bar reads zero until reconciliation restores it or authority confirms removal.
This preserves immediate predicted damage without repeated visual resurrection;
final disappearance waits for authority. Predicted HP can still be corrected.

### Impact reaction follow-up

Baseline seeding no longer advances the cosmetic damage watermark. A lower HP
found during reconciliation must remain eligible for the next forward-step
reaction instead of being silently consumed as a new baseline. The regression
now covers seeding a lower replayed HP before observing the impact. Predicted
lethal hits also attach `EnemyHitReaction` to the retained visual. Enemy squash
starts at contact strength and decays over 160 ms rather than ramping up halfway
through the effect. Attack windup remains part of simulation timing.

### Tentative lethal reaction

The retained enemy visual now collapses into a visible tentative-hit pose over
250 ms, with a small settling motion while confirmation is pending. It never
hides speculatively. When reconciliation restores the enemy, the component is
removed and ordinary visual smoothing restores its pose. Confirmed removal still
despawns it. This avoids a frozen upright enemy without promising an irreversible
predicted death. An ECS regression verifies that the visual animates and remains
visible beyond the reaction duration.

### Simulation-owned presentation and server history (protocol 10)

Arc Burst visuals now read stable simulation instances rather than reliable
server events. The shared lifecycle covers snapshot replication, restore/replay,
expiry, and renderer correction. See [the presentation contract](predicted-presentation.md)
for the extension rules, migration boundaries, and server historical-query foundation.
The history retains approximately one second of positions; historical K hit
validation is not enabled yet.

### Bounded Arc Burst historical validation (protocol 11)

K now supplies its predicted target-view tick. The server validates it against a
six-tick / 200 ms maximum and measured half-RTT plus two scheduling ticks, then
uses continuous historical target positions with the authoritative caster origin.
Invalid context falls back to current-world targeting; mana, cooldown, command
ordering and deduplication remain unchanged. Removed targets cannot receive damage.
Recorder `lag_compensation_query` events expose the decision.

Validation included the full workspace tests, native/WASM checks, recorder tests,
and rebuilt browser/server at 150 ms delay each way. The live cast's view tick
matched the execution tick (282), demonstrating why an additional blind RTT
subtraction would be incorrect for predicted targets. Separate tests verify actual
historical damage, forged timestamps, wraparound, missing/teleported actors, and
cooldown/duplicate protection. This is exact-tick AoE validation, not whole-world
rollback or a complete anti-cheat system.
