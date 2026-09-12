#!/usr/bin/env python3
"""Run reproducible network checks against the supplied development/acceptance plan.

Reports are generated build artifacts. A passing contract suite never changes a
real-process, hardware, impairment, or soak requirement into a passed release gate.
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import platform
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
PLAN = ROOT / "todo" / "planning"


def capture(*command: str) -> str:
    result = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, check=True)
    return result.stdout.strip()


def load_plan() -> tuple[list[dict[str, str]], list[dict[str, str]]]:
    def read(name: str) -> list[dict[str, str]]:
        with (PLAN / name).open(newline="", encoding="utf-8-sig") as source:
            return list(csv.DictReader(source))

    tasks = read("implementation_backlog.csv")
    acceptance = read("acceptance_tests.csv")
    for rows in [tasks, acceptance]:
        ids = [row["id"] for row in rows]
        if len(ids) != len(set(ids)):
            raise ValueError("duplicate planning identity")
    acceptance_ids = {row["id"] for row in acceptance}
    milestones = {row["milestone"] for row in tasks}
    for task in tasks:
        for gate in filter(None, task["dependency_gates"].split(";")):
            if gate not in milestones or gate >= task["milestone"]:
                raise ValueError(f"invalid milestone dependency in {task['id']}: {gate}")
        for test in filter(None, task["acceptance_test_ids"].split(";")):
            if test not in acceptance_ids:
                raise ValueError(f"unknown acceptance identity in {task['id']}: {test}")
    return tasks, acceptance


def machine() -> dict[str, str]:
    result = {
        "platform": platform.platform(),
        "architecture": platform.machine(),
        "compiler": capture("rustc", "--version", "--verbose"),
    }
    if sys.platform == "darwin":
        result["cpu"] = capture("sysctl", "-n", "machdep.cpu.brand_string")
        result["memory_bytes"] = capture("sysctl", "-n", "hw.memsize")
    return result


def source_digest(root: Path = ROOT) -> str:
    """Include new implementation files as well as the committed checkout."""
    names = subprocess.check_output(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], cwd=root
    ).split(b"\0")
    digest = hashlib.sha256()
    for name in sorted(set(filter(None, names))):
        path = root / name.decode("utf-8")
        digest.update(len(name).to_bytes(8, "little"))
        digest.update(name)
        digest.update(hashlib.sha256(path.read_bytes()).digest() if path.is_file() else b"deleted")
    return digest.hexdigest()


def local_sources() -> list[dict]:
    """A lockfile cannot identify the contents of local path dependencies."""
    metadata = json.loads(capture("cargo", "metadata", "--locked", "--offline", "--format-version", "1"))
    repositories: dict[Path, list[dict]] = {}
    for package in metadata["packages"]:
        if package["source"] is not None:
            continue
        manifest = Path(package["manifest_path"])
        root = Path(subprocess.check_output(
            ["git", "rev-parse", "--show-toplevel"], cwd=manifest.parent, text=True
        ).strip())
        repositories.setdefault(root, []).append({
            "package": package["name"], "version": package["version"],
            "manifest": str(manifest.relative_to(root)),
        })
    return [{
        "root": str(root),
        "git_head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip(),
        "source_sha256": source_digest(root),
        "packages": sorted(packages, key=lambda package: package["package"]),
    } for root, packages in sorted(repositories.items())]


def run_step(name: str, command: list[str], output: Path, cwd: Path = ROOT) -> dict:
    print(f"Running {name}", flush=True)
    start = time.monotonic()
    log = output / f"{name}.log"
    with log.open("w", encoding="utf-8") as stream:
        try:
            result = subprocess.run(command, cwd=cwd, stdout=stream, stderr=subprocess.STDOUT,
                                    text=True, timeout=1200, check=False)
            code = result.returncode
        except subprocess.TimeoutExpired:
            code = 124
            stream.write("\nQualification step exceeded its 1200 second work budget.\n")
    elapsed = time.monotonic() - start
    print(f"{name}: {'passed' if code == 0 else 'failed'} ({elapsed:.2f}s); {log}", flush=True)
    return {"name": name, "command": command, "cwd": str(cwd), "exit_code": code,
            "seconds": elapsed, "log": str(log), "passed": code == 0}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--list", action="store_true", help="show milestone ordering and acceptance counts")
    parser.add_argument("--run", choices=["foundation", "workspace"], help="run implemented checks")
    parser.add_argument("--output", type=Path, default=ROOT / "out" / "network-qualification")
    args = parser.parse_args()
    tasks, acceptance = load_plan()
    if args.list or not args.run:
        for milestone in sorted({row["milestone"] for row in tasks}):
            rows = [row for row in tasks if row["milestone"] == milestone]
            gates = sorted({gate for row in rows for gate in row["dependency_gates"].split(";") if gate})
            print(f"{milestone}: {len(rows)} tasks; requires {', '.join(gates) or 'none'}")
            for row in rows:
                print(f"  {row['id']}: {row['title']}")
        print(f"{len(acceptance)} acceptance requirements ({len(tasks)} implementation tasks)")
    if not args.run:
        return 0
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    manifest = {
        "version": 2,
        "suite": args.run,
        "git_head": capture("git", "rev-parse", "HEAD"),
        "branch": capture("git", "branch", "--show-current"),
        "working_tree": capture("git", "status", "--porcelain"),
        "tracked_diff_sha256": hashlib.sha256(capture("git", "diff", "HEAD").encode()).hexdigest(),
        "source_sha256": source_digest(),
        "local_sources": local_sources(),
        "lock_sha256": hashlib.sha256((ROOT / "Cargo.lock").read_bytes()).hexdigest(),
        "machine": machine(),
        "planning_sha256": {
            name: hashlib.sha256((PLAN / name).read_bytes()).hexdigest()
            for name in ["implementation_backlog.csv", "acceptance_tests.csv"]
        },
        "steps": [],
        # This is the requested acceptance checklist, with requirements preserved
        # verbatim. Contract suites cannot sign off production evidence by proxy.
        "acceptance": [{**row, "qualification": "required", "evidence": []} for row in acceptance],
        "production_ready": False,
    }
    commands = [
        ("reference-contracts", [sys.executable, "-m", "unittest", "discover", "-s", "todo/reference_model", "-v"]),
        ("architecture", ["just", "architecture"]),
        ("rust-foundation", ["cargo", "test", "--locked", "-p", "engine_net", "-p", "engine_core", "-p", "dreamwake_sim"]),
        ("replay-record", ["cargo", "run", "--locked", "-p", "dreamwake_sim", "--example", "replay", "--", "record", str(output / "replay.json")]),
        ("replay-verify", ["cargo", "run", "--locked", "-p", "dreamwake_sim", "--example", "replay", "--", "verify", str(output / "replay.json")]),
    ]
    if args.run == "workspace":
        commands = commands[:2] + [
            ("workspace-check", ["just", "check"]),
            ("workspace-tests", ["cargo", "test", "--locked", "--workspace"]),
            ("wasm-check", ["cargo", "check", "--locked", "-p", "dreamwake_client", "--target", "wasm32-unknown-unknown"]),
        ] + commands[3:]
    for name, command in commands:
        step = run_step(name, command, output)
        manifest["steps"].append(step)
        manifest["checks_passed"] = all(step["passed"] for step in manifest["steps"])
        (output / "report.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        if not step["passed"]:
            return 1
    manifest["source_changed_during_checks"] = (
        source_digest() != manifest["source_sha256"]
        or local_sources() != manifest["local_sources"]
    )
    if manifest["source_changed_during_checks"]:
        manifest["checks_passed"] = False
    (output / "report.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    if not manifest["checks_passed"]:
        print("Source changed while checks ran; repeat against a stable checkout.")
        return 1
    print(f"Implemented checks passed. Production qualification remains separate: {output / 'report.json'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
