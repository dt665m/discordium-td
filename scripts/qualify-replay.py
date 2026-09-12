#!/usr/bin/env python3
"""Build shared native/WASM replay fixtures; browser verification is a separate step."""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def sources() -> dict[str, str]:
    files = [ROOT / "Cargo.toml", ROOT / "Cargo.lock", Path(__file__).resolve(),
             ROOT / "scripts/replay-browser.html"]
    for folder in ["engine/build-support", "engine/core", "engine/net", "games/dreamwake/simulation"]:
        files += [p for p in (ROOT / folder).rglob("*")
                  if p.is_file() and (p.suffix == ".rs" or p.name == "Cargo.toml")]
    return {str(p.relative_to(ROOT)): sha(p) for p in sorted(set(files))}


def bindgen(explicit: Path | None) -> Path:
    packages = tomllib.loads((ROOT / "Cargo.lock").read_text())["package"]
    version = next(p["version"] for p in packages if p["name"] == "wasm-bindgen")
    candidates = [explicit] if explicit else [
        Path(shutil.which("wasm-bindgen") or "/missing-wasm-bindgen"),
        Path.home() / "Library/Caches/dev.trunkrs.trunk" / f"wasm-bindgen-{version}/wasm-bindgen",
        Path.home() / ".cache/trunk" / f"wasm-bindgen-{version}/wasm-bindgen",
    ]
    for candidate in candidates:
        if candidate and candidate.is_file():
            found = subprocess.check_output([str(candidate), "--version"], text=True).strip()
            if found == f"wasm-bindgen {version}":
                return candidate.resolve()
    raise RuntimeError(f"wasm-bindgen {version} is required; pass its path with --wasm-bindgen")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--wasm-bindgen", type=Path)
    args = parser.parse_args()
    output = (args.output or ROOT / "out/network-qualification" /
              ("replay-" + datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ"))).resolve()
    if output.exists() and any(output.iterdir()):
        parser.error("output must be new or empty so old browser results cannot survive")
    output.mkdir(parents=True, exist_ok=True)
    cli = bindgen(args.wasm_bindgen)
    before = sources()
    commands = [
        ["cargo", "build", "--locked", "-p", "dreamwake_sim", "--example", "replay"],
        ["cargo", "build", "--locked", "-p", "dreamwake_sim", "--example", "replay_web",
         "--target", "wasm32-unknown-unknown", "--profile", "web-dev"],
    ]
    for i, command in enumerate(commands):
        with (output / f"build-{i}.log").open("w") as log:
            subprocess.run(command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, check=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    if not target.is_absolute():
        target = ROOT / target
    binary = target / "debug/examples/replay"
    wasm = target / "wasm32-unknown-unknown/web-dev/examples/replay_web.wasm"
    subprocess.run([str(cli), str(wasm), "--target", "web", "--out-dir", str(output),
                    "--out-name", "replay_web"], check=True)
    native = {}
    for scenario in ["combat", "wall-dash"]:
        fixture = output / f"{scenario}.json"
        subprocess.run([str(binary), "record", str(fixture), scenario], check=True)
        native[scenario] = json.loads(subprocess.check_output([str(binary), "verify", str(fixture)], text=True))
    after = sources()
    changed = sorted(p for p in set(before) | set(after) if before.get(p) != after.get(p))
    metadata = {
        "kind": "native_wasm_checkpoint_replay", "host": platform.platform(),
        "rustc": subprocess.check_output(["rustc", "-vV"], text=True).strip(),
        "build_commands": commands, "source_sha256": after, "source_changed": changed,
        "native_binary_sha256": sha(binary), "wasm_bindgen": str(cli),
        "artifacts": {p.name: sha(p) for p in output.iterdir() if p.suffix in [".wasm", ".js", ".json"]},
        "native_results": native, "browser_status": "pending_actual_browser_execution",
        "scope": "Synthetic full-checkpoint traces with every-boundary continuation; does not qualify x86, all game traces, transport, input latency, or release gates.",
    }
    (output / "provenance.json").write_text(json.dumps(metadata, indent=2) + "\n")
    if changed:
        raise RuntimeError(f"sources changed during qualification: {changed}")
    shutil.copyfile(ROOT / "scripts/replay-browser.html", output / "index.html")
    print(f"Native fixtures verified. Serve {output} locally and open it in the built-in browser.")


if __name__ == "__main__":
    main()
