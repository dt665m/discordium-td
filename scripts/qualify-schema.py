#!/usr/bin/env python3
"""Compare Dreamwake's twelve canonical and nine compact public codecs in native/WASM."""
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


def run(args: list[str]) -> str:
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def source_hashes() -> dict[str, str]:
    files = [ROOT / "Cargo.toml", ROOT / "Cargo.lock", Path(__file__).resolve(), ROOT / "scripts/qualify-replay.py", ROOT / "scripts/schema-browser.html", ROOT / "scripts/schema-worker.js"]
    for folder in ["engine/build-support", "engine/core", "engine/net", "games/dreamwake/simulation"]:
        files.extend(p for p in (ROOT / folder).rglob("*") if p.is_file() and (p.suffix == ".rs" or p.name == "Cargo.toml"))
    return {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(set(files))}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--wasm-bindgen", type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    if output.exists() and any(output.iterdir()):
        parser.error("output must be new or empty")
    output.mkdir(parents=True, exist_ok=True)
    # Shared tool-version check prevents mixing the CLI with another lockfile ABI.
    bindgen = runpy.run_path(str(ROOT / "scripts/qualify-replay.py"))["bindgen"]
    cli = bindgen(args.wasm_bindgen)
    before = source_hashes()
    (output / "sources.before.json").write_text(json.dumps(before, indent=2) + "\n")
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    if not target.is_absolute():
        target = ROOT / target
    native = json.loads(run(["cargo", "run", "-p", "dreamwake_sim", "--example", "schema", "--quiet"]))
    (output / "native.json").write_text(json.dumps(native, indent=2) + "\n")
    run(["cargo", "build", "-p", "dreamwake_sim", "--example", "schema_web", "--target", "wasm32-unknown-unknown", "--quiet"])
    run([str(cli), "--target", "nodejs", "--out-dir", str(output / "wasm"),
         str(target / "wasm32-unknown-unknown/debug/examples/schema_web.wasm")])
    run([str(cli), "--target", "web", "--out-dir", str(output / "wasm-web"),
         str(target / "wasm32-unknown-unknown/debug/examples/schema_web.wasm")])
    for name in ["schema-browser.html", "schema-worker.js"]:
        shutil.copyfile(ROOT / "scripts" / name, output / name)
    wasm = json.loads(run(["node", "-e", "process.stdout.write(require(process.argv[1]).schema_fixture())", str(output / "wasm/schema_web.js")]))
    (output / "wasm.json").write_text(json.dumps(wasm, indent=2) + "\n")
    expected = {"registry", "global", "owner", "collision", "hero", "enemy", "projectile", "wisp", "effect", "damage", "cover", "cover_marker", "platform"}
    public_roots = expected - {"registry", "global", "owner", "collision"}
    expected |= {"compact_" + name for name in public_roots}
    if {name for name, _ in native} != expected or len(native) != len(expected):
        raise RuntimeError("fixture omitted a production root")
    if native != wasm:
        raise RuntimeError("native/WASM schema or encoded root digest mismatch")
    after = source_hashes()
    (output / "sources.after.json").write_text(json.dumps(after, indent=2) + "\n")
    if before != after:
        changed = sorted(name for name in before.keys() | after.keys() if before.get(name) != after.get(name))
        raise RuntimeError("sources changed during qualification: " + ", ".join(changed))
    manifest = {
        "passed": True, "roots": 12, "compact_public_roots": len(public_roots), "registry": dict(native)["registry"],
        "rustc": run(["rustc", "--version"]), "wasm_bindgen": run([str(cli), "--version"]),
        "node": run(["node", "--version"]),
        "browser_page": "schema-browser.html", "browser_verified": False,
        "sources": after,
    }
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps({"passed": True, "roots": 12, "compact_public_roots": len(public_roots), "registry": manifest["registry"], "output": str(output)}))


if __name__ == "__main__":
    main()
