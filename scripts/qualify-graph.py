#!/usr/bin/env python3
"""Run the persistent graph workload and summarize observed current-host evidence.

The Rust executable owns the fixture, independent oracle, codecs and assertions.
This runner only captures provenance and summarizes its raw CSV measurements.
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import os
import platform
import runpy
from pathlib import Path
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]


def capture(*args: str) -> str | None:
    result = subprocess.run(args, cwd=ROOT, text=True, capture_output=True)
    return result.stdout.strip() if result.returncode == 0 else None


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--source-manifest", type=Path, help="verify complete captured sources without Git discovery")
    parser.add_argument("--source-root", type=Path, help="defaults to manifest sibling sources")
    parser.add_argument("--probe", choices=["maintenance", "mixed-maintenance", "canonical"], help="bounded 15/20-frame component probe")
    args = parser.parse_args()
    output = (args.output or ROOT / "out/network-qualification" / time.strftime("graph-%Y%m%dT%H%M%SZ", time.gmtime())).resolve()
    frozen = None
    if args.source_root and not args.source_manifest:
        parser.error("--source-root requires --source-manifest")
    if args.source_manifest:
        helpers = runpy.run_path(str(ROOT / "scripts/qualify-soak.py"))
        manifest = args.source_manifest.resolve()
        base = (args.source_root or manifest.parent / "sources").resolve()
        files, manifest_hash = helpers["frozen_sources"](manifest, base, ROOT, output)
        frozen = (helpers, manifest, base, files, manifest_hash)
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    if not target.is_absolute():
        target = ROOT / target
    target = target.resolve()
    if frozen and (target == frozen[2] or frozen[2] in target.parents):
        parser.error("CARGO_TARGET_DIR must be outside the frozen source tree")
    output.mkdir(parents=True, exist_ok=True)
    command = ["cargo", "build", "--release", "-p", "engine_net", "--example", "graph_qualification", "--locked", "--offline"]
    metadata = {
        "created_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "host": platform.platform(), "machine": platform.machine(),
        "cpu": capture("sysctl", "-n", "machdep.cpu.brand_string") if platform.system() == "Darwin" else platform.processor(),
        "memory_bytes": capture("sysctl", "-n", "hw.memsize") if platform.system() == "Darwin" else None,
        "logical_cpus": capture("sysctl", "-n", "hw.logicalcpu") if platform.system() == "Darwin" else None,
        "rustc": capture("rustc", "-Vv"), "git_head": None if frozen else capture("git", "rev-parse", "HEAD"),
        "git_status": None if frozen else capture("git", "status", "--short"), "build_command": command,
        "allocation_instrumentation": "System allocator with atomic requested-live/peak byte counters; timing includes this overhead",
        "rustflags": os.environ.get("RUSTFLAGS", ""), "cargo_encoded_rustflags": os.environ.get("CARGO_ENCODED_RUSTFLAGS", ""),
        "worker_threads": 1, "affinity": "unpinned", "gpu_browser": "not applicable; headless engine harness",
        "layout_seed": 20260909, "connection_count": 100, "actor_count": 50000,
        "probe": args.probe,
        "committed_actor_updates_per_frame": 1000 if args.probe == "mixed-maintenance" else 50000 if args.probe == "maintenance" else None,
        "fixture_offset": "maintenance only: +0.25 XZ avoids incidental footprint changes during 0.001m motion; exact span checks counted outside gather" if args.probe in ("maintenance", "mixed-maintenance") else None,
        "probe_limits": "maintenance: 3 patterns x 5 frames; canonical: 4 scenarios x 5 frames; runtime capped at 240s" if args.probe else None,
        "timing_basis": "wall-clock nanoseconds; aggregate work for 100 connections per replication frame",
        "transport_basis": ("no encoding or transport; persistent graph maintenance/gather only" if args.probe in ("maintenance", "mixed-maintenance") else "positive fixture payload bytes only; no full/group codecs, transport framing or physical wire" if args.probe == "canonical" else "actual full/group codec bytes through synthetic immediate bounded sink; no transport framing or physical wire"),
        "reference_hardware": "unassigned; no performance gate verdict", "soak": "not performed",
    }
    metadata["source_manifest_sha256"] = frozen[4] if frozen else None
    sources = ["Cargo.toml", "Cargo.lock", "engine/net/Cargo.toml", "engine/net/build.rs", "scripts/qualify-graph.py", "engine/net/examples/graph_qualification.rs", "todo/config/arena_profile.toml", "todo/config/spatial_scale_profile.toml"]
    sources += sorted(str(path.relative_to(ROOT)) for path in (ROOT/"engine/net/src").rglob("*.rs"))
    metadata["source_sha256"] = {name: hashlib.sha256((ROOT/name).read_bytes()).hexdigest() for name in sources}
    (output/"metadata.json").write_text(json.dumps(metadata, indent=2)+"\n")
    with (output/"build.log").open("w") as log:
        subprocess.run(command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, check=True)
    after_build = {name: hashlib.sha256((ROOT/name).read_bytes()).hexdigest() for name in sources}
    metadata["source_unchanged_during_build"] = after_build == metadata["source_sha256"]
    if not metadata["source_unchanged_during_build"]:
        (output/"metadata.json").write_text(json.dumps(metadata, indent=2)+"\n")
        raise RuntimeError("qualification source changed during build; rerun from stable source")
    if frozen and (not frozen[0]["unchanged"](frozen[2], frozen[3]) or frozen[0]["digest"](frozen[1]) != frozen[4]):
        raise RuntimeError("frozen sources changed during build")
    executable = target / "release/examples/graph_qualification"
    metadata["executable_sha256"] = hashlib.sha256(executable.read_bytes()).hexdigest()
    (output/"metadata.json").write_text(json.dumps(metadata, indent=2)+"\n")
    run = [str(executable), str(output)]
    if args.probe:
        run.append(f"--{args.probe}-probe")
    if Path("/usr/bin/time").exists():
        run = ["/usr/bin/time", "-l" if platform.system() == "Darwin" else "-v", *run]
    with (output/"run.log").open("w") as log:
        subprocess.run(run, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=240 if args.probe else None)
    metadata["source_unchanged_after_run"] = all(hashlib.sha256((ROOT/name).read_bytes()).hexdigest() == value for name, value in metadata["source_sha256"].items())
    metadata["executable_unchanged_after_run"] = hashlib.sha256(executable.read_bytes()).hexdigest() == metadata["executable_sha256"]
    metadata["frozen_manifest_unchanged_after_run"] = not frozen or (frozen[0]["unchanged"](frozen[2], frozen[3]) and frozen[0]["digest"](frozen[1]) == frozen[4])
    (output/"metadata.json").write_text(json.dumps(metadata, indent=2)+"\n")
    if not all(metadata[key] for key in ["source_unchanged_after_run", "executable_unchanged_after_run", "frozen_manifest_unchanged_after_run"]):
        raise RuntimeError("qualification source or executable changed during run")
    if args.probe:
        with (output/f"{args.probe}.csv").open() as source:
            rows = list(csv.DictReader(source))
        group = "pattern" if args.probe in ("maintenance", "mixed-maintenance") else "scenario"
        results = {}
        for name in dict.fromkeys(row[group] for row in rows):
            selected = [row for row in rows if row[group] == name]
            fields = {}
            for key in selected[0]:
                if key in [group, "frame"]:
                    continue
                values = sorted(int(row[key]) for row in selected)
                fields[key] = {"min": values[0], "max": values[-1], "p50": values[len(values)//2], "sum": sum(values)}
            results[name] = {"frames": len(selected), "measurements": fields}
        summary = {"status": "bounded_component_probe; not a release or 30Hz feasibility verdict", "probe": args.probe, "scenarios": results,
            "limitations": "Five observations per case; canonical probe times fixture payload construction/cache lookup, not transport codecs, Dreamwake JSON, simulation or rendering. Reference and cached paths compare identical bytes; private field grants bypass reuse. Maintenance runs 50000 committed upserts; mixed-maintenance runs 1000 (100 players + 900 dynamics), retaining 49000 unchanged awake props. Both update 100 approved observers per frame, with oracle work outside timing. Neither is the complete five-scenario capacity qualification."}
        (output/"summary.json").write_text(json.dumps(summary, indent=2)+"\n")
        print(output/"summary.json")
        return
    with (output/"frames.csv").open() as source:
        rows = list(csv.DictReader(source))
    scenarios = {}
    def quantile(values: list[int], q: float) -> int:
        return sorted(values)[max(0, math.ceil(len(values)*q)-1)]
    for name in dict.fromkeys(row["scenario"] for row in rows):
        selected = [row for row in rows if row["scenario"] == name]
        fields = {}
        for key in selected[0]:
            if key in ["scenario", "frame"]:
                continue
            values = [int(row[key]) for row in selected]
            fields[key] = {"min": min(values), "max": max(values), "sum": sum(values), "p50": quantile(values,.5), "p99": quantile(values,.99), "last": values[-1]}
        scenarios[name] = {"frames": len(selected), "measurements": fields}
    (output/"summary.json").write_text(json.dumps({"status": "bounded_behavior_assertions_passed; observed_performance_not_a_release_gate", "scenarios": scenarios},indent=2)+"\n")
    lines = ["# Current-host synthetic graph qualification", "", (output/"qualification.txt").read_text(), "", "All CPU columns below are aggregate milliseconds per frame for 100 peers. Quantiles include transition frames; nearest-rank p99 is the maximum for these 90-frame and two-frame samples. Teleport frames after terminal reset perform no peer delivery work. Candidate/eligible totals include only successful gathers; rejected gather work counts are unavailable, with caps reported separately in dense-oracle.txt. Max pending below counts entering/leaving transitions; summary.json separately records mass-wake pending new versions.", "", "| Scenario | Frames | Maintenance p99 ms | Gather p99 ms | Offer + encode p99 ms | Schedule p99 ms | Codec bytes | Max pending | Resets |", "|---|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for name, result in scenarios.items():
        m = result["measurements"]
        times = " | ".join(f"{m[key]['p99']/1e6:.3f}" for key in ["maintenance_ns","gather_ns","offer_encode_ns","schedule_ns"])
        lines.append(f"| {name} | {result['frames']} | {times} | {m['encoded_codec_bytes']['sum']} | {m['pending_transitions']['max']} | {m['terminal_resets']['sum']} |")
    lines += ["", "Memory allocation accounting:", "", (output/"graph-memory.txt").read_text(), "", "Limitations: this exercises generic library contracts, not Dreamwake's production 100-peer server or physical egress. Asset readiness is a synthetic grant gate. The fixture payload is schema-projected test data; no client prediction, renderer, asset download, socket loss distribution, 24-hour soak or reference-machine deadline qualification is represented. One deliberately dropped dormant delivery verifies per-peer retry independence. Scope/exit controls are applied synchronously and are not charged to codec byte totals. Timing includes atomic requested-allocation instrumentation. Serialization costs include encoding deferred candidates; they are reported separately from the graph."]
    (output/"report.md").write_text("\n".join(lines)+"\n")
    print(output/"report.md")


if __name__ == "__main__":
    main()
