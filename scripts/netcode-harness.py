#!/usr/bin/env python3
"""Judge the network layer by what players see, over UDP and WebRTC.

Runs a real server and two rendered, autoplaying native clients per scenario,
over each transport the browser and native builds use. Every scenario is gated
on player-facing outcomes the client measures from its display, not on the
network implementation's own invariants:

  * the session never disconnects or asks the server to recover it;
  * the owner's character never moves further in a frame than physics allows
    by a metre or more (a teleport), and rarely by a visible margin at all;
  * the client stays connected for the run.

Scenarios reproduce the conditions players report: a clean link, a long WAN
path with jitter and loss, and the same path with periodic main-thread stalls
like the browser's garbage collection and shader compilation hitches.

Examples:
  python3 scripts/netcode-harness.py
  python3 scripts/netcode-harness.py --transports webrtc --scenarios wan-stall --duration 90
  python3 scripts/netcode-harness.py --no-build --binary-dir target/release

Artifacts (logs, traces, report.json) are written under ignored out/.
"""
from __future__ import annotations

import argparse
from contextlib import ExitStack
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import runpy
import socket
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
SPATIAL = runpy.run_path(str(Path(__file__).with_name("playtest-spatial.py")))
reserve_ports = SPATIAL["reserve_ports"]
launch = SPATIAL["launch"]
stop_owned = SPATIAL["stop_owned"]
latest_trace = SPATIAL["latest_trace"]
save_json = SPATIAL["save_json"]

TRANSPORTS = ("udp", "webrtc")
# Conditioner values are per direction, so "wan" is about 150 ms round trip.
SCENARIOS = {
    "clean": {"delay_ms": 0, "jitter_ms": 0, "loss_percent": 0, "stall": None},
    "wan": {"delay_ms": 75, "jitter_ms": 10, "loss_percent": 1, "stall": None},
    "wan-stall": {"delay_ms": 75, "jitter_ms": 10, "loss_percent": 1, "stall": "500@5"},
}
PLAYERS = (2001, 2002)
# Visible corrections are tolerated only at a rate a player would not notice
# as a pattern; any teleport, disconnect or recovery fails the scenario.
MAX_CORRECTIONS_PER_MINUTE = 6.0
MIN_CONNECTED_FRACTION = 0.9
STARTUP_ALLOWANCE_SECONDS = 20.0


def build(binary_dir: Path) -> None:
    subprocess.run(["cargo", "build", "--release", "--locked", "-p", "dreamwake_server",
                    "-p", "dreamwake_client", "--bin", "game_server", "--bin", "dreamwake"],
                   cwd=ROOT, check=True)
    for name in ("game_server", "dreamwake"):
        if not (binary_dir / name).is_file():
            raise SystemExit(f"build did not produce {binary_dir / name}")


def server_resyncs(log: Path) -> list[str]:
    try:
        text = log.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return []
    return [line.strip()[-240:] for line in text.splitlines()
            if re.search(r"resynchron|terminat|disconnect", line, re.IGNORECASE)][:40]


def client_warnings(log: Path) -> list[str]:
    try:
        text = log.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return []
    return [line.strip()[-240:] for line in text.splitlines()
            if re.search(r"WARN|ERROR|panick", line)][:40]


def transports_seen(path: Path) -> set[str]:
    seen = set()
    try:
        with path.open(encoding="utf-8") as source:
            for line in source:
                try:
                    kind = json.loads(line).get("transport")
                except ValueError:
                    continue
                if kind:
                    seen.add(kind)
    except OSError:
        pass
    return seen


def judge_client(side: str, trace: dict | None, seen: set[str], transport: str, stall: str | None,
                 exit_code: int | None, duration: float) -> dict:
    failures: list[str] = []
    health = (trace or {}).get("session_health") or {}
    if trace is None:
        failures.append("no trace records")
    if exit_code != 0:
        failures.append(f"client exited with {exit_code}")
    if seen != {transport}:
        failures.append(f"ran over {sorted(seen) or 'no transport'}, not {transport}")
    connects = health.get("connects", 0)
    if connects == 0:
        failures.append("never connected")
    if connects > 1:
        failures.append(f"reconnected {connects - 1} time(s)")
    # Quitting may record one disconnect at shutdown; a mid-run loss without a
    # reconnect shows up as lost connected time instead.
    if health.get("disconnects", 0) > 1:
        failures.append(f"disconnected {health['disconnects']} time(s)")
    if health.get("recoveries", 0) > 0:
        failures.append(f"requested {health['recoveries']} recovery resync(s)")
    if health.get("teleports", 0) > 0:
        failures.append(f"{health['teleports']} teleport frame(s), worst "
                        f"{health.get('max_excess_metres', 0):.2f} m beyond physics")
    connected = float(health.get("connected_seconds", 0.0))
    minutes = max(connected / 60.0, 1e-9)
    correction_rate = health.get("corrections", 0) / minutes
    if connected > 0 and correction_rate > MAX_CORRECTIONS_PER_MINUTE:
        failures.append(f"{correction_rate:.1f} visible corrections per minute")
    playable = max(duration - STARTUP_ALLOWANCE_SECONDS, 1.0)
    if connected / playable < MIN_CONNECTED_FRACTION:
        failures.append(f"connected {connected:.1f}s of {playable:.0f}s expected")
    if stall and not (trace or {}).get("stalls_injected"):
        failures.append("no stalls were injected; the stall scenario proved nothing")
    return {"side": side, "passed": not failures, "failures": failures, "health": health,
            "corrections_per_minute": correction_rate,
            "stalls_injected": (trace or {}).get("stalls_injected"),
            "transports": sorted(seen), "exit_code": exit_code}


def run_scenario(transport: str, name: str, directory: Path, duration: float, seed: int,
                 binaries: tuple[Path, Path]) -> dict:
    directory.mkdir(mode=0o700, parents=True)
    scenario = SCENARIOS[name]
    http, udp, webrtc = reserve_ports()
    base = f"http://127.0.0.1:{http}"
    server_command = [str(binaries[0]), "--identity-store", str(directory / "server.bin"),
        "--seed", str(seed), "--max-clients", str(len(PLAYERS)),
        "--http-bind", f"127.0.0.1:{http}", "--public-http-base", base,
        "--udp-bind", f"127.0.0.1:{udp}", "--public-udp-addr", f"127.0.0.1:{udp}",
        "--webrtc-bind", f"127.0.0.1:{webrtc}", "--public-webrtc-addr", f"127.0.0.1:{webrtc}",
        "--net-delay-ms", str(scenario["delay_ms"]),
        "--net-jitter-ms", str(scenario["jitter_ms"]),
        "--net-loss-percent", str(scenario["loss_percent"])]
    sides = ("left", "right")
    client_commands = []
    for player, side in zip(PLAYERS, sides):
        command = [str(binaries[1]), "--http-base", base, "--wait-players", str(len(PLAYERS)),
            "--player-id", str(player), "--profile-file", str(directory / f"{side}-profile.json"),
            "--autoplay-restart", "--transport", transport,
            "--network-trace", str(directory / f"{side}.ndjson"),
            "--quit-after", str(duration)]
        if scenario["stall"]:
            command += ["--inject-stall", scenario["stall"]]
        client_commands.append(command)
    result = {"transport": transport, "scenario": name, "conditions": scenario,
              "duration": duration, "seed": seed, "server_command": server_command,
              "client_commands": client_commands, "errors": []}
    processes = []
    exit_codes: list[int | None] = [None] * len(sides)
    with ExitStack() as handles:
        try:
            server = launch(server_command, directory / "server.log", handles)
            processes.append(server)
            deadline = time.monotonic() + 30
            while True:
                if server.poll() is not None:
                    raise RuntimeError("server exited before bootstrap became ready")
                try:
                    with socket.create_connection(("127.0.0.1", http), timeout=0.2):
                        break
                except OSError:
                    if time.monotonic() >= deadline:
                        raise RuntimeError("bootstrap readiness timed out")
                    time.sleep(0.1)
            clients = [launch(command, directory / f"{side}.log", handles)
                       for command, side in zip(client_commands, sides)]
            processes.extend(clients)
            deadline = time.monotonic() + duration + 60
            while any(process.poll() is None for process in clients):
                if server.poll() is not None:
                    raise RuntimeError("server exited while clients were running")
                if time.monotonic() >= deadline:
                    raise RuntimeError("clients exceeded duration plus 60-second allowance")
                time.sleep(0.5)
            exit_codes = [process.returncode for process in clients]
        except (OSError, RuntimeError, KeyboardInterrupt) as error:
            result["errors"].append(str(error) or "interrupted")
        finally:
            for process in reversed(processes):
                stop_owned(process)
            if len(processes) == 1 + len(sides):
                exit_codes = [process.returncode for process in processes[1:]]
    result["clients"] = [judge_client(side, latest_trace(directory / f"{side}.ndjson"),
                                      transports_seen(directory / f"{side}.ndjson"),
                                      transport, scenario["stall"], code, duration)
                         for side, code in zip(sides, exit_codes)]
    for client, side in zip(result["clients"], sides):
        client["log_warnings"] = client_warnings(directory / f"{side}.log")
    result["server_events"] = server_resyncs(directory / "server.log")
    result["passed"] = not result["errors"] and all(c["passed"] for c in result["clients"])
    save_json(directory / "result.json", result)
    return result


def summary_line(result: dict) -> str:
    verdict = "PASS" if result["passed"] else "FAIL"
    parts = []
    for client in result["clients"]:
        h = client["health"]
        parts.append(f"{client['side']}: conn {h.get('connected_seconds', 0):.0f}s "
                     f"rec {h.get('recoveries', 0)} disc {h.get('disconnects', 0)} "
                     f"tele {h.get('teleports', 0)} corr {h.get('corrections', 0)} "
                     f"max {h.get('max_excess_metres', 0):.2f}m stalls {client['stalls_injected']}")
    return f"{verdict} {result['transport']:>6} {result['scenario']:<10} " + " | ".join(parts)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--transports", nargs="+", choices=TRANSPORTS, default=list(TRANSPORTS))
    parser.add_argument("--scenarios", nargs="+", choices=SCENARIOS, default=list(SCENARIOS))
    parser.add_argument("--duration", type=float, default=45.0,
                        help="client lifetime in seconds per scenario, 30-600 (default 45)")
    parser.add_argument("--seed", type=int, default=4242)
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--binary-dir", type=Path, default=ROOT / "target" / "release")
    parser.add_argument("--output-root", type=Path, default=ROOT / "out" / "netcode-harness")
    args = parser.parse_args()
    if not 30 <= args.duration <= 600:
        parser.error("--duration must be between 30 and 600 seconds")
    binary_dir = args.binary_dir.resolve()
    if not args.no_build:
        build(binary_dir)
    binaries = (binary_dir / "game_server", binary_dir / "dreamwake")
    for binary in binaries:
        if not os.access(binary, os.X_OK):
            parser.error(f"missing executable {binary}")
    run = args.output_root / datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    results = []
    for transport in args.transports:
        for name in args.scenarios:
            print(f"running {transport} {name} for {args.duration:.0f}s ...", flush=True)
            result = run_scenario(transport, name, run / f"{transport}-{name}", args.duration,
                                  args.seed, binaries)
            print(summary_line(result), flush=True)
            for client in result["clients"]:
                for failure in client["failures"]:
                    print(f"    {client['side']}: {failure}", flush=True)
            for error in result["errors"]:
                print(f"    error: {error}", flush=True)
            results.append(result)
    report = {"run": str(run), "passed": all(r["passed"] for r in results),
              "results": results}
    save_json(run / "report.json", report)
    print(f"{'PASSED' if report['passed'] else 'FAILED'}: {run / 'report.json'}")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
