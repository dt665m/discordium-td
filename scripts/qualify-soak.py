#!/usr/bin/env python3
"""Capture the workspace and its local path dependencies as immutable, hashed sources.

Qualification runners (game soak, clock, spatial playtests, native capture)
build from this capture and reuse its helpers. This script builds and launches
nothing itself.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import runpy
import shutil
import subprocess

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


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--source-manifest", type=Path, help="reuse this verified capture, without Git discovery")
    parser.add_argument("--source-root", type=Path, help="captured tree root; defaults to manifest sibling sources")
    parser.add_argument("--prepare-only", action="store_true", help="accepted for compatibility; capture is the only mode")
    args = parser.parse_args()
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
    if not unchanged(source_base, files) or digest(manifest) != manifest_hash:
        raise ValueError("source capture changed during preparation")
    write_json(output / "capture.json", {
        "source_manifest": str(manifest), "source_manifest_sha256": manifest_hash,
        "captured_workspace": str(captured), "source_files": len(files),
        "prepared_only": True, "workload_launched": False,
    })
    print(f"Sources prepared without a build or workload: {manifest}", flush=True)
    print(f"Source manifest SHA-256: {manifest_hash}", flush=True)
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
