#!/usr/bin/env python3
"""Capture immutable sources, then run the persistent generic 50k/100-peer wall-clock soak.

A short run validates the harness only. Even an actual 24-hour generic pass does
not qualify Dreamwake bots, sockets, browser clients, or reference hardware.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import runpy
import shutil
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]


def write_json(path: Path, value: object) -> None:
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2) + "\n")
    temporary.replace(path)


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for data in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(data)
    return value.hexdigest()


def capture_sources(output: Path, helpers: dict) -> tuple[Path, list[dict], Path]:
    repositories = helpers["local_sources"]()
    common = Path(os.path.commonpath([repo["root"] for repo in repositories]))
    # Preserve relative path dependencies across all local repositories, without .git or build output.
    if any(Path(repo["root"]) == common for repo in repositories):
        common = common.parent
    base = output / "sources"
    files = []
    for repo in repositories:
        root = Path(repo["root"])
        names = subprocess.check_output(["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], cwd=root).split(b"\0")
        for name in sorted(set(filter(None, names))):
            source = root / os.fsdecode(name)
            if not source.is_file():
                continue
            if source.is_symlink():
                raise ValueError(f"source capture requires ordinary files: {source}")
            target = base / root.relative_to(common) / os.fsdecode(name)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, target)
            files.append({"path": str(target.relative_to(base)), "sha256": digest(target), "bytes": target.stat().st_size, "executable": bool(target.stat().st_mode & 0o111)})
    manifest = output / "source-manifest.json"
    write_json(manifest, {"repositories_at_capture_start": repositories, "files": files})
    # Cargo writes only to the separately selected build directory; source bytes are read-only.
    for entry in files:
        (base / entry["path"]).chmod(0o555 if entry["executable"] else 0o444)
    for directory, _, _ in os.walk(base, topdown=False):
        Path(directory).chmod(0o555)
    return base / ROOT.relative_to(common), files, manifest


def unchanged(base: Path, files: list[dict]) -> bool:
    try:
        seen = set()
        for entry in files:
            relative = Path(entry["path"])
            if relative.is_absolute() or ".." in relative.parts or not relative.parts or str(relative) in seen:
                return False
            seen.add(str(relative))
            path = base / relative
            if any((base / Path(*relative.parts[:i])).is_symlink() for i in range(1, len(relative.parts) + 1)):
                return False
            if not path.is_file() or digest(path) != entry["sha256"]:
                return False
        actual = {str(path.relative_to(base)) for path in base.rglob("*") if path.is_file() or path.is_symlink()}
        return bool(files) and actual == seen
    except (OSError, KeyError, TypeError):
        return False


def frozen_sources(manifest: Path, base: Path, workspace: Path, output: Path) -> tuple[list[dict], str]:
    """Validate a captured tree without consulting an enclosing Git checkout."""
    base, workspace, output = base.resolve(), workspace.resolve(), output.resolve()
    if output == base or base in output.parents:
        raise ValueError("output must be outside the immutable source tree")
    if workspace != base and base not in workspace.parents:
        raise ValueError("workspace must be inside the manifest source root")
    manifest_hash = digest(manifest)
    files = json.loads(manifest.read_text())["files"]
    if not unchanged(base, files):
        raise ValueError("frozen manifest contains unsafe, missing or changed source files")
    required = str((workspace / "Cargo.toml").relative_to(base))
    if required not in {entry["path"] for entry in files}:
        raise ValueError("manifest does not include this workspace Cargo.toml")
    return files, manifest_hash


class Trend:
    """Constant-memory online linear RSS trend; hours retain only aggregate samples."""
    def __init__(self) -> None:
        self.n = 0
        self.mean_x = self.mean_y = self.sxx = self.sxy = 0.0
        self.minimum = self.maximum = self.first = self.last = None

    def add(self, seconds: float, rss: int) -> None:
        self.n += 1
        dx, dy = seconds - self.mean_x, rss - self.mean_y
        self.mean_x += dx / self.n
        self.mean_y += dy / self.n
        self.sxx += dx * (seconds - self.mean_x)
        self.sxy += dx * (rss - self.mean_y)
        self.first = rss if self.first is None else self.first
        self.last = rss
        self.minimum = rss if self.minimum is None else min(self.minimum, rss)
        self.maximum = rss if self.maximum is None else max(self.maximum, rss)

    def result(self) -> dict:
        return {"samples": self.n, "first_bytes": self.first, "last_bytes": self.last,
                "min_bytes": self.minimum, "max_bytes": self.maximum,
                "mean_bytes": self.mean_y if self.n else None,
                "linear_bytes_per_hour": self.sxy / self.sxx * 3600 if self.sxx else None}


def rss_bytes(pid: int) -> int | None:
    result = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], text=True, capture_output=True)
    return int(result.stdout.strip()) * 1024 if result.returncode == 0 and result.stdout.strip() else None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--source-manifest", type=Path, help="reuse this verified capture, without Git discovery")
    parser.add_argument("--source-root", type=Path, help="captured tree root; defaults to manifest sibling sources")
    parser.add_argument("--seconds", type=int, default=86400)
    parser.add_argument("--report-seconds", type=int, default=3600)
    parser.add_argument("--rss-sample-seconds", type=float, default=5)
    parser.add_argument("--warmup-seconds", type=float, default=300)
    parser.add_argument("--max-rss-mib", type=float, default=1024)
    parser.add_argument("--max-rss-growth-mib", type=float, default=32)
    parser.add_argument("--max-rss-trend-mib-per-hour", type=float, default=1)
    parser.add_argument("--udp-preflight", action="store_true", help="run existing real UDP activation test once; not a bot soak")
    args = parser.parse_args()
    if not (1 <= args.seconds <= 604800 and 1 <= args.report_seconds <= 3600 and 0.1 <= args.rss_sample_seconds <= 60
            and 0 <= args.warmup_seconds < args.seconds and 64 <= args.max_rss_mib <= 8192
            and 0 <= args.max_rss_growth_mib <= 8192 and 0 <= args.max_rss_trend_mib_per_hour <= 1024
            and (args.seconds + args.report_seconds - 1) // args.report_seconds <= 1024):
        parser.error("invalid duration, report, sampling, warmup, or RSS budget")
    output = args.output.resolve()
    if output.exists() and any(output.iterdir()):
        parser.error("output must be new or empty")
    output.mkdir(parents=True, exist_ok=True)
    helpers = runpy.run_path(str(ROOT / "scripts/qualify-network.py"))
    if args.source_root and not args.source_manifest:
        parser.error("--source-root requires --source-manifest")
    if args.source_manifest:
        manifest = args.source_manifest.resolve()
        source_base = (args.source_root or manifest.parent / "sources").resolve()
        files, manifest_hash = frozen_sources(manifest, source_base, ROOT, output)
        captured = ROOT
        shutil.copy2(manifest, output / "source-manifest.json")
    else:
        print("Capturing local dependency sources", flush=True)
        captured, files, manifest = capture_sources(output, helpers)
        source_base = output / "sources"
        manifest_hash = digest(manifest)
    env = dict(os.environ, CARGO_TARGET_DIR=str(output / "build"))
    provenance = {"kind": "persistent_generic_network_wall_clock_soak", "machine": helpers["machine"](),
                  "source_manifest_sha256": manifest_hash, "captured_workspace": str(captured),
                  "configuration": {key: str(value) if isinstance(value, Path) else value for key, value in vars(args).items()},
                  "population": {"entities": 50000, "peers": 100, "target_hz": 30},
                  "build_environment": {key: value for key, value in env.items() if key in {
                      "RUSTFLAGS", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "RUSTUP_TOOLCHAIN",
                      "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET", "CARGO_BUILD_RUSTFLAGS"
                  } or key.startswith("CARGO_PROFILE_RELEASE_")},
                  "transport": "bounded synthetic packet loss/reorder; no socket or game simulation",
                  "source_immutable_before": unchanged(source_base, files) and digest(manifest) == manifest_hash,
                  "wall_clock_24h_passed": False, "production_game_or_transport_soak_passed": False,
                  "udp_preflight": {"requested": args.udp_preflight, "is_bot_soak": False}}
    write_json(output / "report.json", provenance)
    command = ["cargo", "build", "--release", "--locked", "--offline", "-p", "engine_net", "--example", "scope_soak"]
    with (output / "build.log").open("w") as log:
        result = subprocess.run(command, cwd=captured, env=env, stdout=log, stderr=subprocess.STDOUT)
    provenance["build"] = {"command": command, "exit_code": result.returncode}
    if result.returncode:
        write_json(output / "report.json", provenance)
        print(f"Build failed: {output / 'build.log'}", file=sys.stderr)
        return 1
    binary = output / "scope_soak"
    shutil.copy2(output / "build/release/examples/scope_soak", binary)
    binary.chmod(0o555)
    provenance["binary_sha256"] = digest(binary)
    if args.udp_preflight:
        command = ["cargo", "test", "--release", "--locked", "--offline", "-p", "dreamwake_server",
                   "production_udp_uses_two_stage_activation_and_distinct_command_finalization", "--", "--nocapture"]
        with (output / "udp-preflight.log").open("w") as log:
            result = subprocess.run(command, cwd=captured, env=env, stdout=log, stderr=subprocess.STDOUT)
        provenance["udp_preflight"].update({"command": command, "exit_code": result.returncode})
        if result.returncode:
            write_json(output / "report.json", provenance)
            return 1
    provenance["source_immutable_before_run"] = unchanged(source_base, files) and digest(manifest) == manifest_hash
    if not provenance["source_immutable_before_run"]:
        write_json(output / "report.json", provenance)
        return 1
    write_json(output / "report.json", provenance)
    command = [str(binary), str(args.seconds), str(args.report_seconds), str(output / "metrics.jsonl")]
    print(f"Starting {args.seconds}s real-wall-clock soak; report: {output / 'report.json'}", flush=True)
    started = time.monotonic()
    trend, all_rss, interval = Trend(), Trend(), Trend()
    next_report = args.report_seconds
    interrupted = False
    deadline_exceeded = False
    rss_limit_exceeded = False
    def interrupted_signal(_signum: int, _frame: object) -> None:
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted_signal)
    with (output / "stderr.log").open("w") as log, (output / "rss.jsonl").open("w") as rss_log:
        process = subprocess.Popen(command, cwd=captured, env=env, stdout=log, stderr=subprocess.STDOUT)
        provenance.update({"status": "running", "pid": process.pid, "started_at_unix_seconds": time.time()})
        write_json(output / "report.json", provenance)
        try:
            while process.poll() is None:
                elapsed = time.monotonic() - started
                if elapsed > args.seconds + 120:
                    deadline_exceeded = True
                    process.terminate()
                    break
                rss = rss_bytes(process.pid)
                if rss is not None:
                    all_rss.add(elapsed, rss)
                    interval.add(elapsed, rss)
                    if elapsed >= args.warmup_seconds:
                        trend.add(elapsed, rss)
                    if rss > args.max_rss_mib * 1024**2:
                        rss_limit_exceeded = True
                        process.terminate()
                        break
                if elapsed >= next_report:
                    record = {"elapsed_seconds": elapsed, "interval": interval.result(), "after_warmup": trend.result()}
                    rss_log.write(json.dumps(record) + "\n")
                    rss_log.flush()
                    interval = Trend()
                    next_report += args.report_seconds
                    print(f"Soak elapsed {elapsed:.1f}s; RSS {rss if rss is not None else 'unavailable'} bytes", flush=True)
                time.sleep(args.rss_sample_seconds)
        except KeyboardInterrupt:
            interrupted = True
            process.terminate()
        try:
            code = process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            code = process.wait()
        elapsed = time.monotonic() - started
        rss_log.write(json.dumps({"elapsed_seconds": elapsed, "phase": "final", "interval": interval.result(), "after_warmup": trend.result()}) + "\n")
    final = None
    metrics_error = None
    metrics_path = output / "metrics.jsonl"
    if metrics_path.exists():
        try:
            with metrics_path.open() as stream:
                for line in stream:
                    row = json.loads(line)
                    final = row if row.get("phase") == "final" else None
        except (OSError, ValueError) as error:
            metrics_error = str(error)
            final = None
    stable = unchanged(source_base, files) and digest(manifest) == manifest_hash and digest(binary) == provenance["binary_sha256"]
    rss_trend = trend.result()
    growth = None if trend.first is None else trend.last - trend.first
    trend_ok = (trend.n >= 2 and growth <= args.max_rss_growth_mib * 1024**2
                and rss_trend["linear_bytes_per_hour"] <= args.max_rss_trend_mib_per_hour * 1024**2)
    coverage = bool(final and all(final.get(name, 0) > 0 for name in ["scope_exits", "destroyed_generations", "dormant_offers",
                        "dropped", "reordered", "groups_published", "partial_groups", "baseline_retirements", "event_records", "full_history_windows", "retired_history_checks"]))
    actual_24h = bool(final and final["elapsed_seconds"] >= 86400 and elapsed >= 86400 and args.seconds >= 86400)
    tick_rate_ok = bool(final and final["ticks"] / final["elapsed_seconds"] >= 27)
    complete = bool(code == 0 and not interrupted and not deadline_exceeded and final and final["elapsed_seconds"] >= args.seconds and stable and not rss_limit_exceeded)
    provenance.update({"status": "finished", "command": command, "exit_code": code, "interrupted": interrupted, "deadline_exceeded": deadline_exceeded, "elapsed_seconds": elapsed,
                       "source_and_binary_unchanged_after": stable, "final_metrics": final, "metrics_error": metrics_error,
                       "rss_all": all_rss.result(), "rss_after_warmup": rss_trend, "rss_growth_bytes_after_warmup": growth,
                       "rss_limit_exceeded": rss_limit_exceeded, "rss_trend_within_budget": trend_ok,
                       "required_workload_paths_exercised": coverage, "target_tick_rate_within_10_percent": tick_rate_ok, "requested_run_completed": complete,
                       "actual_24h_elapsed": actual_24h,
                       "wall_clock_24h_passed": bool(complete and actual_24h and trend_ok and coverage and tick_rate_ok),
                       "qualification_scope": "Generic production components under synthetic impairment only; game bot and transport soak remain separate."})
    write_json(output / "report.json", provenance)
    print(f"Requested run completed={complete}; actual24h={actual_24h}; RSS trend within budget={trend_ok}; report: {output / 'report.json'}")
    return 0 if complete and coverage else 1


if __name__ == "__main__":
    raise SystemExit(main())
