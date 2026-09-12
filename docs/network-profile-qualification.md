# Manual network profile qualification

This procedure prepares the external impairment portion of Engineering Spec
§22.2. Run it manually on a declared Linux test machine after the candidate is
frozen. It has not been executed as qualification. Native binaries captured on
macOS must be rebuilt from the identical frozen sources for Linux and hashed;
copying their filenames does not make them Linux executables.

The [persistent game soak](network-game-soak.md) supplies a separate two-bot UDP
endurance procedure. The [clock runner](network-clock-qualification.md) supplies
frequency-injection and rational-clock evidence. Neither replaces human sessions,
the full profile matrix, or reference-hardware response measurements.

## Preparation and isolated endpoints

Record the source manifest, executable hashes, OS/kernel, `ip -Version`,
`tc -Version`, `iperf3 --version`, display path and driver frame-limit method.
Choose a new output directory, two test player identities and an operator for
human rows. Confirm that `tc netem` supports `seed`. Unsupported options are a
preflight failure; do not silently remove them.

Network namespaces keep these test interfaces separate from the host's normal
network. The setup uses a veth pair and changes only its endpoints. Run these
commands once in an operator terminal; an existing namespace name must be
investigated rather than reused. See the [ip-netns manual](https://www.man7.org/linux/man-pages/man8/ip-netns.8.html).

```sh
set -eu
sudo ip netns add dwq-server
sudo ip netns add dwq-client
sudo ip link add dwq-srv type veth peer name dwq-cli
sudo ip link set dwq-srv netns dwq-server
sudo ip link set dwq-cli netns dwq-client
sudo ip -n dwq-server address add 10.239.0.1/30 dev dwq-srv
sudo ip -n dwq-client address add 10.239.0.2/30 dev dwq-cli
sudo ip -n dwq-server link set lo up
sudo ip -n dwq-client link set lo up
sudo ip -n dwq-server link set dwq-srv up
sudo ip -n dwq-client link set dwq-cli up
```

Set `DWQ_BIN` to the absolute directory containing the verified Linux release
binaries from [paired native capture preparation](network-game-soak.md#captured-inputs)
and `DWQ_OUTPUT` to a new per-attempt artifact directory. Create the latter as
the desktop user. After applying a profile below, start this server in its own
operator terminal, retaining the output in `server.log`:

```sh
sudo ip netns exec dwq-server "$DWQ_BIN/game_server" \
  --identity-store "$DWQ_OUTPUT/server.bin" --seed 8192 \
  --replication-distance 12 --max-clients 2 \
  --http-bind 0.0.0.0:18080 --public-http-base http://10.239.0.1:18080 \
  --udp-bind 0.0.0.0:18081 --public-udp-addr 10.239.0.1:18081 \
  --webrtc-bind 0.0.0.0:18082 --public-webrtc-addr 10.239.0.1:18082 \
  --net-delay-ms 0 --net-jitter-ms 0 --net-loss-percent 0 \
  > "$DWQ_OUTPUT/server.log" 2>&1
```

The explicit zeroes prevent double conditioning. Start this left bot in a
second terminal, and a second copy using `right` and player `1002` in a third:

```sh
sudo --preserve-env=DISPLAY,XAUTHORITY,WAYLAND_DISPLAY,XDG_RUNTIME_DIR,DBUS_SESSION_BUS_ADDRESS \
  ip netns exec dwq-client \
  sudo -u "$USER" --preserve-env=DISPLAY,XAUTHORITY,WAYLAND_DISPLAY,XDG_RUNTIME_DIR,DBUS_SESSION_BUS_ADDRESS \
  "$DWQ_BIN/dreamwake" --http-base http://10.239.0.1:18080 --wait-players 2 \
  --player-id 1001 --profile-file "$DWQ_OUTPUT/left-profile.json" \
  --spatial-playtest left --network-trace "$DWQ_OUTPUT/left.ndjson" \
  --network-trace-chunks 8 --network-trace-interval-seconds 0.25 \
  --capture-dir "$DWQ_OUTPUT/left-captures" --quit-after 300 \
  > "$DWQ_OUTPUT/left.log" 2>&1
```

For human rows, omit `--spatial-playtest left/right` and use ordinary input.
The 300-second process ceiling includes setup; the measurement interval starts
only when both clients are active. If admission leaves fewer than the declared
120 active seconds, the row is incomplete. Record actual game PIDs from their
traces rather than assuming the outer `sudo` PID is the game process. GUI
processes retain the desktop user's display/session environment and profile.

For browser rows, build web assets against `http://10.239.0.1:18080`, run their
HTTP server in `dwq-client`, and launch a separate browser profile in that same
namespace using the desktop-user prefix. Record the resulting web hashes. Use
the normal lobby and F6 controls; no URL configuration switches are supported.
Desktop/browser setup and an actual connected visual preflight must succeed
before scheduling the extended matrix.

## Profile commands

Define this helper in the same operator shell. Downlink is the server endpoint;
uplink is the client endpoint. Every direction has a finite 256-packet queue.
The seed controls netem's random loss sequence; it does not make OS packet arrival schedules
identical. Preserve measured RTT, loss and queue statistics alongside the seed.
The [upstream netem manual](https://kernel.googlesource.com/pub/scm/network/iproute2/iproute2-next/+/refs/heads/main/man/man8/tc-netem.8)
documents delay, loss, duplication, reordering, rate and seed options; actual
timing remains subject to kernel granularity.

```sh
export DWQ_SEED=8192
netem() {
  case "$1" in
    down) ns=dwq-server; device=dwq-srv ;;
    up) ns=dwq-client; device=dwq-cli ;;
    *) return 2 ;;
  esac
  shift
  python3 -c 'import time; print("conditioner_monotonic_ns", time.monotonic_ns())'
  sudo ip netns exec "$ns" tc qdisc replace dev "$device" root \
    netem limit 256 "$@" seed "$DWQ_SEED"
  sudo ip netns exec "$ns" tc -s -j qdisc show dev "$device"
}
both() { netem down "$@"; netem up "$@"; }
```

Apply the selected row before starting its gameplay session. Delays and jitter
below are per direction. Record both configured and observed round-trip values.
These are explicit test profiles, not measured network capabilities.

| Row | Operator command |
| --- | --- |
| LAN | `both delay 1ms` |
| Typical | `both delay 20ms 5ms loss random 1%` |
| Moderate | `both delay 50ms 15ms loss random 3%` |
| Difficult | `both delay 100ms 30ms loss random 5%` |
| Asymmetric uplink | `netem down delay 10ms; netem up delay 90ms` |
| Asymmetric downlink | `netem down delay 90ms; netem up delay 10ms` |
| Reordered | `both delay 20ms reorder 10% duplicate 1%` |
| Constrained | `both delay 20ms 5ms loss random 1% rate 2mbit` |
| Severe | `both delay 100ms 30ms loss random 10%` |

For Burst, start with Typical and, after admission, run this finite transition
sequence while collecting command timestamps and client traces:

```sh
for burst in 0.1 0.2 0.3; do
  sleep 20
  both delay 20ms 5ms loss random 100%
  sleep "$burst"
  both delay 20ms 5ms loss random 1%
done
```

Replacing qdiscs may drop queued packets. Record the transition interval and
actual outage; do not report nominal sleeps as measured outage lengths. For
Reordered, capture observed reorder/duplicate counts: a percentage option alone
does not prove packets arrived out of order. For Severe, additionally pause one
recorded client PID for three seconds, resume it, and retain fresh-stream recovery
evidence. Do not restart the process to erase the fault.

Constrained also needs competing bulk traffic. In another operator terminal,
run a one-connection iperf server in `dwq-server`, then run its client in
`dwq-client` during the game session:

```sh
sudo ip netns exec dwq-server iperf3 -s -1 -p 19090 -J
```

```sh
sudo ip netns exec dwq-client iperf3 -c 10.239.0.1 -p 19090 \
  -u -b 1800K --bidir -t 120 -J --get-server-output
```

Preserve both outputs. This is a declared 1.8 Mbit/s offered load in each
direction sharing the 2 Mbit/s test link; use delivered throughput and queue
statistics to establish actual contention. See the [iperf invocation guide](https://software.es.net/iperf/invoking.html).

## Matrix and evidence

Before running, write a matrix with all ten rows (including both asymmetric
directions), seeds `8192` and `16384`, both bot and human sessions, and measured
render rates of 30, 60, 120 and 240 FPS at the same server tick rate. A proposed
120-second active interval per combination is 160 sessions, about 5 hours
20 minutes before setup and fault exercises. Freeze the chosen durations and
seed set with the run manifest. This entire matrix is a manual extended run.

Set render limits through the declared driver/display configuration and verify
the observed distribution. An unsupported 240 FPS machine cannot qualify that
row by recording a requested cap. The current game has no `--fps` CLI switch.
Include background/foreground suspension, 250 ms and one-second client stalls,
and documented server scheduling/overload faults. Record every injected fault's
PID, monotonic start/end and recovery boundary. The clock guide supplies a
separate controlled-frequency fixture; it must not be called an injected game
clock test.

For each attempt retain launch arguments, source/binary/web hashes, process
identity and exit status, conditioner changes/statistics, complete bounded
traces, snapshots/screenshots, raw response/correction distributions and human
observations. Record successful snapshot opportunities separately from elapsed
recovery seconds. Check all §22.4 authority, lifecycle, scope, bounded-memory,
decoder and reconciliation invariants; short sampled traces alone are not an
exhaustive duplicate-action or privacy proof. Hardware response measurements
must include input/display latency and separate externally caused corrections.

After stopping the exact recorded test PIDs, require these namespace process
lists to be empty before removing the two test namespaces:

```sh
sudo ip netns pids dwq-server
sudo ip netns pids dwq-client
test -z "$(sudo ip netns pids dwq-server)"
test -z "$(sudo ip netns pids dwq-client)"
sudo ip netns del dwq-server
sudo ip netns del dwq-client
```

Keep failed and unsupported rows visible. This procedure provides concrete
external profile configuration; Linux/GUI feasibility, hardware assignment,
visual results and the full matrix remain unqualified until operator artifacts
establish them.
