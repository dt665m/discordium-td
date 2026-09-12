#!/usr/bin/env python3
"""Prepare or manually run a frozen production-clock fixture. No live-source capture.

Simulated time is a regression only; actual-hour/day verdicts require real elapsed
runtime, immutable sources/binary, complete telemetry and bounded resident memory.
"""
from __future__ import annotations
import argparse
import json
import math
import os
from pathlib import Path
import platform
import runpy
import signal
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
HELPERS = runpy.run_path(str(ROOT / "scripts/qualify-soak.py"))


def terminate(process: subprocess.Popen) -> None:
    """Stop the complete build/run process group, including child compiler jobs."""
    if process.poll() is not None:
        return
    os.killpg(process.pid, signal.SIGTERM)
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()


class Continuity:
    """Reject suspension or clock jumps rather than extending a successful run."""
    def __init__(self, monotonic: float, independent: float, maximum_gap: float, tolerance: float = 1.0):
        if not all(math.isfinite(value) for value in (monotonic, independent, maximum_gap, tolerance)) or maximum_gap <= 0 or tolerance < 0:
            raise ValueError("invalid continuity clock or budget")
        self.start_mono = self.last_mono = monotonic
        self.start_independent = self.last_independent = independent
        self.maximum_gap = maximum_gap
        self.tolerance = tolerance
        self.samples = 0
        self.maximum_divergence = self.maximum_delta_divergence = 0.0

    def observe(self, monotonic: float, independent: float) -> None:
        if not math.isfinite(monotonic) or not math.isfinite(independent):
            raise RuntimeError("clock continuity failed: nonfinite reference clock")
        mono_delta = monotonic - self.last_mono
        independent_delta = independent - self.last_independent
        divergence = abs((independent - self.start_independent) - (monotonic - self.start_mono))
        delta_divergence = abs(independent_delta - mono_delta)
        self.samples += 1
        self.maximum_divergence = max(self.maximum_divergence, divergence)
        self.maximum_delta_divergence = max(self.maximum_delta_divergence, delta_divergence)
        if mono_delta < 0 or independent_delta < 0:
            raise RuntimeError("clock continuity failed: backward reference clock")
        if divergence > self.tolerance or delta_divergence > self.tolerance:
            raise RuntimeError("clock continuity failed: suspension or independent-clock jump")
        if max(mono_delta, independent_delta) > self.maximum_gap:
            raise RuntimeError("clock continuity failed: wrapper polling gap")
        self.last_mono, self.last_independent = monotonic, independent

    def result(self) -> dict:
        return {"samples": self.samples, "tolerance_seconds": self.tolerance,
                "maximum_poll_gap_seconds": self.maximum_gap,
                "maximum_elapsed_divergence_seconds": self.maximum_divergence,
                "maximum_delta_divergence_seconds": self.maximum_delta_divergence}


def rss_bytes(pid: int) -> int | None:
    result = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], text=True,
                            capture_output=True, timeout=2)
    return int(result.stdout.strip()) * 1024 if result.returncode == 0 and result.stdout.strip() else None


def final_metrics(path: Path, maximum_rows: int) -> dict:
    final = None
    count = 0
    with path.open() as source:
        while line := source.readline(16_385):
            count += 1
            if len(line) > 16_384 or not line.endswith("\n") or count > maximum_rows:
                raise ValueError("clock telemetry exceeded row/byte bound")
            row = json.loads(line)
            if final is not None:
                raise ValueError("telemetry continued after final record")
            if row.get("phase") not in ("interval", "final"):
                raise ValueError("unknown clock telemetry phase")
            if row["phase"] == "final":
                final = row
    if final is None:
        raise ValueError("clock telemetry has no final record")
    return final


def verdict(final: dict, mode: str, seconds: int, elapsed: float, stable: bool,
            code: int, interrupted: bool, rss: dict, max_growth: int) -> dict:
    """Independent wrapper checks prevent elapsed-time or arithmetic-only claims."""
    complete = (code == 0 and not interrupted and stable and final.get("completed") is True
                and final.get("mode") == mode and final.get("requested_seconds") == seconds
                and final.get("reference_seconds") == seconds and final.get("ticks") == seconds * 60
                and final.get("error") is None and final.get("max_debt_ticks") == 0
                and final.get("max_poll_gap_ns", 10**30) <= 100_000_000)
    drifts = final.get("drifts", [])
    both = len(drifts) == 2 and {item.get("ppm") for item in drifts} == {-1000, 1000}
    phase = both and all(item.get("max_samples", 1000) <= 32
                        and item.get("max_target_phase_ticks", 1000) <= 2
                        and (seconds <= 10 or (item.get("after_warmup_phase", {}).get("samples", 0) > 0
                             and item["after_warmup_phase"].get("max_abs_ns", 10**30) * 60 <= 1_000_000_000))
                        for item in drifts)
    complete = bool(complete and phase)
    memory = (rss.get("samples", 0) >= 2 and rss.get("first_bytes") is not None
              and rss.get("last_bytes") is not None
              and rss["last_bytes"] - rss["first_bytes"] <= max_growth)
    actual = bool(complete and mode == "realtime" and elapsed >= seconds
                  and final.get("actual_elapsed_seconds", 0) >= seconds and memory)
    return {"requested_run_completed": complete,
            "simulated_regression_passed": complete and mode == "simulated",
            "actual_duration_completed": actual,
            "resident_growth_within_budget": bool(memory),
            "T004_actual_hour_passed": actual and seconds >= 3600 and final.get("T004_actual_hour_passed") is True,
            "T007_actual_day_passed": actual and seconds >= 86400 and final.get("T007_actual_day_passed") is True,
            "integrated_game_transport_24h_passed": False}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-manifest", required=True, type=Path)
    parser.add_argument("--source-root", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--mode", required=True, choices=["simulated", "realtime"])
    parser.add_argument("--seconds", required=True, type=int)
    parser.add_argument("--report-seconds", default=60, type=int)
    parser.add_argument("--prepare-only", action="store_true", help="validate frozen inputs and write commands only; no build or run")
    parser.add_argument("--build-timeout-seconds", type=int, default=900)
    parser.add_argument("--runtime-timeout-seconds", type=int, help="default: requested duration+60 real, 300 simulated")
    parser.add_argument("--rss-sample-seconds", type=float, default=5)
    parser.add_argument("--max-rss-mib", type=int, default=256)
    parser.add_argument("--max-rss-growth-mib", type=int, default=16)
    args = parser.parse_args()
    timeout = args.runtime_timeout_seconds or (args.seconds + 60 if args.mode == "realtime" else 300)
    if not (1 <= args.seconds <= 86400 and 1 <= args.report_seconds <= 3600
            and (args.seconds + args.report_seconds - 1) // args.report_seconds <= 1440
            and 1 <= args.build_timeout_seconds <= 3600 and 1 <= timeout <= 90000
            and (args.mode != "realtime" or timeout >= args.seconds)
            and 1 <= args.rss_sample_seconds <= 60 and 16 <= args.max_rss_mib <= 4096
            and 0 <= args.max_rss_growth_mib <= args.max_rss_mib):
        parser.error("invalid duration, report, timeout or memory budget")
    output = args.output.resolve()
    if output.exists() and any(output.iterdir()):
        parser.error("output must be new or empty")
    manifest = args.source_manifest.resolve()
    base = (args.source_root or manifest.parent / "sources").resolve()
    files, manifest_hash = HELPERS["frozen_sources"](manifest, base, ROOT, output)
    output.mkdir(parents=True, exist_ok=True)
    target = output / "build"
    binary = target / "release/examples/clock_qualification"
    build = ["cargo", "build", "--release", "--locked", "--offline", "-p", "engine_net", "--example", "clock_qualification"]
    command = [str(binary), "--mode", args.mode, "--seconds", str(args.seconds),
               "--report-seconds", str(args.report_seconds), "--output", str(output / "metrics.jsonl")]
    metadata = {"kind": "production_clock_injected_drift", "status": "prepared",
                "mode": args.mode, "requested_seconds": args.seconds,
                "created_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                "machine": platform.platform(), "source_manifest": str(manifest),
                "source_manifest_sha256": manifest_hash, "workspace": str(ROOT),
                "source_root": str(base), "build_command": build, "command": command,
                "env": {"CARGO_INCREMENTAL": "0", "CARGO_TARGET_DIR": str(target)},
                "rustflags": os.environ.get("RUSTFLAGS", ""),
                "cargo_encoded_rustflags": os.environ.get("CARGO_ENCODED_RUSTFLAGS", ""),
                "build_timeout_seconds": args.build_timeout_seconds, "runtime_timeout_seconds": timeout,
                "profile": {"drift_ppm": [-1000, 1000], "tick_hz": 60, "poll_ms": 10,
                            "probe_ms": 250, "uplink_ms": 10, "downlink_ms": 10,
                            "processing_ms": 2, "initial_local_offset_seconds": 5,
                            "warmup_seconds": 10, "phase_bound_ticks": 1, "target_phase_bound_ticks": 2,
                            "maximum_poll_gap_ms": 100, "static_lead_ticks": 2,
                            "scope": "proposed symmetric-path fixture profile; no universal network bound"},
                "rss_sample_seconds": args.rss_sample_seconds, "max_rss_mib": args.max_rss_mib,
                "max_rss_growth_mib": args.max_rss_growth_mib,
                "T004_actual_hour_passed": False, "T007_actual_day_passed": False,
                "integrated_game_transport_24h_passed": False}
    write = lambda: HELPERS["write_json"](output / "report.json", metadata)
    write()
    if args.prepare_only:
        print(f"Prepared only; no build or run: {output / 'report.json'}")
        return 0
    stable = lambda: (HELPERS["digest"](manifest) == manifest_hash and HELPERS["unchanged"](base, files))
    env = dict(os.environ, **metadata["env"])
    process = None
    try:
        metadata["rustc"] = subprocess.check_output(["rustc", "-vV"], text=True, timeout=30).strip()
        metadata["status"] = "building"
        write()
        with (output / "build.log").open("w") as log:
            process = subprocess.Popen(build, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            code = process.wait(timeout=args.build_timeout_seconds)
        if code != 0 or not stable():
            raise RuntimeError("build failed or frozen sources changed during build")
        binary_hash = HELPERS["digest"](binary)
        metadata["binary_sha256"] = binary_hash
        metadata["status"] = "running"
        write()
        rss = HELPERS["Trend"]()
        started = time.monotonic()
        if hasattr(time, "CLOCK_BOOTTIME"):
            reference = lambda: time.clock_gettime(time.CLOCK_BOOTTIME)
            metadata["independent_clock"] = "CLOCK_BOOTTIME (sleep-inclusive)"
        else:
            reference = time.time
            metadata["independent_clock"] = "time.time fallback (wall-clock jumps also invalidate)"
        continuity = Continuity(started, reference(), args.rss_sample_seconds + 3)
        write()
        next_report = args.report_seconds
        with (output / "run.log").open("w") as log, (output / "rss.jsonl").open("w") as memory:
            process = subprocess.Popen(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            while process.poll() is None:
                monotonic = time.monotonic()
                continuity.observe(monotonic, reference())
                elapsed = monotonic - started
                if elapsed > timeout:
                    raise TimeoutError("runtime watchdog expired")
                resident = rss_bytes(process.pid)
                if resident is not None:
                    rss.add(elapsed, resident)
                    if resident > args.max_rss_mib * 1024**2:
                        raise RuntimeError("resident memory cap exceeded")
                elif args.mode == "realtime" and process.poll() is None:
                    raise RuntimeError("resident memory measurement unavailable")
                if elapsed >= next_report:
                    memory.write(json.dumps({"elapsed_seconds": elapsed, **rss.result()}) + "\n")
                    memory.flush()
                    next_report += args.report_seconds
                time.sleep(min(args.rss_sample_seconds, max(0.01, timeout - elapsed)))
            monotonic = time.monotonic()
            continuity.observe(monotonic, reference())
            elapsed = monotonic - started
            memory.write(json.dumps({"elapsed_seconds": elapsed, "phase": "final", **rss.result()}) + "\n")
        final = final_metrics(output / "metrics.jsonl", args.seconds // args.report_seconds + 1)
        integrity = stable() and HELPERS["digest"](binary) == binary_hash
        metadata.update({"status": "finished", "exit_code": process.returncode, "elapsed_seconds": elapsed,
                         "source_and_binary_unchanged_after": integrity, "final_metrics": final,
                         "resident_memory": rss.result(), "clock_continuity": continuity.result()})
        metadata.update(verdict(final, args.mode, args.seconds, elapsed, integrity, process.returncode,
                                False, rss.result(), args.max_rss_growth_mib * 1024**2))
        write()
        return 0 if metadata["requested_run_completed"] and (args.mode == "simulated" or metadata["actual_duration_completed"]) else 1
    except (Exception, KeyboardInterrupt) as error:
        if process is not None:
            terminate(process)
        if "started" in locals():
            metadata["elapsed_seconds"] = time.monotonic() - started
        if "rss" in locals():
            metadata["resident_memory"] = rss.result()
        if "continuity" in locals():
            metadata["clock_continuity"] = continuity.result()
        metadata.update({"status": "incomplete", "error": str(error), "T004_actual_hour_passed": False,
                         "T007_actual_day_passed": False})
        try:
            metadata["source_unchanged_after"] = stable()
            if "binary_sha256" in metadata:
                metadata["binary_unchanged_after"] = HELPERS["digest"](binary) == metadata["binary_sha256"]
        except OSError:
            metadata["source_and_binary_unchanged_after"] = False
        write()
        print(f"Incomplete: {error}; {output / 'report.json'}")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
