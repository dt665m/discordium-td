# Dreamwake netcode follow-ups

> Historical record from before the engine consolidation. Source paths, commands,
> and validation results below describe that version; use the current root README
> and architecture/deployment guides for this workspace.

The five netcode/presentation fixes from the TD packet review were checked against
the working `codex/dreamwake-roguelite` branch on 2026-09-08. All 12 legacy source
files match the SHA-256 values in
`target/reports/netcode-followups/2026-09-08/tested-source-hashes.json`. Their
existing changes were preserved; the reference patch was not reapplied.

Dreamwake shares the existing Renet UDP/WebRTC transport, but runs its own server
authority at 60 Hz and publishes compressed full rollback snapshots at 20 Hz.
It retains the default eight-player party and configurable 1–1,024 admission
limit. Full snapshots are individually encoded for each recipient.

## Applicability and changes

| Reviewed fix | Existing TD behavior retained | Dreamwake adaptation |
| --- | --- | --- |
| Respawn-aware lag compensation | Historical heroes retain the authoritative respawn generation; rewind validation rejects crossing a life boundary even at identical positions. Epoch, timestamp, origin and gameplay validation remain. | Dreamwake has no historical hit-query or server rewind mechanism. Attacks execute in the current server simulation. No additional history or generation protocol was introduced for a mechanism it does not use. Restart epochs continue to invalidate old client commands and prediction. |
| Skip unused enemy interpolation | `SnapshotBuffer::sample` accepts an optional enemy lead; `None` skips the enemy pass while retaining remote-hero processing. | Dreamwake already has one presented `DreamView`. The renderer consumes its predicted enemy positions and blends remote corrections; it does not run a second unused snapshot enemy interpolation pass. Existing renderer actor lookups still have their own cost, so the legacy benchmark is not a Dreamwake frame-time result. |
| Targeted presentation reads | The HUD uses an indexed current-epoch hero read without rebuilding a full `WorldDelta`, and missing predicted heroes cannot use stale authoritative fallback. | HUD, scene and audio already consume the shared `DreamView`. Server menu checks now read `phase()` and `is_paused()` directly. Snapshot publication captures and sorts the ECS world once, then uses `DreamSnapshot::select_player` to select each recipient's hero, readiness and rewards. A missing recipient returns `false` and is skipped. Full rollback state remains identical across recipients. |
| ACK metadata without payload ownership | Bounded outstanding records contain sent tick, baseline tick and payload length without retaining the packet `Arc`; successful enqueue controls ACK eligibility. | Dreamwake has no delta-baseline ACK history or outstanding snapshot ownership table. Its input/action ACKs report commands applied by the authority. Its full-state transport send still checks capacity, and no extra packet-retention infrastructure was added. |
| Coalesce worker output after stalls | Bounded worker draining preserves reliable FIFO deliveries, retains the newest world and timing metadata, and leaves batches beyond the bound queued. | Dreamwake's authority and publication run synchronously without a worker-output queue. Catch-up is capped at six simulation steps and publishes the current world once when the snapshot deadline is due. The equivalent client receive boundary was corrected to check the 128-message bound before popping the next message, preserving queued reliable controls. Received states continue to select only the newest accepted revision for reconciliation. |

`DreamSnapshot::select_player` uses the sorted saved hero list. It only updates the
recipient view; it does not clone or modify shared enemies, projectiles,
presentations, RNG state, or latent casts. The publication loop reuses the same
snapshot allocation while setting each player's command ACKs. Serialization and
compression still occur per recipient, so this change removes repeated ECS
capture/sort work without claiming that network cost is independent of party size.

## Validation

- `cargo test -p game_server dreamwake::tests -- --nocapture`: **10 passed**,
  including exact decoded state/ACK equality for eight personalized recipients,
  real eight-client UDP play, admission limits, host departure, loss/rejoin
  recovery, authority validation and a complete two-player run through all three
  realms and boss phases to victory.
- The UDP suite required localhost socket permission. The initial sandboxed run
  failed at socket creation; the approved rerun passed all tests.
- The three simulation accessor regressions **passed** in the 75-test core suite.
  They cover eight recipient views with distinct
  rewards/readiness, unchanged saved state and restore equality, rejection of a
  removed recipient through restart, and direct phase/pause reads across restore
  and restart.
- The new accessor module and server file were formatted with Rustfmt.
- Legacy validation and helper measurements remain documented in
  [client-presentation-benchmark.md](client-presentation-benchmark.md). The original
  isolated 159-test result is evidence for those unchanged legacy files, not a
  claim about the final expanded Dreamwake workspace suite. No new Dreamwake
  performance benchmark was used to claim FPS or 1,024-player load capacity.

Implementation: `shared/simulation/src/dream/snapshot_access.rs`,
`game_server/src/dreamwake.rs`, and the bounded receive loop in
`game_client/src/dreamwake/network.rs`.
