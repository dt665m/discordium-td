# Netcode audit — 2026-09-06

Historical audit stage. For the final protocol and dependency versions, see
[registry migration verification](registry-migration-verification.md).

Scope: current Discordium TD worktree, preserving the existing debugging/UI work.
No autoreview, publication, merge, or renet fork. The fixes below are game protocol,
simulation, or game integration changes, not changes to the vendored transport.

## Original findings

| Severity | Finding on entry | Current result | Evidence |
| --- | --- | --- | --- |
| High | Older snapshots could be rejected by the interpolation buffer but still drive ACK pruning, hit effects, reconciliation and WorldView. Duplicates were accepted. | Fixed. A successful buffer insertion now gates all those consumers. The reliable join seeds the same watermark; unreliable snapshots before initialization are ignored. | `protocol_regressions::snapshots_reject_stale_and_duplicate_ticks_across_wrap`; inspect `network_update` and `SnapshotBuffer::push` in game_client/src/plugins/game.rs. |
| High | Movement 101 could cause reliable action 100 to be discarded. A movement ACK also discarded that action from client replay. | Fixed. Movement and reliable actions have independent sequence watermarks, restored in snapshots. Replay prunes individual commands using their channel's ACK. | The delayed-action reproduction failed before the fix (Ready instead of Windup). `delayed_reliable_action_survives_newer_movement`, `movement_ack_does_not_discard_or_reapply_reliable_action`, duplicate/wrap tests, and the real transport test below now pass. |
| High | Restore reset enemy target, waypoint and repath state; reconstructed speed/reward rather than preserving them. | Fixed for the tested restore/replay contract. Snapshots now carry these fields, hero movement backlog, direction and channel watermarks. Simulation maps have deterministic key order. | Target-state reproduction failed before the fix (Base instead of Hero). A crowded two-hero/eight-enemy restore diverged on the very next tick with unordered maps. `restored_world_replays_identically_with_movement_backlog` now compares complete deltas and events for 30 replay ticks. |

## Additional fixes

- **High — movement fast-forward:** issuing an action flushed up to ten queued movement
  displacements in one server tick. Removed that flush; movement consumption remains
  at most one queued move per tick. The displacement-bound regression failed before
  the fix and now passes.
- **Medium — baseline retention:** server patches could reference a 60-tick-old
  baseline, but the client retained only 12 snapshots. The bounded client history
  now retains 64 snapshots. Missing baselines still cause a patch to be skipped;
  the server falls back to full snapshots after baseline expiry. Tests exercise the
  usable window, expiry and tick wrap.
- **Medium — ACK poisoning:** an unknown/future client ACK previously advanced the
  server watermark even when it did not match sent history. Only known, newer sent
  snapshots now advance it.
- **Medium — reconnect contamination:** bootstrap replacement clears old prediction,
  pending actions, world state, timing history, interpolation and smoothing.
  Removing a server player also removes queued commands for that player. Terminal
  disconnects stop transport polling instead of logging the same failure every frame;
  successful reconnects clear prior-session diagnostic counters.
- **Medium — bounded input work:** server command ingestion runs at fixed simulation
  cadence with per-client message budgets; reliable movement is rejected; movement
  bundles are size/count bounded. Simulation pending commands are bounded and
  deduplicated, and non-finite directions are rejected. Binary decoding rejects
  trailing bytes and limits decoded payload size.
- **Medium — entity namespaces:** hero removal IDs are separate from enemy/tower
  tombstones. Render-index keys also distinguish heroes from world entities, avoiding
  collisions when a client ID equals a simulation entity ID.
- **Medium — clock/reset edges:** transport timers use real elapsed time, independent
  of game time scaling; the client no longer clips transport elapsed time to 100 ms.
  Prediction increments and spawn/reset deadlines handle u32 wrap. Match reset clears
  queued movement. The exact half-range sequence boundary is corrected.

## Validation

- Workspace checks and tests; client wasm target check/build; server UI library tests.
- Four recorder-analysis tests.
- Built-in browser smoke test: connected WebRTC client consumed delta snapshots with
  zero decode errors and baseline misses. A local server restart exercised the
  timeout/reconnect menu path; it also exposed the repeated-error issue above.
- Deterministic tests cover stale/duplicate/wrapped snapshots, redundant and reordered
  movements, delayed and duplicate actions, independent ACK pruning, invalid float
  input, player removal/re-add, restore/replay, baseline expiry and invalid ACKs.
- Native process/UDP integration: 150 ms added each direction, ±25 ms jitter, 5% loss,
  and a one-second client outage. First confirms movement 101, then sends reliable
  ability 100 twice during the outage. Checks that the action is ACKed, produces
  exactly one authoritative cast event, and the connection remains live.
  This complements the existing server-delay RTT test; RTT alone is not its oracle.

## Compatibility and limits

Wire protocol is **8**, previously 7. Rebuild client and server together. The additional
snapshot fields are necessary for replay and increase bandwidth; production-scale
bandwidth and CPU profiling remain to be done. No new dependencies were introduced.

Restore/replay equality is tested with identical known inputs on one native platform.
It does not prove cross-architecture bit equality or predict future remote inputs.
The native impairment test is short and uses transport conditioning; it is not an
Internet soak test. Browser validation is a smoke test, not equivalent coverage of
SCTP scheduling, background-tab throttling, or ICE/DTLS setup failures.

Remaining areas for targeted follow-up: multi-hour clock precision/interpolation
pacing, long outages spanning automatic match resets, delayed reliable visual events
(which have no server-tick stamp), maximum-population replay cost, and real production
authentication/rate limiting. The current bootstrap policy is explicitly development
authentication. This audit does not certify production readiness or upstream-library
correctness. No reproducible renet-cross defect was identified.
