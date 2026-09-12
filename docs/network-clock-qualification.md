# Clock qualification

The clock fixture exercises the production `TickClock`, `TickRate`,
`ClockEstimator`, and `LeadController` APIs. It is a generic clock test, with
synthetic timestamp injection and empty fixed-step callbacks. It does not run
Dreamwake, network sockets, hardware clocks on two machines, or the integrated
24-hour game/transport gate in engineering specification §22.4.

## Modes and acceptance

`simulated` advances an injected reference in 10 ms increments without sleeping.
Even 86,400 simulated seconds is only a clock regression. `realtime` uses
`Instant::elapsed()` from one process epoch and sleeps toward absolute 10 ms poll
instants. It never resets that epoch or silently drops scheduling debt. The two
modes have separate result fields; simulated results cannot pass actual-duration
gates.

Both modes run **−1,000 and +1,000 ppm simultaneously**, with an integer affine
local-clock mapping and a five-second initial local offset. These are proposed
fixture parameters, not universal network accuracy promises. Four timestamps use
10 ms uplink, 2 ms server processing, and 10 ms downlink. Probes are generated every
250 ms and delivered after their declared receive time. Each drift instance holds
at most one pending probe and the production estimator's 32-sample window. The
lead controller uses a fixed initial lead of two ticks; this profile does not
exercise asymmetric arrival-slack adaptation, packet loss, or jitter.

Every observed estimate must be monotonic. After ten reference seconds of warmup,
absolute estimated-server phase must be at most one 60 Hz tick. Issued command
targets must increase and remain within two ticks of the true server tick plus
lead. Each predicted clock dispatches fixed callbacks on its estimated time plus
lead. Server and predicted callback tick IDs must be contiguous. The
`declared_fixed_dt_bits` field identifies the fixture's configured `1/60` constant;
callbacks receive tick metadata and do not integrate gameplay. The evidence is
clock phase, contiguous dispatch, and exact rational deadlines, not measured
gameplay delta or a game's integration behavior.

For **every dispatched tick**, an independent integer check requires
`0 <= deadline_ns * 60 - tick * 1_000_000_000 < 60`. This verifies upward rounding
against the exact rational period without using `TickRate` as its own oracle.
At the exact 86,400-second reference cutoff, the server must have dispatched
5,184,000 ticks. The exact cutoff is used only after checking the actual final
polling gap; an OS pause across the end of the run cannot be hidden by clamping.
Absolute rational deadlines remain anchored to tick zero. Lateness is measured
separately from deadline arithmetic. Any polling gap over 100 ms, remaining debt
following the production four-tick dispatch budget, clock error, phase violation,
or interruption makes the run fail or incomplete; no automatic reset forgives it.

The fixture streams interval and final JSON lines, never per-tick histories. It
retains counters, constant-size phase histograms, and online RMS/linear-trend
statistics, including maximum phase at every poll and after warmup. Histogram
upper bounds in nanoseconds are 100,000, 250,000, 500,000, 1,000,000, 2,000,000,
5,000,000, 16,666,667, then overflow. Reports also include probes, retained sample
high-water marks, predicted steps, maximum deadline lateness, and debt. At most
1,440 interval periods are allowed, with at most one additional final row.

The wrapper additionally compares a sleep-inclusive independent reference against
its monotonic runtime clock. It uses `CLOCK_BOOTTIME` when available, falling back
to `time.time` elsewhere. Both per-sample and cumulative elapsed divergence over
one second invalidate the run; backward reference clocks also invalidate it.
Wall-clock corrections cannot be distinguished from suspension in the fallback,
so those corrections conservatively make the run incomplete. The wrapper also
rejects gaps longer than its RSS sampling period plus three seconds (two seconds
for the bounded `ps` call and one second tolerance), and checks continuity again
when the child exits. This catches a whole-system sleep even where `Instant` or
`time.monotonic` exclude suspended time. The recorded clock basis and tolerance
are part of the evidence; this is not a claim to detect every subsecond pause.

The wrapper checks source and binary integrity, streams bounded resident-memory
measurements, and enforces a default 256 MiB RSS ceiling and 16 MiB first-to-last
RSS growth budget. Actual-duration acceptance requires at least two valid RSS
samples. These are fixture memory budgets, not game server budgets. Native RSS
sampling currently requires `ps` with `-o rss=` support.

- `T004_actual_hour_passed`: both drift signs meet this profile for at least
  3,600 actual seconds, with complete immutable evidence and memory checks.
- `T007_actual_day_passed`: the rational dispatch run meets the same checks for
  86,400 actual seconds and the exact full tick count.
- `simulated_regression_passed`: successful injected-time regression only.
- `integrated_game_transport_24h_passed`: always false in this fixture.

Neither an arithmetic check at a 24-hour timestamp nor the existing generic
resource soak proves T004/T007. The resource soak runs its own 30 Hz loop and does
not measure the production estimator's phase. Preserve separate evidence.

## Frozen manual workflow

Use a capture containing this runner and its dependencies. The wrapper requires
`--source-manifest`; it does not capture a live tree or discover an enclosing Git
repository. Run the **captured copy** of the script. `--source-root` defaults to
the manifest's sibling `sources` directory. The existing strict manifest helper
rejects changed, missing, extra, duplicate, traversal and symlink source entries.
Build output is always `<output>/build`, outside the frozen source tree. Every
attempt needs a new output directory, including preparation-only attempts.

Set `CLOCK_CAPTURE` to the actual capture directory, then inspect commands and
validate the manifest without building or running:

```sh
python3 "$CLOCK_CAPTURE/sources/discordium-td/scripts/qualify-clock.py" \
  --source-manifest "$CLOCK_CAPTURE/source-manifest.json" \
  --mode realtime --seconds 86400 --report-seconds 60 \
  --output "$CLOCK_CAPTURE/manual/clock-day-prepared" --prepare-only
```

Actual-duration gates are **manual long runs**. Reserve Cargo for the build and
stop unrelated profiling or game workloads before launching:

```sh
python3 "$CLOCK_CAPTURE/sources/discordium-td/scripts/qualify-clock.py" \
  --source-manifest "$CLOCK_CAPTURE/source-manifest.json" \
  --mode realtime --seconds 3600 --report-seconds 60 \
  --output "$CLOCK_CAPTURE/manual/clock-hour"

python3 "$CLOCK_CAPTURE/sources/discordium-td/scripts/qualify-clock.py" \
  --source-manifest "$CLOCK_CAPTURE/source-manifest.json" \
  --mode realtime --seconds 86400 --report-seconds 60 \
  --output "$CLOCK_CAPTURE/manual/clock-day"
```

A successful full-day run also supplies the one-hour duration predicate for both
signs; running both commands is only necessary when separate artifacts are wanted.
Use `--mode simulated` with a new output path for an explicitly requested
accelerated regression; it cannot substitute for either command above.

The default build timeout is 900 seconds. The runtime watchdog is requested real
duration plus 60 seconds, or 300 seconds for simulated mode. Explicit
`--build-timeout-seconds` and `--runtime-timeout-seconds` override these within
bounded limits. Timeout or Ctrl-C terminates the process group and writes an
incomplete report. An incomplete build/run does not create a passing gate result.
Do not resume partial output as a new run or reduce duration to relabel it complete.

Preserve `report.json`, `build.log`, `run.log`, `metrics.jsonl`, `rss.jsonl`, the
referenced immutable source manifest/tree, and the executable identified by its
SHA-256. `report.json` records toolchain, platform, flags, exact commands, profile,
source-manifest and binary hashes, integrity checks, and separate gate results.
Runtime or artifact errors return a nonzero exit status. A failed phase/debt bound
is evidence to investigate, not permission to widen it after seeing the result.

## Small harness checks

These checks use small synthetic samples, a finite 1,004-poll regression across
the ten-second warmup boundary, and constructed report/timing rows. They do not
launch the qualification CLI or a real-time workload:

```sh
cargo test -p engine_net --example clock_qualification
python3 -m unittest discover -s scripts -p test_qualify_clock.py
```
