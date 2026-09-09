# Client presentation follow-up — 2026-09-08

> Historical record from before the engine consolidation. Source paths, commands,
> and validation results below describe that version; use the current root README
> and architecture/deployment guides for this workspace.

Two client changes remove work that did not contribute to normal rendering:

- `SnapshotBuffer::sample` skips enemy interpolation while local prediction owns
  enemy positions. Remote heroes still interpolate; uninitialized clients retain
  the enemy interpolation fallback and spawn/epoch boundaries.
- `Simulation::hero_snapshot` retrieves one current-epoch hero through the
  existing identity index. Both HUD callers use it instead of extracting and
  sorting a complete world. A missing predicted hero stays missing.

## Focused benchmark

```bash
cargo test --release -p game_client --bin game_client \
  benchmark_client_presentation_hot_paths -- \
  --ignored --nocapture --test-threads=1
```

The opt-in benchmark uses 8 players / 256 enemies and 32 players / 1,024 enemies.
Each path has 16 warm-up calls, then nine alternating before/after repetitions
of 256 calls. Inputs and outputs use `black_box`; output destruction is included.
Equivalent hero results are checked outside timing. Results are medians of each
repetition's average duration per call, on an Apple M3 Max with 64 GiB RAM.

The old HUD path builds the world and selects one hero; the new path fetches that
hero directly. The old interpolation path samples heroes and enemies; the new
path samples only heroes. The preserved enemy fallback supplies the old path for
comparison. This measures helper costs, not rendered frame time, FPS, simulation,
network delay, or GPU work. Both versions run in one optimized binary.

| Helper / workload | Previous | Current |
| --- | ---: | ---: |
| One HUD hero, 8 players / 256 enemies | 2.493 µs | 0.075 µs |
| Interpolation, 8 players / 256 enemies | 23.437 µs | 0.357 µs |
| One HUD hero, 32 players / 1,024 enemies | 9.479 µs | 0.076 µs |
| Interpolation, 32 players / 1,024 enemies | 214.374 µs | 1.320 µs |

The indexed hero read no longer scales with enemy count. Skipping unused enemy
sampling removes the nested enemy-ID searches and the resulting map allocation.
These results establish lower helper cost; they do not measure a whole-frame
or server-CPU improvement.

## Correctness and validation

This follow-up also:

- Records server-owned hero respawn generations in lag-compensation history and
  rejects rewinds across life boundaries even at identical positions.
- Stores only tick/baseline/size metadata for outstanding snapshot ACKs, allowing
  shared-history eviction to release packet payloads.
- Coalesces a bounded worker backlog into its newest world while preserving all
  reliable messages in FIFO order. Skipped publication ticks remain recoverable
  from retained replication baselines.

Regression coverage includes same-position respawns, tick/generation wrap,
identity restore/removal/rejoin, interpolation fallback boundaries, payload
release with late ACKs, bounded worker draining, reliable ordering, and exact
reconstruction after skipped/lost publications.

The validation copy passed `just check`, formatting, all 159 workspace tests,
the wasm client check, and six netcode analysis tests. The benchmark is ignored
in the normal suite and runs separately. The existing conditioner test initially
exhausted its gameplay window while Renet was still connecting under packet loss.
It now allows a separate bounded handshake period before the full gameplay
sample; all three real-UDP conditioner cases passed afterward.

Validation used the committed baseline plus this task's changes in an isolated
source copy because concurrent Dreamwake edits did not compile during the initial
workspace check. The implementation files in that copy were verified byte-for-byte
against the workspace. Raw benchmark output, passing test output, source hashes,
and the task patch are saved in `target/reports/netcode-followups/2026-09-08/`.
