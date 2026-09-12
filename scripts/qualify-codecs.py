#!/usr/bin/env python3
"""Bounded deterministic adversarial qualification of the real live codecs.

Generated reports contain synthetic fixtures and source hashes only. This runner
is not a coverage-guided fuzzer and does not close all security/release gates.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import resource
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SEED = 0x7A29D10C00DECAFE


def sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def sources() -> dict[str, str]:
    files = [ROOT / "Cargo.toml", ROOT / "Cargo.lock", Path(__file__).resolve()]
    for directory in ["engine/core", "engine/net", "games/dreamwake/simulation", "games/dreamwake/protocol"]:
        base = ROOT / directory
        files += [p for p in base.rglob("*") if p.is_file() and (p.suffix == ".rs" or p.name == "Cargo.toml")]
    return {str(p.relative_to(ROOT)): sha(p) for p in sorted(set(files))}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seed", type=int, default=DEFAULT_SEED)
    parser.add_argument("--cases", type=int, default=100_000)
    parser.add_argument("--case", type=int, help="Replay one independently seeded case")
    parser.add_argument("--cpu-seconds", type=int, default=120)
    parser.add_argument("--memory-mib", type=int, default=1024)
    parser.add_argument("--output", type=Path, default=ROOT / "out/network-qualification/codecs")
    parser.add_argument("--skip-build", action="store_true", help="Use existing binary; provenance records this choice")
    args = parser.parse_args()
    if not (0 < args.seed < 2**64 and 1 <= args.cases <= 1_000_000 and 1 <= args.cpu_seconds <= 3600 and 128 <= args.memory_mib <= 8192):
        parser.error("seed/cases/CPU/memory values exceed the bounded campaign configuration")
    if args.case is not None and args.case < 0:
        parser.error("case must be nonnegative")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    before = sources()
    build = ["cargo", "build", "--locked", "-p", "dreamwake_protocol", "--example", "adversarial_decode"]
    if not args.skip_build:
        with (output / "build.log").open("w") as log:
            built = subprocess.run(build, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
        if built.returncode:
            print(f"Codec campaign build failed; see {output / 'build.log'}", file=sys.stderr)
            return built.returncode
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    if not target.is_absolute():
        target = ROOT / target
    binary = target / "debug/examples/adversarial_decode"
    after = sources()
    changed = sorted(path for path in set(before) | set(after) if before.get(path) != after.get(path))
    command = [str(binary), "--seed", str(args.seed), "--cases", str(args.cases), "--failure-dir", str(output / "failures")]
    if args.case is not None:
        command += ["--case", str(args.case)]
    memory = args.memory_mib * 1024 * 1024
    limits = {"cpu_seconds": args.cpu_seconds, "data_bytes": memory if sys.platform.startswith("linux") else None,
              "address_space_bytes": memory if sys.platform.startswith("linux") else None,
              "process_memory_limit_note": "Linux RLIMIT_DATA/AS; macOS uses the hard allocator case caps because its process limits reject these requested values.",
              "hard_per_case_allocation_bytes": 8 * 1024 * 1024, "hard_per_case_allocation_calls": 65_536}

    def constrain() -> None:
        resource.setrlimit(resource.RLIMIT_CPU, (args.cpu_seconds, args.cpu_seconds + 1))
        if sys.platform.startswith("linux"):
            resource.setrlimit(resource.RLIMIT_DATA, (memory, memory))
            resource.setrlimit(resource.RLIMIT_AS, (memory, memory))

    provenance = {"kind": "deterministic_adversarial_codec_campaign", "seed": args.seed, "requested_cases": args.cases, "replay_case": args.case,
                  "command": command, "build_command": None if args.skip_build else build, "limits": limits,
                  "platform": platform.platform(), "python": platform.python_version(),
                  "git_head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
                  "rustc": subprocess.check_output(["rustc", "--version"], cwd=ROOT, text=True).strip(),
                  "lock_sha256": sha(ROOT / "Cargo.lock"), "binary_sha256": sha(binary),
                  "source_sha256": after, "source_changed_during_build": changed,
                  "scope": "Seeded mutations and explicit hostile cases against real decoders; no coverage-guided fuzzing, authentication penetration test, or complete security gate claim."}
    (output / "provenance.json").write_text(json.dumps(provenance, indent=2) + "\n")
    start = time.monotonic()
    timed_out = False
    with (output / "campaign.stdout").open("w") as stdout, (output / "campaign.stderr").open("w") as stderr:
        process = subprocess.Popen(command, cwd=ROOT, stdout=stdout, stderr=stderr, preexec_fn=constrain)
        while True:
            pid, status, usage = os.wait4(process.pid, os.WNOHANG)
            if pid:
                code = os.waitstatus_to_exitcode(status)
                process.returncode = code
                break
            if time.monotonic() - start > args.cpu_seconds * 3 + 30:
                timed_out = True
                process.kill()
                _, status, usage = os.wait4(process.pid, 0)
                process.returncode = os.waitstatus_to_exitcode(status)
                code = 124
                break
            time.sleep(0.05)
    final_sources = sources()
    run_changed = sorted(path for path in set(after) | set(final_sources) if after.get(path) != final_sources.get(path))
    qualified = code == 0 and not changed and not run_changed and not args.skip_build
    report = {**provenance, "qualified": qualified, "source_changed_during_run": run_changed, "source_sha256_after_run": final_sources, "exit_code": code, "wall_seconds": time.monotonic() - start, "wall_timeout": timed_out}
    if code == 0:
        report["campaign"] = json.loads((output / "campaign.stdout").read_text())
    report["peak_campaign_rss_native_units"] = usage.ru_maxrss
    report["campaign_user_cpu_seconds"] = usage.ru_utime
    report["campaign_system_cpu_seconds"] = usage.ru_stime
    report["rss_units"] = "bytes" if sys.platform == "darwin" else "KiB"
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    if qualified:
        campaign = report["campaign"]
        print(f"Passed {campaign['cases']} cases in {campaign['elapsed_seconds']:.3f}s; max case allocation {campaign['max_case_allocated_bytes']} bytes; report: {output / 'report.json'}")
    elif code == 0:
        print(f"Campaign cases passed but provenance is unqualified (changed sources or skipped build); report: {output / 'report.json'}", file=sys.stderr)
    else:
        print(f"Campaign failed (exit {code}); replay seed {args.seed} and failing case from {output / 'campaign.stderr'}; report: {output / 'report.json'}", file=sys.stderr)
    return 0 if qualified else 1


if __name__ == "__main__":
    raise SystemExit(main())
