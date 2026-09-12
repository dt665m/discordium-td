#!/usr/bin/env python3
"""Run two rendered native Dreamwake clients and verify spatial scope reentry.

Examples:
  python3 scripts/playtest-spatial.py --profiles lan typical moderate --duration 120
  python3 scripts/playtest-spatial.py --no-build --binary-dir out/spatial-playtest/RUN/binaries --profiles lan
  python3 scripts/playtest-spatial.py --analyze out/spatial-playtest/RUN

Live-mode artifacts and isolated credentials stay under ignored out/. Explicit
--source-manifest frozen mode permits external output, requires --no-build and
--binary-dir, and verifies matching native-provenance.json without Git discovery.
Frozen --analyze requires the same source and binary identities as the run. This exercises
ordinary client input and production admission/transport; it does not inject game
state or qualify browser, human play, external hardware, or the full network matrix.
"""
from __future__ import annotations

import argparse
from contextlib import ExitStack
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import runpy
import signal
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import zlib
from datetime import datetime, timezone

ROOT = Path(__file__).resolve().parents[1]
NATIVE_HELPERS = runpy.run_path(str(Path(__file__).with_name("build-native-capture.py")))
PROFILES = {
    "lan": {"delay_ms_per_direction": 0, "jitter_ms_per_direction": 0, "loss_percent_per_direction": 0},
    "typical": {"delay_ms_per_direction": 20, "jitter_ms_per_direction": 5, "loss_percent_per_direction": 1},
    "moderate": {"delay_ms_per_direction": 50, "jitter_ms_per_direction": 15, "loss_percent_per_direction": 3},
}
PLAYERS = (1001, 1002)
TRACE_LIMIT = 8 * 1024 * 1024
TICK_SKEW = 0
MIN_OBSERVATIONS = 3
GRAPH_CELL_METRES = 32.0
MIN_OBSERVED_CELLS = 3
RECOVERY_DEADLINE_SECONDS = 10.0
END_ACTIVE_WINDOW_SECONDS = 3.0
DECODE_COUNTERS = ("decode_errors", "stale_epoch_rejections", "clock_resync_rejections",
                   "obsolete_rejections", "decode_failures")


def save_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, indent=2, allow_nan=False) + "\n", encoding="utf-8")


def integer(value: object, minimum: int = 0) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value >= minimum


def scope_key(value: object) -> tuple[int, int, int, int, int] | None:
    if not isinstance(value, dict) or not isinstance(value.get("entity"), dict):
        return None
    entity = value["entity"]
    result = (value.get("connection"), entity.get("index"), entity.get("generation"),
              value.get("scope"), value.get("representation"))
    return result if all(integer(v, 1) for v in result) else None


def hero_scope(record: dict, hero: int) -> tuple[int, int, int, int, int] | None:
    entries = record.get("hero_scopes", [])
    if not isinstance(entries, list):
        return None
    matches = [scope_key(entry.get("scope")) for entry in entries
               if isinstance(entry, dict) and entry.get("hero_id") == hero]
    live = {scope_key(scope) for scope in record["scopes"]}
    return matches[0] if len(matches) == 1 and matches[0] in live else None


def read_trace(path: Path) -> tuple[list[dict], list[str]]:
    errors: list[str] = []
    records: list[dict] = []
    if not path.is_file():
        return [], [f"missing trace: {path.name}"]
    if path.stat().st_size > TRACE_LIMIT:
        return [], [f"trace exceeds {TRACE_LIMIT} bytes: {path.name}"]
    previous_elapsed = -1.0
    with path.open(encoding="utf-8") as source:
        for line_number, line in enumerate(source, 1):
            try:
                row = json.loads(line)
                if not isinstance(row, dict) or row.get("format") != 1:
                    raise ValueError("unsupported record format")
                elapsed = row.get("elapsed")
                position = row.get("position")
                if (not isinstance(elapsed, (int, float)) or not math.isfinite(elapsed)
                    or elapsed < previous_elapsed
                    or not isinstance(position, list) or len(position) != 2
                    or any(not isinstance(v, (int, float)) or not math.isfinite(v) for v in position)
                    or not all(integer(row.get(name)) for name in ("pid", "owner", "match_epoch", "server_tick"))
                    or not isinstance(row.get("heroes"), list)
                    or not all(integer(v, 1) for v in row["heroes"])
                    or not isinstance(row.get("scopes"), list)
                    or any(scope_key(v) is None for v in row["scopes"])
                    or not isinstance(row.get("active"), bool)
                    or not isinstance(row.get("separated"), bool)):
                    raise ValueError("invalid identity, pose, time, or scope fields")
                if row["active"] and (authority_tick(row) is None
                    or not isinstance(row.get("hero_scopes"), list)
                    or any(not isinstance(entry, dict) or not integer(entry.get("end_tick"))
                           or not valid_position(entry.get("position"))
                           or not integer(entry.get("hero_id"), 1)
                           or scope_key(entry.get("scope")) is None
                           for entry in row["hero_scopes"])):
                    raise ValueError("active observation lacks authoritative checkpoint or timestamped hero scopes")
                previous_elapsed = elapsed
                records.append({**row, "line": line_number})
            except (ValueError, TypeError) as error:
                errors.append(f"{path.name}:{line_number}: {error}")
                if len(errors) >= 10:
                    break
    if not records:
        errors.append(f"no valid observations: {path.name}")
    return records, errors


def authority_tick(row: dict) -> int | None:
    tick = row.get("owner_checkpoint_tick")
    pose = row.get("authoritative_owner_position")
    return tick if integer(tick) and valid_position(pose) else None


def valid_position(pose: object) -> bool:
    return (isinstance(pose, list) and len(pose) == 2
            and all(isinstance(v, (int, float)) and not isinstance(v, bool)
                    and math.isfinite(v) for v in pose))


def observation_pairs(left: list[dict], right: list[dict]) -> list[tuple[dict, dict]]:
    """Only exact authoritative checkpoint times; never predicted/interpolated poses."""
    by_time: dict[tuple[int, int], list[dict]] = {}
    for row in right:
        tick = authority_tick(row)
        if row["active"] and row.get("phase") == "Combat" and row["match_epoch"] > 0 and tick is not None:
            by_time.setdefault((row["match_epoch"], tick), []).append(row)
    used: set[int] = set()
    paired = []
    for row in left:
        tick = authority_tick(row)
        if not row["active"] or row.get("phase") != "Combat" or tick is None:
            continue
        candidates = [other for other in by_time.get((row["match_epoch"], tick), [])
                      if other["line"] not in used and abs(other["elapsed"] - row["elapsed"]) <= 1.0]
        if candidates:
            other = min(candidates, key=lambda other: abs(other["elapsed"] - row["elapsed"]))
            used.add(other["line"])
            paired.append((row, other))
    return paired


def witness(left: dict, right: dict) -> dict:
    return {"match_epoch": left["match_epoch"], "ticks": [authority_tick(left), authority_tick(right)],
            "lines": [left["line"], right["line"]],
            "positions": [left["authoritative_owner_position"], right["authoritative_owner_position"]],
            "distance": math.dist(left["authoritative_owner_position"], right["authoritative_owner_position"]),
            "heroes": [left["heroes"], right["heroes"]]}


def fresh_disclosures(pairs: list[tuple[dict, dict]], traces: tuple[list[dict], list[dict]]) -> list[dict]:
    # A retained draw-list entry says nothing about when information was sent.
    # Match the remote FullState's timestamp to BOTH actual owner checkpoints.
    known_far = {(a["match_epoch"], authority_tick(a)): (a, b) for a, b in pairs
                 if math.dist(a["authoritative_owner_position"], b["authoritative_owner_position"]) >= 23.0}
    leaks = []
    seen = set()
    for direction, rows in enumerate(traces):
        remote = PLAYERS[1-direction]
        for row in rows:
            if not row["active"]:
                continue
            for entry in row.get("hero_scopes", []):
                if entry.get("hero_id") != remote or scope_key(entry.get("scope")) is None:
                    continue
                tick = entry.get("end_tick")
                pair = known_far.get((row["match_epoch"], tick))
                identity = (direction, row["match_epoch"], tick, scope_key(entry["scope"]))
                if pair is not None and identity not in seen:
                    seen.add(identity)
                    leaks.append({**witness(*pair), "observer": PLAYERS[direction],
                                  "remote_update_tick": tick, "remote_update_line": row["line"],
                                  "scope": entry["scope"]})
    return leaks


def connection_continuity(rows: list[dict], end: float) -> dict:
    """Time-weighted sampled state, with initial admission outside recovery gating."""
    active_duration = 0.0
    first_active = None
    last_active = None
    inactive_since = None
    longest_inactive = 0.0
    for index, row in enumerate(rows):
        elapsed = row["elapsed"]
        if elapsed > end:
            break
        next_elapsed = min(end, rows[index + 1]["elapsed"] if index + 1 < len(rows) else end)
        if row["active"]:
            active_duration += max(0.0, next_elapsed - elapsed)
            if first_active is None:
                first_active = elapsed
            last_active = elapsed
            if inactive_since is not None:
                longest_inactive = max(longest_inactive, elapsed - inactive_since)
                inactive_since = None
        elif first_active is not None and inactive_since is None:
            inactive_since = elapsed
    if inactive_since is not None:
        longest_inactive = max(longest_inactive, end - inactive_since)
    return {"first_active_elapsed": first_active, "last_active_elapsed": last_active,
            "active_duration_seconds": active_duration,
            "active_fraction": active_duration / end if end > 0 else 0.0,
            "longest_inactive_after_activation_seconds": longest_inactive,
            "active_near_run_end": last_active is not None and end - last_active <= END_ACTIVE_WINDOW_SECONDS}


def decode_health(rows: list[dict]) -> dict:
    """Audit lifetime counters, including startup/recovery; latest issue text is diagnostic only."""
    maxima = {name: None for name in DECODE_COUNTERS}
    first = {}
    for index, row in enumerate(rows, 1):
        observed = {"line": row.get("line", index), "elapsed": row["elapsed"]}
        counters = {name: row[name] for name in DECODE_COUNTERS if integer(row.get(name))}
        if len(counters) != len(DECODE_COUNTERS):
            first.setdefault("unverifiable_classification", {
                **observed, "missing_or_invalid_fields": [name for name in DECODE_COUNTERS if name not in counters]})
        elif counters["decode_errors"] != sum(counters[name] for name in DECODE_COUNTERS[1:]):
            first.setdefault("partition_mismatch", {**observed, "counters": counters,
                             "classified_total": sum(counters[name] for name in DECODE_COUNTERS[1:])})
        regressed = [name for name, value in counters.items() if maxima[name] is not None and value < maxima[name]]
        if regressed:
            first.setdefault("counter_regression", {**observed, "counters": counters,
                             "regressed_fields": regressed, "previous_maxima": dict(maxima)})
        if counters.get("decode_failures", 0) > 0:
            first.setdefault("hard_failure", {**observed, "counters": counters,
                             "latest_issue_at_observation": row.get("decode_issue")
                             if isinstance(row.get("decode_issue"), str) else None})
        for name, value in counters.items():
            maxima[name] = value if maxima[name] is None else max(maxima[name], value)
    failures = []
    if not rows:
        failures.append("lifetime decode classification is unverifiable: no observations")
    if issue := first.get("unverifiable_classification"):
        failures.append(f"lifetime decode classification is unverifiable at line {issue['line']}: "
                        f"missing or invalid {', '.join(issue['missing_or_invalid_fields'])}")
    if issue := first.get("partition_mismatch"):
        failures.append(f"decode counter partition is inconsistent at line {issue['line']}: "
                        f"decode_errors={issue['counters']['decode_errors']}, classified_total={issue['classified_total']}")
    if issue := first.get("counter_regression"):
        failures.append(f"lifetime decode counters regressed at line {issue['line']}: {', '.join(issue['regressed_fields'])}")
    if issue := first.get("hard_failure"):
        failures.append(f"lifetime hard decode_failures reached {maxima['decode_failures']}; "
                        f"first observed at line {issue['line']} ({issue['elapsed']:g}s)")
    return {"passed": not failures, "failures": failures, "observed_maxima": maxima,
            "first_issues": first,
            "classification_verified": bool(rows) and not any(name in first for name in
                ("unverifiable_classification", "partition_mismatch", "counter_regression"))}


def analyze_records(left: list[dict], right: list[dict], expected_pids: list[int] | None = None,
                    duration: float | None = None) -> dict:
    failures: list[str] = []
    pid_sets = [sorted({row["pid"] for row in rows}) for rows in (left, right)]
    if any(len(pids) != 1 or pids[0] <= 0 for pids in pid_sets) or pid_sets[0] == pid_sets[1]:
        failures.append("traces must originate from two distinct, stable process IDs")
    if expected_pids is not None and pid_sets != [[pid] for pid in expected_pids]:
        failures.append("trace process IDs do not match the launched client processes")
    for rows, owner in zip((left, right), PLAYERS):
        if not any(row["active"] and row["owner"] == owner for row in rows):
            failures.append(f"player {owner} never became active")
        if any(row["active"] and row["owner"] != owner for row in rows):
            failures.append(f"active player identity changed from expected {owner}")
    pairs = observation_pairs(left, right)
    far = []
    leaks = fresh_disclosures(pairs, (left, right))
    retained = []
    reentries: list[dict | None] = [None, None]
    previous_near: list[dict[int, tuple[dict, tuple]]] = [{}, {}]
    exited: list[dict[int, tuple[dict, tuple, dict]]] = [{}, {}]
    for a, b in pairs:
        epoch = a["match_epoch"]
        distance = math.dist(a["authoritative_owner_position"], b["authoritative_owner_position"])
        # A new waypoint intent can arrive while the authoritative position is
        # still near the previous waypoint. Only actual same-tick poses prove
        # separation or return; controller intent cannot veto either witness.
        separated = distance >= 23.0
        center = (distance <= 8.0
                  and all(math.dist(row["authoritative_owner_position"], [0.0, 2.0]) <= 5.0 for row in (a, b)))
        if separated:
            proof = witness(a, b)
            far.append(proof)
            if PLAYERS[1] in a["heroes"] or PLAYERS[0] in b["heroes"]:
                retained.append(proof)
        for direction, (row, other_id) in enumerate(((a, PLAYERS[1]), (b, PLAYERS[0]))):
            current_scope = hero_scope(row, other_id)
            if center and other_id in row["heroes"] and current_scope is not None:
                old = exited[direction].get(epoch)
                if old is not None and reentries[direction] is None:
                    prior_row, prior_scope, exit_proof = old
                    # Same connection, entity generation and representation; only
                    # the scope incarnation advances. Reconnects cannot pass this.
                    if (current_scope[:3] == prior_scope[:3]
                        and current_scope[4] == prior_scope[4]
                        and current_scope[3] > prior_scope[3]):
                        reentries[direction] = {"observer": PLAYERS[direction], "hero_id": other_id,
                            "before_line": prior_row["line"], "after_line": row["line"],
                            "before_scope": prior_scope, "after_scope": current_scope,
                            "exit": exit_proof, "return": witness(a, b)}
                previous_near[direction][epoch] = (row, current_scope)
            if separated and other_id not in row["heroes"]:
                prior = previous_near[direction].get(epoch)
                if prior is not None:
                    prior_row, prior_scope = prior
                    live_entities = {scope_key(scope)[1:3] for scope in row["scopes"]}
                    if current_scope is None and prior_scope[1:3] not in live_entities:
                        exited[direction][epoch] = (prior_row, prior_scope, witness(a, b))
    unique_far = {(entry["match_epoch"], tuple(entry["ticks"])) for entry in far}
    if len(unique_far) < MIN_OBSERVATIONS:
        failures.append(f"only {len(unique_far)} eligible separated observations; require {MIN_OBSERVATIONS}")
    if leaks:
        failures.append(f"fresh remote FullState disclosed at {len(leaks)} proven authoritative far observations")
    for direction, proof in enumerate(reentries):
        if proof is None:
            failures.append(f"player {PLAYERS[direction]} lacks a proven same-entity newer-scope return for {PLAYERS[1-direction]}")
    metrics = []
    end = duration if duration is not None else max((row["elapsed"] for rows in (left, right) for row in rows), default=0.0)
    for rows, owner in zip((left, right), PLAYERS):
        health = decode_health(rows)
        failures.extend(f"player {owner}: {failure}" for failure in health["failures"])
        continuity = connection_continuity(rows, end)
        if not continuity["active_near_run_end"]:
            failures.append(f"player {owner} was not active within {END_ACTIVE_WINDOW_SECONDS:g}s of run end")
        if continuity["longest_inactive_after_activation_seconds"] > RECOVERY_DEADLINE_SECONDS:
            failures.append(f"player {owner} exceeded the {RECOVERY_DEADLINE_SECONDS:g}s recovery deadline after activation")
        active = [row for row in rows if row["active"]]
        cells = sorted({tuple(math.floor(axis / GRAPH_CELL_METRES)
                              for axis in row["authoritative_owner_position"])
                        for row in active if authority_tick(row) is not None})
        if len(cells) < MIN_OBSERVED_CELLS:
            failures.append(f"player {owner} visited only {len(cells)} authoritative graph cells; require {MIN_OBSERVED_CELLS}")
        maxima = {}
        for name in ("pending_commands", "history_bytes", "scope_bytes", "staging_bytes", "rtt_ms"):
            values = [row[name] for row in active if isinstance(row.get(name), (int, float))
                      and math.isfinite(row[name])]
            maxima[name] = max(values) if values else None
        metrics.append({"records": len(rows), "active_records": len(active), "observed_maxima": maxima,
                        "decode_health": health,
                        "authoritative_graph_cells": cells,
                        "connection_continuity": continuity})
    return {"passed": not failures, "failures": failures, "process_ids": pid_sets,
            "paired_observations": len(pairs), "eligible_separated_observations": len(unique_far),
            "separated_examples": far[:3], "disclosure_violations": leaks[:10],
            "retained_remote_examples": retained[:10],
            "measurement_boundary": "Positions are actual owner checkpoints at exactly equal server ticks; no predicted pose, interpolation, or waypoint intent is used. Fresh disclosure requires a decoded remote FullState end_tick matching that same far world time. Retained older replicas alone are not fresh disclosure. Omission and same-entity newer-scope return must still be proven. Missing or unmatched timestamps cannot qualify. This sampled rendered-client check is not a production privacy proof or an exhaustive packet audit.",
            "reentries": reentries, "metrics": metrics,
            "criteria": {"minimum_distance": 23.0, "maximum_tick_skew": TICK_SKEW,
                         "minimum_observations": MIN_OBSERVATIONS,
                         "graph_cell_metres": GRAPH_CELL_METRES,
                         "minimum_observed_cells_per_player": MIN_OBSERVED_CELLS,
                         "maximum_inactive_after_activation_seconds": RECOVERY_DEADLINE_SECONDS,
                         "active_within_seconds_of_run_end": END_ACTIVE_WINDOW_SECONDS,
                         "maximum_lifetime_hard_decode_failures": 0,
                         "decode_counter_partition": "decode_errors = stale_epoch_rejections + clock_resync_rejections + obsolete_rejections + decode_failures",
                         "decode_counter_method": "Every observation, including inactive startup and recovery, requires nonnegative integer monotonic lifetime counters with an exact partition. Expected stale-epoch, typed clock-resync and obsolete rejections remain separate. Latest issue text cannot classify earlier failures; missing legacy classification is unverifiable and fails.",
                         "active_duration_method": "Sample state held until next observation; fraction uses requested run duration. Initial admission is excluded only from recovery deadline gating."}}


def analyze_profile(directory: Path, process_run: dict) -> dict:
    traces = [read_trace(directory / f"{side}.ndjson") for side in ("left", "right")]
    report = analyze_records(traces[0][0], traces[1][0], process_run.get("client_pids"), process_run.get("duration"))
    report["failures"].extend(error for _, errors in traces for error in errors)
    for side, rows in zip(("left", "right"), (traces[0][0], traces[1][0])):
        captures = []
        for path in sorted((directory / side).glob("*.png")):
            if capture_has_color(path):
                captures.append(path)
        report.setdefault("captures", {})[side] = [str(path.relative_to(directory)) for path in captures]
        if not captures:
            report["failures"].append(f"{side} produced no validated PNG capture with nonblack color")
        if rows and rows[-1]["elapsed"] < process_run.get("duration", 0) - 3.0:
            report["failures"].append(f"{side} trace stopped before the requested duration")
    if process_run.get("client_exit_codes") != [0, 0]:
        report["failures"].append("both clients must exit successfully through --quit-after")
    if process_run.get("pause_at") is not None:
        pause = process_run.get("pause_recovery", {})
        report["pause_recovery"] = pause
        if (not pause.get("passed") or not recovered_after_pause(pause.get("before", {}), pause.get("after"))
                or pause.get("observed_recovery_seconds", math.inf) > RECOVERY_DEADLINE_SECONDS):
            report["failures"].append("controlled pause lacks proven fresh-stream recovery within 10 seconds")
    report["failures"].extend(process_run.get("errors", []))
    report["passed"] = not report["failures"]
    save_json(directory / "analysis.json", report)
    return report


def capture_has_color(path: Path) -> bool:
    """Reject black captures without an imaging dependency; not scene recognition.

    For a completely black RGB image every filtered RGB byte is zero under all
    five PNG filters. Alpha is deliberately ignored. Bevy writes 8-bit RGB(A).
    """
    if not 1024 < path.stat().st_size <= 32 * 1024 * 1024:
        return False
    data = path.read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        return False
    offset = 8
    header = None
    compressed = bytearray()
    while offset + 12 <= len(data):
        length = int.from_bytes(data[offset:offset + 4], "big")
        kind = data[offset + 4:offset + 8]
        if offset + length + 12 > len(data):
            return False
        body = data[offset + 8:offset + 8 + length]
        if kind == b"IHDR":
            header = body
        elif kind == b"IDAT":
            compressed.extend(body)
        offset += length + 12
        if kind == b"IEND":
            break
    if header is None or len(header) != 13 or header[8] != 8 or header[9] not in (2, 6) or header[12] != 0:
        return False
    width, height = int.from_bytes(header[:4], "big"), int.from_bytes(header[4:8], "big")
    if not (1 <= width <= 8192 and 1 <= height <= 8192):
        return False
    channels = 3 if header[9] == 2 else 4
    stride = width * channels
    budget = (stride + 1) * height
    if budget > 128 * 1024 * 1024:
        return False
    try:
        decoder = zlib.decompressobj()
        pixels = decoder.decompress(compressed, budget + 1)
    except zlib.error:
        return False
    if len(pixels) != budget or not decoder.eof:
        return False
    for row in range(height):
        start = row * (stride + 1)
        if pixels[start] > 4:
            return False
        color = pixels[start + 1:start + stride + 1]
        if any(color[channel::channels].strip(b"\0") for channel in range(3)):
            return True
    return False


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def capture_binaries(binaries: list[Path], output: Path) -> tuple[list[Path], list[dict]]:
    """Run immutable copies so subsequent builds cannot replace this evidence."""
    destination = output / "binaries"
    destination.mkdir(mode=0o700)
    captured = []
    provenance = []
    for name, source in zip(("game_server", "dreamwake"), binaries, strict=True):
        before = digest(source)
        path = destination / name
        shutil.copy2(source, path)
        after = digest(path)
        if before != after or digest(source) != before:
            raise RuntimeError(f"binary changed during capture: {source.name}")
        path.chmod(0o500)
        captured.append(path)
        provenance.append({"path": str(path), "source_path": str(source), "sha256": after,
                           "bytes": path.stat().st_size})
    return captured, provenance


def reserve_ports() -> list[int]:
    # Hold all three sockets until distinct free ports have been selected. The
    # server still validates its binds; a race causes failure instead of reuse.
    for _ in range(16):
        with ExitStack() as stack:
            sockets = [stack.enter_context(socket.socket(socket.AF_INET, kind))
                       for kind in (socket.SOCK_STREAM, socket.SOCK_DGRAM, socket.SOCK_DGRAM)]
            for sock in sockets:
                sock.bind(("127.0.0.1", 0))
            ports = [sock.getsockname()[1] for sock in sockets]
            if len(set(ports)) == 3:
                return ports
    raise RuntimeError("could not reserve three distinct free ports")


def launch(command: list[str], log: Path, handles: ExitStack) -> subprocess.Popen:
    stream = handles.enter_context(log.open("w", encoding="utf-8"))
    return subprocess.Popen(command, cwd=ROOT, stdout=stream, stderr=subprocess.STDOUT,
                            start_new_session=True)


def latest_trace(path: Path) -> dict | None:
    """Read a bounded tail, ignoring a record still being written by the client."""
    try:
        with path.open("rb") as stream:
            stream.seek(max(0, path.stat().st_size - 256 * 1024))
            lines = stream.read(256 * 1024).splitlines()
        for line in reversed(lines):
            try:
                row = json.loads(line)
                if isinstance(row, dict) and row.get("format") == 1:
                    return row
            except (ValueError, UnicodeDecodeError):
                continue
    except OSError:
        pass
    return None


def recovered_after_pause(before: dict, after: dict | None) -> bool:
    """Require a new active game stream in the same process and server instance."""
    return bool(after and before.get("active") and after.get("active")
        and integer(before.get("connection_epoch"), 1)
        and integer(after.get("connection_epoch"), 1)
        and after["connection_epoch"] != before["connection_epoch"]
        and integer(before.get("pid"), 1) and after.get("pid") == before["pid"]
        and before.get("server_instance") is not None
        and after.get("server_instance") == before["server_instance"]
        and authority_tick(before) is not None and authority_tick(after) is not None
        and authority_tick(after) > authority_tick(before)
        and after.get("elapsed", 0) > before.get("elapsed", 0))


def stop_owned(process: subprocess.Popen) -> list[str]:
    actions = []
    for sig, grace in ((signal.SIGINT, 10), (signal.SIGTERM, 5), (signal.SIGKILL, 2)):
        if process.poll() is not None:
            break
        try:
            # start_new_session made this exact child the process-group leader.
            if os.getpgid(process.pid) != process.pid:
                actions.append("unexpected process group; refused to signal")
                break
            os.killpg(process.pid, sig)
            actions.append(signal.Signals(sig).name)
            process.wait(timeout=grace)
        except subprocess.TimeoutExpired:
            continue
        except ProcessLookupError:
            break
    return actions


def run_profile(name: str, directory: Path, duration: float, seed: int, binaries: list[Path],
                pause_at: float | None = None) -> dict:
    directory.mkdir(mode=0o700)
    http, udp, webrtc = reserve_ports()
    base = f"http://127.0.0.1:{http}"
    profile = PROFILES[name]
    server_command = [str(binaries[0]), "--identity-store", str(directory / "server.bin"),
        "--seed", str(seed), "--replication-distance", "12", "--max-clients", "2",
        "--http-bind", f"127.0.0.1:{http}", "--public-http-base", base,
        "--udp-bind", f"127.0.0.1:{udp}", "--public-udp-addr", f"127.0.0.1:{udp}",
        "--webrtc-bind", f"127.0.0.1:{webrtc}", "--public-webrtc-addr", f"127.0.0.1:{webrtc}",
        "--net-delay-ms", str(profile["delay_ms_per_direction"]),
        "--net-jitter-ms", str(profile["jitter_ms_per_direction"]),
        "--net-loss-percent", str(profile["loss_percent_per_direction"])]
    client_commands = [[str(binaries[1]), "--http-base", base, "--wait-players", "2",
        "--player-id", str(player), "--profile-file", str(directory / f"{side}-profile.json"),
        "--spatial-playtest", side, "--network-trace", str(directory / f"{side}.ndjson"),
        "--capture-dir", str(directory / side), "--quit-after", str(duration)]
        for player, side in zip(PLAYERS, ("left", "right"))]
    result = {"profile": name, "conditioner": profile, "duration": duration, "seed": seed,
              "pause_at": pause_at,
              "server_command": server_command, "client_commands": client_commands,
              "client_pids": [], "client_exit_codes": [], "errors": [], "cleanup": {}}
    processes: list[subprocess.Popen] = []
    paused = False
    pause_done = False
    resume_at = None
    resumed_at = None
    with ExitStack() as handles:
        try:
            server = launch(server_command, directory / "server.log", handles)
            processes.append(server)
            result["server_pid"] = server.pid
            ready_deadline = time.monotonic() + 30
            while True:
                if server.poll() is not None:
                    raise RuntimeError("server exited before bootstrap became ready")
                try:
                    with socket.create_connection(("127.0.0.1", http), timeout=0.2):
                        break
                except OSError:
                    if time.monotonic() >= ready_deadline:
                        raise RuntimeError("bootstrap readiness timed out")
                    time.sleep(0.1)
            clients = []
            for command, side in zip(client_commands, ("left", "right")):
                process = launch(command, directory / f"{side}.log", handles)
                processes.append(process)
                clients.append(process)
                result["client_pids"].append(process.pid)
            save_json(directory / "processes.json", result)
            print(f"{name}: server PID {server.pid}; rendered client PIDs {result['client_pids']}; {directory}", flush=True)
            start = time.monotonic()
            deadline = start + duration + 90
            next_update = start + 15
            while any(process.poll() is None for process in clients):
                now = time.monotonic()
                if server.poll() is not None:
                    raise RuntimeError("server exited while clients were running")
                if now >= deadline:
                    raise RuntimeError("clients exceeded duration plus 90-second startup/exit allowance")
                if any(process.poll() not in (None, 0) for process in clients):
                    raise RuntimeError("a client exited unsuccessfully; inspect its log")
                if pause_at is not None and not pause_done and now - start >= pause_at:
                    before = latest_trace(directory / "left.ndjson")
                    if not before or not before.get("active"):
                        raise RuntimeError("pause requires an active left client")
                    clients[0].send_signal(signal.SIGSTOP)
                    paused = True
                    pause_done = True
                    resume_at = now + 3.0
                    result["pause_recovery"] = {"side": "left", "seconds": 3.0,
                        "stopped_at_process_elapsed": now - start, "before": before,
                        "passed": False}
                    save_json(directory / "processes.json", result)
                    print(f"{name}: left client stopped for 3 seconds", flush=True)
                if paused and now >= resume_at:
                    clients[0].send_signal(signal.SIGCONT)
                    paused = False
                    resumed_at = now
                    result["pause_recovery"]["resumed_at_process_elapsed"] = now - start
                    save_json(directory / "processes.json", result)
                    print(f"{name}: left client resumed; awaiting a fresh active stream", flush=True)
                recovery = result.get("pause_recovery")
                if resumed_at is not None and recovery and not recovery["passed"]:
                    after = latest_trace(directory / "left.ndjson")
                    elapsed = now - resumed_at
                    if recovered_after_pause(recovery["before"], after):
                        recovery.update({"passed": elapsed <= RECOVERY_DEADLINE_SECONDS,
                            "observed_recovery_seconds": elapsed, "after": after})
                        save_json(directory / "processes.json", result)
                        if recovery["passed"]:
                            print(f"{name}: fresh stream recovered in {elapsed:.2f}s", flush=True)
                    if not recovery["passed"] and elapsed > RECOVERY_DEADLINE_SECONDS:
                        recovery["last_observation"] = after
                        raise RuntimeError("paused client did not activate a fresh stream within 10 seconds")
                if now >= next_update:
                    print(f"{name}: {now-start:.0f}s elapsed; client exits {[p.poll() for p in clients]}", flush=True)
                    next_update = now + 15
                time.sleep(0.2)
            result["client_exit_codes"] = [process.returncode for process in clients]
        except (OSError, RuntimeError, KeyboardInterrupt) as error:
            result["errors"].append(str(error) or "interrupted")
        finally:
            if paused and clients[0].poll() is None:
                clients[0].send_signal(signal.SIGCONT)
                result["pause_recovery"]["resumed_during_cleanup"] = True
            for process in reversed(processes):
                result["cleanup"][str(process.pid)] = stop_owned(process)
            if len(processes) == 3:
                result["client_exit_codes"] = [process.poll() for process in processes[1:]]
            if processes:
                result["server_exit_code"] = processes[0].poll()
            save_json(directory / "processes.json", result)
    return result


def frozen_context(args, output: Path):
    """Strict captured-source and paired-binary identity; never consult Git."""
    if not args.no_build or args.binary_dir is None:
        raise ValueError("frozen mode requires --no-build and --binary-dir")
    manifest = args.source_manifest.resolve()
    base = (args.source_root or manifest.parent / "sources").resolve()
    helpers = runpy.run_path(str(ROOT / "scripts/qualify-soak.py"))
    files, manifest_hash = helpers["frozen_sources"](manifest, base, ROOT, output)
    binary_dir = args.binary_dir.resolve()
    provenance_path = (args.binary_provenance or binary_dir.parent / "native-provenance.json").resolve()
    provenance_hash = digest(provenance_path)
    provenance = json.loads(provenance_path.read_text())
    if provenance["source_manifest_sha256"] != manifest_hash:
        raise ValueError("binary provenance belongs to a different source manifest")
    # Historical analysis preserves its original identity and remains read-only
    # with respect to launches, including runs made before release was required.
    if not args.analyze:
        NATIVE_HELPERS["require_optimized_provenance"](provenance)
    binary_hashes = {}
    for name in ("game_server", "dreamwake"):
        path = binary_dir / name
        if path.is_symlink() or not path.is_file() or not os.access(path, os.X_OK):
            raise ValueError(f"ordinary executable missing: {path}")
        binary_hashes[name] = digest(path)
        if binary_hashes[name] != provenance["artifacts"][name]["sha256"]:
            raise ValueError(f"binary hash mismatch: {name}")
    identity = {"source_manifest": str(manifest), "source_manifest_sha256": manifest_hash,
                "source_root": str(base), "source_files": len(files),
                "binary_provenance": str(provenance_path), "binary_provenance_sha256": provenance_hash,
                "binary_dir": str(binary_dir), "binary_sha256": binary_hashes}
    def unchanged():
        try:
            return (digest(manifest) == manifest_hash and helpers["unchanged"](base,files)
                    and digest(provenance_path) == provenance_hash
                    and all(not (binary_dir/name).is_symlink() and digest(binary_dir/name) == value
                            for name,value in binary_hashes.items()))
        except OSError:
            return False
    return identity, unchanged


def input_hashes(output: Path, profiles: list[str]) -> dict[str,str]:
    paths = [output / "manifest.json"]
    for name in profiles:
        if name not in PROFILES:
            raise ValueError("unknown profile in run manifest")
        folder = output / name
        paths += [folder / "processes.json", folder / "left.ndjson", folder / "right.ndjson"]
        paths += list(folder.glob("*/*.png"))
    return {str(path.relative_to(output)): digest(path) for path in paths if path.is_file()}


def captured_binaries_unchanged(manifest: dict, output: Path) -> bool:
    try:
        if ("native_provenance_sha256" in manifest
                and digest(output / "native-provenance.json") != manifest["native_provenance_sha256"]):
            return False
        entries = manifest["binaries"]
        if len(entries) != 2:
            return False
        for item,name in zip(entries,("game_server","dreamwake")):
            path = output / "binaries" / name
            if (path.is_symlink() or not path.is_file() or str(path) != item["path"]
                    or digest(path) != item["sha256"]):
                return False
        return True
    except (OSError,KeyError,TypeError):
        return False


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--profiles", nargs="+", type=str.lower, choices=PROFILES, default=["lan"])
    parser.add_argument("--duration", type=float, default=120, help="client lifetime in seconds, 30–600 (default 120)")
    parser.add_argument("--seed", type=int, default=8192)
    parser.add_argument("--pause-at", type=float,
        help="stop the left client for 3 seconds at this process elapsed time; require fresh-stream recovery within 10 seconds")
    parser.add_argument("--no-build", action="store_true", help="reuse a captured pair with --binary-dir; target/release fallback requires target/native-provenance.json")
    parser.add_argument("--binary-dir", type=Path, help="captured release binaries with sibling native-provenance.json; requires --no-build")
    parser.add_argument("--output-root", type=Path, default=ROOT / "out" / "spatial-playtest")
    parser.add_argument("--analyze", type=Path, help="reanalyze an existing run without launching processes")
    parser.add_argument("--source-manifest", type=Path, help="strict frozen mode; no enclosing Git discovery")
    parser.add_argument("--source-root", type=Path, help="defaults to manifest sibling sources")
    parser.add_argument("--binary-provenance", type=Path, help="defaults to binary-dir sibling native-provenance.json")
    args = parser.parse_args()
    if args.source_root and not args.source_manifest:
        parser.error("--source-root requires --source-manifest")
    if args.binary_provenance and not args.no_build:
        parser.error("--binary-provenance requires --no-build")
    frozen = None
    analysis_inputs = None
    collected_inputs = {}
    if args.source_manifest:
        try:
            frozen = frozen_context(args, (args.analyze or args.output_root).resolve())
        except (OSError,ValueError,KeyError,TypeError) as error:
            parser.error(str(error))
    if args.analyze:
        output = args.analyze.resolve()
        if not frozen and not output.is_relative_to(ROOT / "out"):
            parser.error("--analyze must stay under this repository's ignored out/ directory")
        run_manifest_hash = digest(output / "manifest.json")
        manifest = json.loads((output / "manifest.json").read_text())
        if frozen and manifest.get("frozen") != frozen[0]:
            parser.error("analysis requires the original frozen source and binary identity")
        analysis_inputs = input_hashes(output, manifest["profiles"])
        reports = {name: analyze_profile(output / name,
                   json.loads((output / name / "processes.json").read_text())) for name in manifest["profiles"]}
    else:
        if os.name != "posix":
            parser.error("owned process-group cleanup currently requires a POSIX host")
        if not math.isfinite(args.duration) or not 30 <= args.duration <= 600:
            parser.error("--duration must be between 30 and 600 seconds")
        if not 0 <= args.seed < 1 << 64:
            parser.error("--seed must fit u64")
        if args.pause_at is not None and (not math.isfinite(args.pause_at)
                or not 15 <= args.pause_at <= args.duration - 20):
            parser.error("--pause-at must allow 15 seconds for activation and 20 seconds before run end")
        output_root = args.output_root.resolve()
        if not frozen and not output_root.is_relative_to(ROOT / "out"):
            parser.error("--output-root must stay under this repository's ignored out/ directory")
        output_root.mkdir(parents=True, exist_ok=True)
        output = Path(tempfile.mkdtemp(prefix=datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ-"), dir=output_root))
        if args.binary_dir is not None and not args.no_build:
            parser.error("--binary-dir requires --no-build; captured runs cannot rebuild source")
        build_command = NATIVE_HELPERS["build_command"]()
        try:
            if args.no_build:
                binary_dir = args.binary_dir.resolve() if args.binary_dir is not None else ROOT / "target" / "release"
                binaries = [binary_dir / name for name in ("game_server", "dreamwake")]
                provenance_path = (args.binary_provenance or binary_dir.parent / "native-provenance.json").resolve()
                if not provenance_path.is_file():
                    raise ValueError(f"--no-build requires native provenance at {provenance_path}; use --binary-dir RUN/binaries from a captured release run")
                native_build = json.loads(provenance_path.read_text())
                NATIVE_HELPERS["require_optimized_provenance"](native_build)
                for binary in binaries:
                    if (binary.is_symlink() or not binary.is_file() or not os.access(binary, os.X_OK)
                            or digest(binary) != native_build["artifacts"][binary.name]["sha256"]):
                        raise ValueError(f"binary differs from native provenance: {binary}")
            else:
                print(f"Building paired release native binaries once; {output / 'build.log'}", flush=True)
                native_build = NATIVE_HELPERS["build_pair"](ROOT, output)
                binaries = [Path(native_build["artifacts"][name]["source_path"]) for name in ("game_server", "dreamwake")]
        except (OSError, ValueError, KeyError, TypeError) as error:
            parser.error(str(error))
        binaries, binary_provenance = capture_binaries(binaries, output)
        for name, item in zip(("game_server", "dreamwake"), binary_provenance):
            if item["sha256"] != native_build["artifacts"][name]["sha256"]:
                raise RuntimeError("captured binaries differ from native provenance")
            item["profile"] = native_build["artifacts"][name]["profile"]
            native_build["artifacts"][name]["path"] = f"binaries/{name}"
        if frozen and not frozen[1]():
            raise RuntimeError("frozen inputs changed during binary capture")
        native_build.update(format=2, status="complete")
        save_json(output / "native-provenance.json", native_build)
        (output / "native-provenance.json").chmod(0o400)
        manifest = {"format": 1, "platform": platform.platform(), "python": sys.version,
            "git_head": None if frozen else subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
            "frozen": frozen[0] if frozen else None,
            "profiles": list(dict.fromkeys(args.profiles)), "duration": args.duration,
            "pause_at": args.pause_at,
            "build_command": None if args.no_build else build_command,
            "native_build": native_build,
            "native_provenance_sha256": digest(output / "native-provenance.json"),
            "binaries": binary_provenance,
            "captured_at_utc": datetime.now(timezone.utc).isoformat(),
            "scope": "Two native rendered local processes; not human, browser, external-hardware, or full-matrix qualification."}
        save_json(output / "manifest.json", manifest)
        run_manifest_hash = digest(output / "manifest.json")
        if frozen and not frozen[1]():
            raise RuntimeError("frozen inputs changed before process launch")
        print(f"Captured paired binaries and provenance; {output / 'manifest.json'}", flush=True)
        reports = {}
        for name in manifest["profiles"]:
            process_run = run_profile(name, output / name, args.duration, args.seed, binaries, args.pause_at)
            before_analysis = input_hashes(output, [name])
            collected_inputs.update(before_analysis)
            reports[name] = analyze_profile(output / name, process_run)
            if before_analysis != input_hashes(output, [name]):
                reports[name]["passed"] = False
                reports[name].setdefault("failures", []).append("analysis inputs changed during read")
            print(f"{name}: {'PASS' if reports[name]['passed'] else 'FAIL'} — {output / name / 'analysis.json'}", flush=True)
            if process_run["errors"]:
                break
    stable = (digest(output / "manifest.json") == run_manifest_hash
              and captured_binaries_unchanged(manifest, output) and (not frozen or frozen[1]()))
    if collected_inputs:
        stable = stable and collected_inputs == input_hashes(output, list(reports))
    if analysis_inputs is not None:
        stable = stable and analysis_inputs == input_hashes(output, manifest["profiles"])
    result = {"passed": stable and bool(reports) and all(report["passed"] for report in reports.values()),
              "inputs_unchanged_after": stable,
              "source_manifest_sha256": frozen[0]["source_manifest_sha256"] if frozen else None,
              "profiles": reports, "output": str(output)}
    save_json(output / "summary.json", result)
    print(f"{'PASS' if result['passed'] else 'FAIL'}: {output / 'summary.json'}", flush=True)
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
