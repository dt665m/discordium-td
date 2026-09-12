# Persistent network resource soak

`scripts/qualify-soak.py` captures the workspace and every local path-dependency
repository into an independent source tree, records the file hashes, makes that
tree read-only, and builds a release executable from it. Each output directory
must be new or empty. Run final qualification only after implementation and
short integration checks are finished, against a frozen release candidate.

Soaks, endurance tests and extended matrices are manual handoffs. Before one is
launched, prepare the exact command, frozen source/build identity, expected
duration, pass criteria and output paths; stop and let the operator run it and
return the artifacts. A short development smoke does not satisfy a release gate.

For an operator-run one-minute harness smoke:

```sh
python3 scripts/qualify-soak.py \
  --output out/network-qualification/soak-smoke \
  --seconds 60 --report-seconds 15 --warmup-seconds 10
```

The final operator-run day-long test uses elapsed monotonic wall time:

```sh
python3 scripts/qualify-soak.py \
  --output out/network-qualification/soak-24h
```

Keep that process running on an awake machine for the full duration. A shortened,
interrupted, failed, or suspended run does not substitute simulated ticks for
elapsed operation. The runner records both elapsed time and completed ticks;
qualification requires at least 27 ticks/second against the 30 Hz target.

The generic workload keeps one production interest graph with 50,000 registered
actors and 100 connections alive throughout the run. The same per-connection
scope stores, scheduler, group assembler, prediction manager, baseline caches,
and event journal persist. Public scopes exit and reenter as observers move;
100 stable public actor indices change generation. Dormant actors receive only
explicit mutations, while owner/dependency groups continue publishing. Packet
loss and up to two ticks of reordering use capped synthetic queues. Prediction
history periodically fills its 256-tick window before reconciliation resumes;
baseline and event history are reclaimed through their production retirement
APIs. The harness never periodically recreates its population to clear memory.

At most 1,024 interval reports are allowed per run.
`metrics.jsonl` contains bounded interval and final component counters, retained
bytes, queue peaks, graph/index counts, scope churn, decode/reorder/loss counts,
and tick timing. `rss.jsonl` contains interval RSS aggregates and a constant-memory
linear trend after warmup. `report.json` binds those measurements to machine,
compiler, source-manifest and executable hashes. Source and executable hashes are
checked again at completion. Logs and reports are generated artifacts, not
repository documentation.

The default RSS ceilings are 1 GiB resident, 32 MiB growth after the five-minute
warmup, and a fitted trend no greater than 1 MiB/hour. They are explicit harness
budgets, not assigned reference-hardware requirements; command-line options may
set different budgets before a run. The report records those choices. A sampled RSS value above
the resident ceiling terminates the process. Sampling does not measure every transient allocator peak. Missing RSS measurements prevent a
24-hour pass. Short-run RSS trends are reported but do not qualify a day-long
resource gate.

`wall_clock_24h_passed` requires a successful uninterrupted requested run, at least
86,400 actual seconds, immutable inputs, required workload paths exercised,
acceptable tick rate, and the configured RSS budgets. This is a generic component
resource soak under synthetic delivery. It does not establish a Dreamwake game,
UDP/WebRTC transport, browser, gameplay-bot, or reference-hardware soak. The optional
`--udp-preflight` runs the existing real UDP activation test once before the soak;
that test remains a preflight and does not turn this workload into a bot soak.
