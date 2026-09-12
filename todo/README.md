# Server-Authoritative Prediction Library Handoff

**Revision 2: Fortnite / Unreal Replication Graph spatial foundation.** The public Epic graph model is now the required basis for spatial replication across the architecture, protocol, planning, configuration and qualification tests. Spatial selectivity is implemented in M1–M2 and required for the M3 prediction vertical slice; it is no longer introduced as a later scaling feature.

Open [ENGINEERING_SPEC.html](ENGINEERING_SPEC.html) for the complete navigable report. For the focused spatial design, open [SPATIAL_REPLICATION.html](SPATIAL_REPLICATION.html). Editable Markdown sources are included for both.

## What changed

The library now has a first-class `net_interest` subsystem with persistent spatial, global, owner/connection and custom semantic nodes. It prepares shared graph data once, gathers per-connection candidates, closes permitted prediction dependencies, applies disclosure rules, reconciles scope lifecycle, then prioritizes and encodes within bandwidth limits.

The specification distinguishes the observed Epic mechanisms from derived library contracts. The exact portable grid algorithm, scope epochs, authorization policy, dependency admission and configuration values are proposed implementations, not claims about proprietary Fortnite internals. Unity is supporting comparison. Native Unreal Replication Graph and Iris are separate backend choices, not layers to combine.

## Package contents

| File | Purpose |
|---|---|
| `ENGINEERING_SPEC.html` / `.md` | Complete research and engineering specification; 27 engineering sections and 62 source references |
| `SPATIAL_REPLICATION.html` / `.md` | Focused normative graph design, evidence mapping, algorithm, lifecycle, security, budgets and qualification gates |
| `PROTOCOL_ADDENDUM.html` / `.md` | Connection sequence, scoped entry/exit, ACKs, baseline fencing, dormancy delivery and bounded recovery |
| `CHANGELOG.md` | Changes from the original handoff, migrated tasks and attribution boundaries |
| `sources.json` | Machine-readable register; original 50 references preserved and 12 Epic references added |
| `planning/implementation_backlog.csv` / `.json` | 103 proposed tasks, milestone gates, owners, acceptance criteria and test references |
| `planning/acceptance_tests.csv` / `.json` | 156 required production acceptance cases; explicitly not claimed executed |
| `planning/feature_inventory.csv` / `.json` | 93 requirements with relevant public mechanisms and provenance notes |
| `planning/plan_source.json` | Canonical planning source for regeneration; original IDs preserved |
| `config/arena_profile.toml` | Proposed 60 Hz arena and required graph/scope/dormancy defaults |
| `config/spatial_scale_profile.toml` | Explicit benchmark overlay for 100 connections / 50,000 potential actors; sparse/dense/dormant/wake/teleport scenarios |
| `reference_model/model.py` / `test_model.py` | Original movement, replay, command and baseline contract model/tests |
| `reference_model/spatial_model.py` / `test_spatial_model.py` | New in-memory graph, dependency, scope-incarnation and connection-dormancy model/tests |
| `reference_model/TEST_RESULTS.txt` | Actual output: 89 tests passed in the local Python runtime |
| `VALIDATION.json` / `BROWSER_CHECKS.json` | Actual execution scope, counts, consistency checks and browser review status |
| `build_planning.py` / `build_report.py` | Regenerate exports and all HTML, including identical embedded spatial/protocol sections |
| `validate_package.py` | Run tests and validate package counts, references, configuration and generated-file consistency |
| `requirements-docs.txt` | Tested HTML builder dependency; no third-party dependency is needed for contract tests |
| `SHA256SUMS.txt` | Checksums for packaged files, excluding this checksum file itself |

## Run and rebuild

The contract tests require Python 3.10 or newer and only the standard library:

```sh
python -m unittest discover -s reference_model -v
```

For package validation use Python 3.11 or newer, which includes `tomllib`. Rebuilding HTML additionally requires `mistune`:

```sh
python -m pip install -r requirements-docs.txt
python build_planning.py
python build_report.py
python validate_package.py
```

Edit `SPATIAL_REPLICATION.md` for detailed spatial contracts and `PROTOCOL_ADDENDUM.md` for protocol completion contracts. `build_report.py` embeds them into Sections 17 and 27 of the main specification. Edit other sections in `ENGINEERING_SPEC.md`. Edit `planning/plan_source.json`, not the generated planning exports. The scale profile is a documented overlay; the production team must implement explicit profile merging/admission rather than assuming a TOML parser merges files automatically.

## What has and has not been executed

All **89 contract tests passed** locally: the original 42 plus 47 new spatial/scope tests. One original test simulates delivery across 100 seeds with 80 scheduled server ticks per seed. A new spatial test checks 25 seeds, 200 actors per seed and 20 query/update rounds per seed against an independent full-scan oracle. Another checks all 120 permutations of five scoped lifecycle/state messages. These are executable contract checks, not game-performance measurements.

The movement model remains one-dimensional integer simulation. The spatial model uses in-memory floating-point positions for interest queries only; it does not implement 3D physics, real transport, field serialization, authentication, visibility geometry, production chunk scheduling or worker concurrency. It receives already-authorized connection views rather than authenticating observer requests. Its scope model validates identity and state-version transitions, not a complete compressed wire protocol. It refuses unbounded scope-identity growth instead of implementing negotiated production retirement. It is not a replacement for the production library.

The 156 production acceptance cases remain requirements, and all implementation tasks remain **Not started**. No live 100-client benchmark, cross-platform qualification, 24-hour soak or commercial-game comparison was executed. The passing contract model must not be reported as those release gates having passed.

## Implementation sequence

M0 freezes the Epic-basis ADR, route/disclosure/observer policies, contracts and dependency pins. M1 builds offline simulation **and the persistent graph**. M2 establishes real networking and scoped entry/exit/dormancy recovery. M3 proves selective delivery with two rendered clients and complete authorized prediction dependencies. M4–M5 complete gameplay and combat. M6 tunes the already-working graph and runs scale qualification. M7 hardens the implementation; M8 expands its supported envelope.

Public source access does not authorize copying. The existing license review requirements remain unchanged. Both executable models were independently written for this handoff; they are not copied Epic, Valve, Unity or other engine implementations. See the source register and provenance notes before reusing external code.
