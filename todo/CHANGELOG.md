# Revision 2 Changes

## Spatial architecture

The selected foundation is now explicitly Epic's public Fortnite/Unreal Replication Graph pattern. Section 4 introduces the graph as a peer of simulation and prediction; Section 17 and the standalone spatial specification provide implementation contracts. Unity relevancy/importance is a supporting comparison, and Iris is a separate alternative backend.

The node model covers persistent spatial lists, static/dynamic/dormancy-driven routing, global and connection lists, custom semantic grants, per-connection gathering and optional distance-frequency scheduling. The exact portable implementation and protocol are labeled derived design, not undisclosed Fortnite behavior.

## Protocol and correctness

Added scope epochs and representation revisions separate from entity generations and baseline generations. New/re-entering scopes require full state and exact decoded-ready ACKs. Exit, dormancy and authoritative death are separate. Lost exits, wake updates, stale ACKs and re-entry reorderings have explicit repair/fencing behavior.

Prediction dependencies can bypass distance but not disclosure rules. Missing, prohibited or over-budget dependencies deny the affected prediction group instead of leaking state or admitting an incomplete group. Replay-history retention is separate from current render-replica lifetime. Observer grants, event audiences, field projections and revocation barriers are explicit security contracts.

Corrected the earlier informal simplifications: graph maintenance is not necessarily only on center-cell crossings; influence footprints and policy changes also matter. Shared dynamic refresh is permitted, and cost includes gathered candidates and dense interactions rather than promising universal linear-in-changes complexity. Dormancy is not out-of-interest, and always-relevant is not always-authorized.

## Implementation gates

Existing task IDs are kept without renumbering. `N060` (spatial node) and `N064` (disclosure) move to M1. Core scope/lifecycle, decoded-entry baselines, atomic chunks, scheduling, dormancy and transport budgets move into M2. Scoped owner groups and persistent-effect re-entry are required in M3. M6 now optimizes and qualifies the existing graph rather than introducing spatial replication.

The planning package contains **103 tasks, 156 production acceptance cases and 93 features**. Added 12 Epic source references while preserving S01–S50. New source IDs are S51–S62.

## Executable evidence and packaging

Added 47 spatial/scope contract tests to the original 42, for **89 passing local tests**. New checks include influence-cell queries, an independent randomized spatial oracle, permission-sensitive dependency closure, stale-scope fencing and per-connection dormancy completion. Their scope remains smaller than the production acceptance matrix.

Updated all Markdown/HTML reports, planning CSV/JSON exports, configuration, source register, model documentation, test output, validation and checksums. Added focused spatial/protocol HTML reports, a synthetic scale profile, canonical planning source and reproducible build/validation scripts. The builders embed the canonical subsystem/protocol text, preventing independent versions of the same section from drifting.

## Unchanged boundaries

No proprietary Fortnite/Overwatch source audit is claimed. No current Fortnite cell size, deployment setting or specific privacy/prediction policy is asserted. The handoff remains a development specification and executable contract model, not a completed production client/server library or proof of commercial-game parity.
