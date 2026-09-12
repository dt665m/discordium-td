# Snapshot delivery qualification: application fragments versus Renet

## Purpose and decision

Partial reliability is a legitimate, generalizable transport feature. The application must declare which messages are interchangeable or expired; Renet should own stopping retransmissions and safely retiring the receiver's state. It is not safe to implement arbitrary cancellation by deleting an entry from stock Renet's sender queue.

However, these measurements do **not yet justify implementing a cancellation fork for this game**. There are meaningful A/C freshness differences, but simple admission/queue policy changes account for substantial differences too. The narrower, immediately actionable obsolete-retry category is generally a few percent of modeled downstream bytes, not the dramatic “most old bytes are wasted” interpretation. An implemented cancellation policy was not benchmarked, so its net benefit remains unmeasured.

The preferred simplification target is **C: bounded whole owner-group messages using default upstream Renet reliable-unordered delivery, without a new cancellation feature**. A has not demonstrated a consistent advantage sufficient to justify maintaining its custom delivery layer when simplicity is the priority. Successful production-shaped integration is the cutover condition, not a reason to maintain permanent A/C implementations. A's adverse reset triggers and C's control-buffer failure remain acceptance questions. This guide records evidence and migration reasoning; it does not claim that the production cutover has happened.

## What was actually tested

Source base: `8c5fb6e998ee29a1defb85395be380464a4b779a`.
New harness: [renet_adverse_delivery.rs](../engine/net/tests/renet_adverse_delivery.rs).
SHA-256: `5b28d77413bdc4ab968329016507bc73ad1dca24d0c85ce4a297ead2b6eb4dd9`.

The isolated build uses crates.io **Renet 2.0.0**, checksum `8535f2f4b5afe5e31fc37be4b8a1f167e61177f356b1c6ee8456b7045d6c74c0`. Production path patches were disabled only in an isolated checkout for the stock experiment. Local path dependencies are not immutable source identities; the ordinary workspace configuration still uses the fork described in [the dependency register](network-dependencies.md).

700 deterministic cases: seven profiles × four workloads × five policies × five seeds (7, 29, 101, 307, 997). Each generates snapshots for ten simulated seconds and allows three seconds for final recovery. The matrix itself runs in about four wall-clock seconds.

Policies:

- **A:** real engine ByteAssembler/fragment_group over unreliable Renet messages. Preserve an immutable first transmission pass, coalesce waiting state, permit topology replacement, and repair the final state after the chosen interval.
- **C4/C16:** whole messages over reliable-unordered Renet, with four/sixteen exact decoded-receipt admission slots and a 128 KiB sender queue. Receipts for obsolete decoded messages are included.
- **C128** (log name CQ): same reliable transport, admission limited only by its 128 KiB sender queue; no decoded-receipt gate.
- **C32** (CQ32): same queue-only policy with a 32 KiB sender queue. All variants retain a 128 KiB state receiver allowance.

Workloads are independent, already-encoded complete states: 512 bytes, 4,096 bytes, 4,096 bytes with eight 29,216-byte bursts, and continuous 29,216-byte overload. No compression, delta/baseline dependencies, interest management, bootstrap/admission, or game reset exchange is modeled. Each completed payload must exactly match one immutable generated fixture.

Both paths publish and flush every 50 ms, share 7,000 bytes of Renet payload allowance per flush, and pass through a modeled 160,000 B/s final egress bucket with a 16,000-byte burst and 2,400-byte control reserve. Each packet is charged actual Renet bytes plus 73 bytes of modeled IPv6/UDP/netcode overhead. A additionally uses a conservative 6,000-byte prefix. Its due-control observer reads actual emitted IDs and delivered ACKs; an assertion checks every admitted A fragment was packetized.

Competing traffic includes 380-byte reliable-ordered control messages, 512-byte remote updates and 48-byte inputs every 50 ms. A bounded 100 ms bottleneck queue performs serialization before random loss. Directions use independent random streams. Equal seeds do not imply identical packet-loss positions when policies produce different packet counts.

Profiles: clean 80 ms base RTT; 150 ms RTT/3% loss with jitter/reorder/duplicates; 500 ms RTT/10% loss with heavier jitter; 800 ms bidirectional outage; 80,000 B/s downlink; 20% reverse loss plus 200 ms reverse delay; and clean-but-slow 80,000 B/s downlink. RTOs are selected per profile (250–1,100 ms), not copied from the game's current 80 ms control setting. Harsh testing follows the type of conditions recommended by [Epic's UE 5.8 network-emulation guidance](https://dev.epicgames.com/documentation/en-us/unreal-engine/using-network-emulation-in-unreal-engine); these policies and tuning values are not attributed to Unreal.

## Important limits

This is a real-Renet-packet simulation, **not an equal-MTU production A/C integration test**. Stock C has 1,200-byte slice payloads plus framing/overhead; production's bounded profile caps the entire IP packet at 1,200 bytes. Larger packets are allowed in this experiment, with no MTU-dependent loss. Some packet efficiency therefore includes this deliberate difference. The game's independent worker polling, variable publication budget and age-sensitive scheduling are also not reproduced.

Freshness is sampled every 10 ms through the ten-second run, including blackouts. It is age of the newest applied state's generation timestamp, not latency conditioned on successful delivery. Numbers below are medians of the five per-run p95s; appendix brackets show the seed range, not statistical confidence intervals. Traffic totals cover generation plus settling, not a steady-state bytes/second measurement.

A retains its real two-second assembly deadline. Once that expires, the production game would need its reset/resync exchange, which is absent here. Post-expiry staleness is consequently **not a measurement of production recovery time**. Such cases are marked reset-required instead of ranked by their inflated p95.

## Results that change the decision

For steady 4 KiB snapshots:

| Condition | A p95 age | C128 p95 age | C32 p95 age |
|---|---:|---:|---:|
| Clean, 80 ms RTT | 120 ms | 120 ms | 120 ms |
| 150 ms RTT, 3% loss | 290 ms | 210 ms | 220 ms |
| Impaired reverse ACK path | 330 ms | 220 ms | 310 ms |
| 800 ms outage | 500 ms | 940 ms | 700 ms |
| 80 KB/s downlink, 3% loss | Reset required, 5/5 | 2,390 ms | 780 ms |
| 500 ms RTT, 10% loss | Reset required, 5/5 | One disconnect; surviving seeds 530–550 ms | 1,210 ms |

**Small updates:** At 512 bytes, A and queue-only C are essentially tied for freshness: p95 160 ms at 3% loss, and 380–400 ms under the harsh profile. The extra reliable machinery buys little for this workload.

**Moderate fragmented updates:** At 4 KiB/3% loss, C128 improves p95 by 80 ms (about 28%) and applies a median 176 newer states versus A's 107, out of 200 generated. Mean modeled downstream traffic is 1.149 MB versus 1.169 MB—only about 1.7% lower. This is primarily a freshness result, not a large bandwidth win. In clean conditions C uses about 4.2% less downstream traffic with equal freshness.

**Admission is material:** The same 4 KiB impaired-reverse profile gives C4 a 1,510 ms median p95, but C128 220 ms. The large gap is caused by the extra application receipt gate, not reliable-unordered delivery inherently waiting for those game receipts. Removing that gate does not remove game decode/baseline validation requirements.

**Queue capacity is material:** Under the congested 80 KB/s profile, C128 to C32 cuts p95 from 2,390 to 780 ms and mean emitted downstream bytes from 1.566 to 0.843 MB, with no cancellation protocol. But C32 is worse on high RTT/loss because it cannot keep enough useful state in flight. A single fixed queue size is not a universal solution. Congestion-responsive admission/rate control remains relevant for both designs.

**Bursts and overload:** Mixed clean traffic favors C32 (150 ms p95) over A (330 ms) and C128 (460 ms), partly because the smaller admission queue coalesces some large intermediate states before transport admission. That is appropriate only where those states truly are interchangeable. Continuous 29,216-byte publication demands 584,320 payload B/s, already 3.65 times the configured 160,000 B/s egress before overhead. Even clean A and C32 have 620 ms p95; C128 has 1,170 ms. Cancellation cannot make all this offered load fit.

**No candidate met every bad-condition objective.** In harsh steady-state cases, 82–87% of reliable ordered control messages across the tested policies missed the illustrative 500 ms deadline. A snapshot improvement is not an overall game pass.

## Failures were retained, not hidden

A required an in-run reset in 60 of its 140 cases: all five seeds for twelve workload/profile combinations. Those are predominantly harsh or overloaded/bottlenecked multi-fragment transfers. Four additional mixed-workload expirations happened only during settling, after final delivery, when a duplicate partial final repair timed out; they are not counted as active-run failures.

The final matrix contains one explicit transport disconnect: C128, steady 4 KiB, harsh profile, seed 307, at 6,750 ms. The reliable-ordered **control** receive channel exhausted its configured 32 KiB allowance. This is not evidence that the reliable state channel itself failed, nor a clean C victory.

An earlier symmetric C32 experiment (32 KiB both sender and receiver) also disconnected under only 3% loss at 5,120 ms, seed 997. Stock receive allocation rounds 4,096 bytes up to four 1,200-byte slice slots (4,800 bytes), so equal payload-based sender and receiver caps are not equivalent. Separating the sender cap from the receiver reserve fixed this tested configuration; all 140 final C32 cases remained connected. This does not prove long-duration receiver memory bounds.

The final state did not recover within the settling allowance in A:60 cases, C4:1, C128:2, and C32:4. C16 had none, but often much worse live freshness. Eventual reliable delivery and good live freshness are different objectives.

## Is “discard if not worth resending” a network-layer feature?

Yes, with a split contract:

| Application owns | Transport owns |
|---|---|
| Whether two states replace one another; epoch/topology/baseline validity | Fragmentation, packet ACKs and retransmission scheduling |
| The useful lifetime and relative priority | Enforcing that lifetime/priority in its queue |
| Declaring explicit cancellation/invalidation | Retiring missing IDs, freeing partial receive state and handling late traffic |
| Successful game decode/acceptance receipts | Transport delivery acknowledgement |

A generic opt-in partial-reliability API could accept a replacement key, lifetime and priority, return a message handle, and allow cancellation. Those are declarations, not a game-specific utility algorithm. Define whether replacement affects only waiting work or an active transfer, when lifetime starts, cancellation-versus-delivery races, and limits on keys/retirement metadata.

This has established precedent: [PR-SCTP, RFC 3758](https://www.rfc-editor.org/rfc/rfc3758.html) separates per-message persistence policy from the protocol mechanism that retires abandoned data. [WebRTC](https://www.w3.org/TR/webrtc/#rtcdatachannel) exposes bounded lifetime or retransmission count. These are precedents, not drop-in changes to Renet; expiring a lower-layer WebRTC datagram would not cancel Renet's own retained reliable message.

### Why sender-only deletion is incorrect

Two focused diagnostics exercised stock Renet's actual reliable packet/receive/ACK paths, then simulated unilateral abandonment in an isolated source copy:

1. Abandon message ID 0, deliver and acknowledge 1,024 later messages: the receiver's oldest-pending boundary remains 0 and retains 1,024 dedup IDs despite zero queued payload bytes.
2. Deliver one slice of a 29,216-byte message, then abandon it and deliver 600 later messages: the receiver retains a 30,000-byte partial allocation. A subsequent large message fails a deliberately tight receive limit.

The retained [diagnostic patch](../engine/net/tests/fixtures/renet-unilateral-abandon.patch) adds those tests to clean upstream Renet 2.0.0. Passing means reproducing unwanted retention, not successful cancellation. The patch is test-only evidence, not a production dependency patch.

General cancellation needs bilateral retirement: communicate skipped IDs/ranges reliably, release partial assemblies, bound deduplication/retirement state, ignore late fragments safely, tolerate late ACKs, and never reuse an abandoned ID. An opt-in per-channel “latest state” service can infer some retirement from newer message IDs, but still requires changed receiver semantics. Expiry without a successor needs an explicit protocol rule. Normal reliable channels must retain their existing guarantee.

### Why newer-generated is not automatically obsolete

At 20 Hz a message becomes older than the newest generation after 50 ms. Transmitting 29,216 bytes takes at least 183 ms at 160,000 B/s before overhead, contention and loss. Abandoning the current transfer every 50 ms can prevent any complete state from ever arriving.

Keep a viable active transfer plus the latest waiting state; allow enough opportunity for useful completion, including repair. A newer decoded, applicable state is much stronger evidence of obsolescence than a newer state existing only on the sender.

## How much avoidable retry cost was measured?

The harness separately measures retry payload older than (a) newest generated, (b) newest applied at the receiver, and (c) newest exact decoded receipt already returned to the sender. The last is the conservative, directly actionable category for these independent full states.

For steady 4 KiB C128:

- 3% loss: that category is payload equivalent to **2.0% of total modeled downstream bytes**.
- Impaired reverse ACKs: **0.5%**.
- Congested 80 KB/s link: **4.7%**.
- C32 under congestion reaches **5.9%**, despite its smaller overall traffic total.

These are not measured net wire savings from a cancellation implementation. Cancellation metadata costs bytes, policy changes affect future loss/queue behavior, and earlier application-declared invalidation could create additional opportunities. Conversely, “50% retransmitted under congestion” does not mean 50% safely cancellable: much of that traffic still advances the receiver.

On this evidence, a new wire protocol is not justified by bandwidth savings alone. Its stronger potential value is avoiding stale queue residence while recovering useful missing fragments—but that benefit must be measured against bounded/coalesced stock C, not against an over-buffered or receipt-blocked straw man.

## Production corroboration and remaining gate

Current production A, using the copied local fork and authenticated native UDP, passed:

- [collision decode-proof admission test](../games/dreamwake/server/src/authority/tests.rs): initial complete group was 2,867 application bytes, illustrating that 29,216 is a capacity stress size, not a universal normal update.
- [moving-enemy cadence test](../games/dreamwake/server/src/authority/tests.rs): about eight measured seconds after setup, 1,017,773 native wire bytes (about 127 KB/s), maximum owner age three 60 Hz ticks and moving-remote gap six ticks; pacing/cap drops zero.

These are clean-link A checks. Adverse full-game resync, equal production MTU, C game integration, browser WebRTC, delta/baseline replay and endurance acceptance remain unverified.

### Clean-cutover scope

Use one complete bounded owner-group message on reliable-unordered transport. Keep remote actor updates on their existing unreliable path; changing the whole current state channel would also change those semantics. Coalesce the latest unpublished owner state before transport admission, enqueue each accepted group once, and let Renet own its slices and retries.

Remove application byte fragmentation, fragment framing, client ByteAssembler and fragment staging, server fragment vectors/cursors/first-pass tracking, and manual prefix repair/repeat scheduling. Reduce FrozenGroup to the small bootstrap/reset record still required for exact decoded identity, activation and deadlines. Do not remove group manifests, scope/topology/epoch identities, transactional baseline reconstruction, atomic publication, input/action receipts, or correctness-related resync.

Default Renet migration must also replace the current publication preflight's unsliced-message assumptions in [engine/server/src/net.rs](../engine/server/src/net.rs). Resolve whole-message admission separately from per-flush packet pacing. Validate the actual default packet sizes rather than carrying forward a claim that stock 1,200-byte payload slices satisfy a 1,200-byte IP-packet cap.

The final target has one delivery implementation, not a permanent compatibility toggle. Bump the protocol identity, rebuild clients and servers together, qualify UDP/WebRTC and bootstrap/baseline/reset/rejoin behavior, then delete unused A paths and update the contracts. Preserve the reproducible historical comparison fixture as test evidence, clearly distinguished from production code.

Removing the Renet fork and removing application A are related but not identical: workspace patches also supply transport/security integration and a separately demonstrated renetcode confirmation-retry correction. Do not silently remove those guarantees or describe a partial de-fork as “full default Renet.” Verify stock behavior, move only genuinely necessary application/session policy to its proper layer, and report any remaining upstream correctness blocker explicitly. Do not introduce cancellation to make the migration possible.

## Reproduction and validation

### Ordinary workspace run

The workspace's existing sibling renet-cross checkout is required by its path patches. This run exercises the same fixture against the selected dependency, not necessarily stock Renet:

```sh
cargo test --locked -p engine_net --test renet_adverse_delivery -- --nocapture
cargo test --locked -p engine_net --lib
```

### Stock-Renet comparison

Use a disposable clean checkout at a commit containing this guide and the tests. Do not edit the working game's dependency configuration merely to reproduce the experiment. In that checkout's root Cargo.toml, remove **only** the three `[patch.crates-io]` entries named `renet`, `renetcode`, and `renet-cross`; preserve the pinned sctp-proto and str0m entries. Allow Cargo.lock to resolve the registry replacements in the disposable checkout.

```sh
cargo pkgid renet
cargo test -p engine_net --test renet_adverse_delivery -- --nocapture
cargo test --locked -p engine_net --lib
```

Verify that `cargo pkgid renet` reports crates.io Renet 2.0.0, not a local path. A different version is a new experiment. Record the checkout identity, edited manifest, resolved lockfile and test output. After initial resolution, add `--locked` to reruns; `--offline` is optional when all dependencies are already cached.

The matrix prints 700 `ADVERSE` JSON rows. To retain a result file in Bash or Zsh:

```sh
set -o pipefail
cargo test --locked -p engine_net --test renet_adverse_delivery -- --nocapture 2>&1 | tee renet-adverse-results.log
```

Treat the log as an output artifact rather than a production acceptance transcript. Compare per-profile/per-workload seeds; do not pool unrelated workloads into one winner. Use `reset_required_ms < 10000` for in-run reset counts, separate settling-only expirations, and report every non-null `disconnected_ms` and `final_lag_ms:null`. Weighted obsolete-retry payload / total downstream bytes is not a counterfactual cancellation saving.

The clean sanity test requires final atomic delivery, no A expiry, delivered controls, no queue/pacer losses, and no reliable retries. Characterization passing means integrity, instrumentation and stated bounds passed while emitting all adverse outcomes; it does **not** mean every case met service objectives. The retained `renet_delivery_qualification.rs` is an earlier legacy smoke fixture: its unequal pacing, cyclic A sender and short reliable RTO make its A/B/C totals unsuitable for architectural decisions.

### Cancellation diagnostics

From the repository root, obtain a separate clean copy of the official Renet 2.0.0 crate source. Never patch a shared Cargo registry cache or the working renet-cross dependency:

```sh
qualification_repo="$PWD"
renet_diagnostic_dir=$(mktemp -d)
curl --fail --location https://static.crates.io/crates/renet/renet-2.0.0.crate --output "$renet_diagnostic_dir/renet-2.0.0.crate"
tar -xzf "$renet_diagnostic_dir/renet-2.0.0.crate" -C "$renet_diagnostic_dir"
cd "$renet_diagnostic_dir/renet-2.0.0"
git apply --check "$qualification_repo/engine/net/tests/fixtures/renet-unilateral-abandon.patch"
git apply "$qualification_repo/engine/net/tests/fixtures/renet-unilateral-abandon.patch"
cargo test --locked --lib unilateral_abandon_diagnostic -- --nocapture
cargo test --locked --lib
```

The official crate includes its lockfile. The evidence above used two passing diagnostics and 23 passing library tests with that lockfile, meaning the retained allocation/gap conditions were reproduced. An earlier symmetric 32 KiB state-receiver configuration is not a final policy variant; its failure is documented above, while the committed C32 fixture deliberately keeps 128 KiB receiver reserve.

### Separate renetcode confirmation regression

The test-only [renetcode confirmation patch](../engine/net/tests/fixtures/renetcode-confirmation-regression.patch) reproduces a distinct upstream 2.0.0 failure: after losing the initial KeepAlive, continuous application payloads postpone confirmation KeepAlive retries. From the repository root, apply it to a disposable official renetcode 2.0.0 crate, not to the working dependency:

```sh
qualification_repo="$PWD"
renetcode_diagnostic_dir=$(mktemp -d)
curl --fail --location https://static.crates.io/crates/renetcode/renetcode-2.0.0.crate --output "$renetcode_diagnostic_dir/renetcode-2.0.0.crate"
tar -xzf "$renetcode_diagnostic_dir/renetcode-2.0.0.crate" -C "$renetcode_diagnostic_dir"
cd "$renetcode_diagnostic_dir/renetcode-2.0.0"
git apply --check "$qualification_repo/engine/net/tests/fixtures/renetcode-confirmation-regression.patch"
git apply "$qualification_repo/engine/net/tests/fixtures/renetcode-confirmation-regression.patch"
cargo test --locked --lib lost_initial_keepalive_recovers_during_continuous_payloads -- --nocapture
```

This regression test is expected to **fail on unmodified stock 2.0.0**. It passes with the existing correction that application payload sends update `last_packet_send_time` only for confirmed clients. It is not an A-versus-C performance result and not a reason to retain A. Full-default dependency cutover must verify whether the selected upstream version has fixed it, provide a tested upstream-compatible recovery mechanism, or report it as an unresolved blocker. The fixture patch contains only the test, not a production fix.

The reference validation also passed 218 engine_net unit tests, formatting, diff checks and `just architecture`. Native production corroboration uses the two named tests in [server authority tests](../games/dreamwake/server/src/authority/tests.rs):

```sh
cargo test --locked -p dreamwake_server collision_decode_proof_is_required_before_owner_activation -- --nocapture
cargo test --locked -p dreamwake_server paced_owner_and_moving_enemies_keep_actual_remote_cadence -- --nocapture
```

Run those against the normal production dependency composition, not the disposable stock-only engine_net build. They are short clean-link tests. Extended impairment, browser and endurance acceptance follow [network profile qualification](network-profile-qualification.md) and [game soak](network-game-soak.md); they remain unverified here and require the documented user-run handoff.

## Full freshness summary

Milliseconds: median per-run p95 [minimum–maximum seed p95]. Reset/disconnect labels replace misleading ordinary latency comparisons. Settling-only A expirations are excluded from reset labels. C4/C16 retain exact application-receipt admission gates; C128/C32 do not.

### compact512

| Profile | A | C4 | C16 | C128 | C32 |
|---|---:|---:|---:|---:|---:|
| clean80 | 100 [100–100] | 100 [100–100] | 100 [100–100] | 100 [100–100] | 100 [100–100] |
| bad150_loss3 | 160 [160–160] | 410 [210–580] | 160 [160–180] | 160 [160–160] | 160 [160–160] |
| harsh500_loss10 | 390 [380–400] | 1390 [1010–3290] | 1820 [1100–2010] | 380 [380–400] | 380 [380–400] |
| bad150_outage800 | 440 [410–460] | 710 [650–1010] | 570 [560–700] | 460 [420–460] | 460 [420–460] |
| bad150_link80000 | 170 [170–170] | 540 [210–890] | 180 [170–250] | 170 [170–170] | 170 [170–170] |
| bad150_ack_loss20_delay200 | 160 [160–160] | 820 [750–1070] | 960 [750–2060] | 160 [160–160] | 160 [160–160] |
| clean80_link80000 | 110 [110–110] | 110 [110–110] | 110 [110–110] | 110 [110–110] | 110 [110–110] |

### steady4096

| Profile | A | C4 | C16 | C128 | C32 |
|---|---:|---:|---:|---:|---:|
| clean80 | 120 [120–120] | 120 [120–120] | 120 [120–120] | 120 [120–120] | 120 [120–120] |
| bad150_loss3 | 290 [290–370] | 460 [370–540] | 220 [210–230] | 210 [210–220] | 220 [210–220] |
| harsh500_loss10 | Reset 5/5 | 1680 [1470–1820] | 1440 [1370–3280] | Disconnect 1/5 | 1210 [860–1290] |
| bad150_outage800 | 500 [460–630] | 780 [760–1080] | 930 [790–970] | 940 [920–970] | 700 [520–790] |
| bad150_link80000 | Reset 5/5 | 650 [480–1120] | 1160 [980–1330] | 2390 [2250–2560] | 780 [770–780] |
| bad150_ack_loss20_delay200 | 330 [310–410] | 1510 [920–2040] | 980 [590–1280] | 220 [210–230] | 310 [270–330] |
| clean80_link80000 | Reset 5/5 | 560 [560–560] | 1180 [1180–1180] | 2350 [2350–2350] | 720 [720–720] |

### mixed4096_29216

| Profile | A | C4 | C16 | C128 | C32 |
|---|---:|---:|---:|---:|---:|
| clean80 | 330 [330–330] | 400 [400–400] | 460 [460–460] | 460 [460–460] | 150 [150–150] |
| bad150_loss3 | 430 [420–460] | 530 [390–610] | 540 [530–550] | 540 [520–560] | 370 [320–470] |
| harsh500_loss10 | Reset 5/5 | 1790 [1200–1950] | 1400 [1190–2100] | 670 [620–790] | 1680 [1360–1970] |
| bad150_outage800 | 580 [550–660] | 920 [730–980] | 1040 [890–1100] | 1040 [930–1110] | 780 [760–830] |
| bad150_link80000 | Reset 5/5 | 880 [800–1600] | 1610 [1350–1870] | 2440 [2280–2600] | 870 [800–2000] |
| bad150_ack_loss20_delay200 | 490 [380–610] | 980 [870–1520] | 770 [650–1570] | 550 [520–560] | 780 [340–930] |
| clean80_link80000 | Reset 5/5 | 850 [850–850] | 1990 [1990–1990] | 2340 [2340–2340] | 750 [750–750] |

### overload29216

| Profile | A | C4 | C16 | C128 | C32 |
|---|---:|---:|---:|---:|---:|
| clean80 | 620 [620–620] | 1170 [1170–1170] | 1170 [1170–1170] | 1170 [1170–1170] | 620 [620–620] |
| bad150_loss3 | Reset 5/5 | 1390 [1320–1430] | 1350 [1330–1400] | 1350 [1330–1400] | 1330 [1280–1420] |
| harsh500_loss10 | Reset 5/5 | 3180 [2920–4120] | 3010 [2390–3290] | 3010 [2390–3290] | 3780 [3270–4710] |
| bad150_outage800 | Reset 5/5 | 2000 [1840–2110] | 2030 [1860–2130] | 2030 [1860–2130] | 1760 [1340–2000] |
| bad150_link80000 | Reset 5/5 | 2940 [2890–3040] | 2940 [2890–3040] | 2940 [2890–3040] | 2620 [2040–3040] |
| bad150_ack_loss20_delay200 | Reset 5/5 | 2150 [1500–2810] | 1430 [1280–1470] | 1430 [1280–1470] | 1900 [1590–2390] |
| clean80_link80000 | Reset 5/5 | 2680 [2680–2680] | 2680 [2680–2680] | 2680 [2680–2680] | 1560 [1560–1560] |
