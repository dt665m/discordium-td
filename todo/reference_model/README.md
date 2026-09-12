# Executable Contract Models

Run `python -m unittest discover -s reference_model -v` from the package root. The suite has **89 tests**: 42 original movement/protocol tests and 47 added spatial/scope tests.

`model.py` covers end-of-tick semantics, immutable commands, bounded future admission, server substitutes, receipt/execute separation, transactional replay, complete restoration, deterministic dependencies, event deduplication, baselines, atomic groups, spawn binding and bounded historical times. Its movement is one-dimensional integer state and its delivery simulator runs in memory.

`spatial_model.py` exercises a small independent implementation of the selected Epic-inspired graph contracts: persistent influence-cell and global/owner/team lists, separate preparation/gathering, exact 3D distance filtering, bounded authorized dependency closure, scope-incarnation fences, full-entry ACK gating, destruction versus retirement and per-connection versioned dormancy completion. These Python types and algorithms are original library proposals, not copied Unreal source or Epic's wire protocol.

The spatial oracle test runs 25 seeds × 200 actors × 20 query/update rounds. A separate scope test checks 120 delivery permutations. The tests also verify that doubling an unrelated distant in-memory population does not increase local candidate visits. That operation-count test is not the production 100-connection / 50,000-actor benchmark.

The spatial model receives already-authorized connection views. It does not authenticate observers, compute occlusion, serialize actual authorized fields, implement a scheduler or full baseline-retirement protocol, retain real physics dependency history, or support concurrent graph workers. Its scope identity counters are deliberately bounded until connection reset. Full atomic prediction groups and transport-baseline details are separate contracts modeled elsewhere or left to the production implementation.

`TEST_RESULTS.txt` records actual local execution. The root README and `VALIDATION.json` distinguish these passing checks from the **157 unexecuted production acceptance requirements**. Use the models as executable examples and production regression seeds, not as deployable game servers.
