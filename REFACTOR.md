# WebRTC transport and netcode refactor checklist

Working checklist (untracked). Items marked *verify first* depend on code not yet
read.

## Context

- Repos: `discordium-td` (game), `../renet-cross` (transport), `../sctp-proto`
  (fork `dt665m/sctp-proto`, upstream `algesten/sctp-proto`), str0m fork
  `dt665m/str0m` (to be dropped).
- sctp-proto: branch `fix/partial-reliability-abandonment` rebased onto upstream
  `main` (0.10.5), force-pushed; it is upstream PR #57 (commits `180e01e`,
  `68eb43b`). Both `[patch.crates-io]` entries point at that branch; lockfiles
  resolve to `68eb43b`.
- The `realtime-cwnd-floor` feature is dropped: it violates RFC 9260 §7.2.1
  (initial cwnd), §7.2.3 (ssthresh) and §6.3.3 (T3-rtx cwnd = 1 MTU). Until
  phases 2–5 land, WebRTC throughput after random loss falls back to the RFC
  floor (~4·MTU per RTT ≈ 30 KB/s at 150 ms).
- Sizes: renet `SLICE_SIZE = 1200` (hard-coded const), renetcode
  `NETCODE_MAX_PACKET_BYTES = 1400`, str0m `DATAGRAM_MTU_TARGET = 1150`, str0m
  SCTP payload = `mtu − 48 (DTLS) − 40 (SCTP)` = 1062 at the default MTU. A
  full renet packet therefore splits into 2 SCTP fragments today; losing either
  loses the packet.
- str0m `dtls_buf` is 2000 bytes and `set_mtu` is capped at 1500, so
  `BufferTooSmall` is unreachable with browser peers (str0m PR #1061 answered
  by the maintainer: use `RtcConfig::set_mtu`).

## Phase 0 — finish the sctp-proto switch

- [ ] Commit the sctp-proto switch in renet-cross and discordium-td. In
      renet-cross, drop the unpushed `b0984b4` (enabled the floor) or commit on
      top of it. Changes are uncommitted in: both `Cargo.toml`/`Cargo.lock`,
      renet-cross `CHANGELOG.md`/`README.md`, `docs/network-dependencies.md`.
- [ ] Delete local sctp-proto branches `feat/realtime-cwnd-floor` and
      `backup/pr57-pre-rebase`.
- [ ] Close str0m PR #1061 with thanks.
- [ ] Run the UDP+WebRTC harness for a no-floor baseline.

## Phase 1 — dependencies

- [ ] Move to str0m 0.24.0 from crates.io: drop the str0m git patch in
      `discordium-td/Cargo.toml`, bump renet-cross `str0m = "0.23.1"`, fix 0.24
      API changes, update the str0m row in `docs/network-dependencies.md`.
- [ ] Get sctp-proto PR #57 merged; remove the git patch once released.

## Phase 2 — packet sizing (double split)

- [ ] Measure the largest real netcode packet on the WebRTC path (renet slice +
      renet header + netcode header/MAC).
- [ ] Set `RtcConfig::set_mtu` in renet-cross so one netcode packet fits one
      SCTP packet (likely ~1350). No renet fork needed. Tradeoff: IP
      fragmentation risk over VPN/tunnel/TURN paths.
- [ ] Regression test: a full-size netcode packet produces exactly one SCTP DATA
      chunk server → client.
- [ ] Check browser → server packets fit Chrome's SCTP MTU (~1200 UDP).
- [ ] Fallback only if the MTU route is unacceptable: upstream renet PR making
      `SLICE_SIZE` configurable.

## Phase 3 — sctp-proto spec fixes (upstream)

- [ ] *Verify first:* read sctp-proto's send path; confirm the lifetime clock
      starts at first transmission (`c.since`) and unsent messages never expire.
- [ ] New upstream PR, timed reliability: measure lifetime from enqueue; abandon
      expired messages before TSN assignment. Test: messages queued behind a
      full cwnd are abandoned without ever getting a TSN. Cite:
  - RFC 3758 §4.1: lifetime expiry cancels sending "if the first transmission
    of the data does not take place (because of rwnd or cwnd limitations…)";
    TR3 (MUST evaluate lifetime before assigning a TSN, abandon if expired),
    TR4 (before every transmit/retransmit), TR6 (lifetime fixed when passed to
    SCTP).
  - RFC 9260 §11.1.5 SEND `life time`: "The user data will not be sent by SCTP
    after the life time expires."
  - RFC 8831 §6.1: data channels MUST support RFC 3758 timed reliability.
  - Prior art: webrtc-rs PR #235; Chrome dcSCTP drops expired messages from the
    send queue.
- [ ] *Verify first:* whether PR #57 already covers FORWARD-TSN/SACK ping-pong
      (pion/sctp #505) and timed retransmission (webrtc-rs #237).
- [ ] Do not make `maxRetransmits: 0` drop unsent messages: RFC 8831 §6.1 says
      such messages are "sent exactly once"; RFC 7496 §3.1 only abandons at
      the first retransmission.

## Phase 4 — channel configuration

- [ ] Replace `maxRetransmits(0)` with `maxPacketLifeTime(~100–200 ms)` in
      `renet-cross/src/web_client.rs:407` and the native WebRTC client
      (browsers reject setting both).
- [ ] *Verify first:* str0m/sctp-proto apply the channel's lifetime (from DCEP)
      to the server's own outgoing messages.

## Phase 5 — backpressure

- [ ] Server: per-client rate cap plus a saturation check that skips the tick
      instead of queueing (Unreal `IsNetReady`).
- [ ] Browser: check `bufferedAmount` before sending; skip or merge when high.
- [ ] Remove the patches these replace: `MAX_PEER_BACKLOG_BYTES` (64 KiB drain
      gate), bandwidth-delay-product shedding, keeping congested work queued in
      Renet (0.7.4), the 250 ms resend timer, the command-retry throttle.
      Restore Renet's normal redundancy.

## Phase 6 — congestion control decision

- [ ] Run the harness with stock RFC congestion control after phases 2–5.
- [ ] Only if insufficient: a configurable game-oriented controller in
      sctp-proto (rate cap that backs off on sustained loss or RTT growth, not a
      floor). Document it as a deliberate RFC 8831 deviation in
      `docs/network-contracts.md#epic-design-basis`.

## Phase 7 — netcode rebuild

- [ ] Owner corrections: unreliable, latest-wins, quantized, prediction fields
      only. Remove frozen reset, proof deadline, finalization audit, 16-batch
      window, 10 s reset deadline, deferred-resync machinery.
- [ ] Client input: newest move plus oldest unacknowledged moves; drop the
      separate retry loop.
- [ ] World state: changed fields only, quantized; on a lost packet re-send
      current values; packet-level acks instead of per-message decode receipts.
- [ ] Bandwidth: from ~75 KB/s to tens of KB/s per client (Unreal defaults
      10–15 KB/s).
- [ ] Bump the protocol identity whenever the serialized authoritative layout
      changes.

Keep from earlier work: local recovery instead of fail-stops, `network_now`,
flushing every frame, str0m drain-after-each-input, arrival-slack protocol,
relevance cadence (near 20 Hz, far 10 Hz), deltas over raw bytes, frame
compression.

## Phase 8 — qualification

- [ ] Harness scenarios over UDP and WebRTC, judged by the player-facing
      ledger: bandwidth caps below the send rate, burst loss, RTT spikes
      (100 → 600 ms), jitter bursts, Wi-Fi-like mixes.
- [ ] Soak and extended impairment runs are user-run (AGENTS.md long-running
      test handoff).

## Docs

- [ ] `docs/network-contracts.md` (Epic design basis): never queue stale data,
      Unreal rate-cap reasoning, any congestion-control deviation.
- [ ] Keep `docs/network-dependencies.md` and renet-cross README/CHANGELOG
      current as patches are added or removed.

## Later / optional

- [ ] WebTransport as the primary browser path with WebRTC fallback, if
      sctp-proto keeps producing bugs (datagrams are still congestion
      controlled; check Safari support).
- [ ] Track the renetcode fork (`fix/confirmation-keepalive-starvation`) and
      drop it once upstream.
