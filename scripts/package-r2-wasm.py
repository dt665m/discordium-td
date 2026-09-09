#!/usr/bin/env python3
"""Split a Trunk build into a Pages launcher and immutable gzip WASM for R2."""
import argparse
import base64
import gzip
import hashlib
import json
from pathlib import Path
import shutil


def integrity(data):
    return "sha384-" + base64.b64encode(hashlib.sha384(data).digest()).decode()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dist", type=Path)
    parser.add_argument("output", type=Path, help="New output directory")
    parser.add_argument("--release", required=True, help="Immutable release identifier")
    parser.add_argument("--asset-base", default="https://assets.datab.fun")
    parser.add_argument("--wasm", type=Path, help="Matching WASM override from the same build")
    args = parser.parse_args()
    if not args.release or any(c not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_" for c in args.release):
        parser.error("release must contain only letters, digits, hyphens, or underscores")
    files = list(args.dist.glob("*.wasm"))
    if len(files) != 1:
        parser.error("expected exactly one WASM in the Trunk output")
    original = files[0]
    data = (args.wasm or original).read_bytes()
    if not data.startswith(b"\x00asm"):
        parser.error("invalid WASM header")
    digest = hashlib.sha256(data).hexdigest()
    name = f"dreamwake-{digest[:16]}_bg.wasm"
    key = f"dreamwake/releases/{args.release}/{name}"
    url = f"{args.asset_base.rstrip('/')}/{key}"
    html = (args.dist / "index.html").read_text()
    old_url = f"/{original.name}"
    old_integrity = integrity(original.read_bytes())
    if html.count(old_url) != 2 or old_integrity not in html:
        parser.error("expected Trunk root-relative init, preload, and SHA-384 integrity")
    html = html.replace(old_url, url).replace(old_integrity, integrity(data))
    # Refuse overwriting an earlier package so rollback artifacts survive.
    args.output.mkdir(parents=True, exist_ok=False)
    pages = args.output / "pages"
    shutil.copytree(args.dist, pages)
    (pages / original.name).unlink()
    (pages / "index.html").write_text(html)
    assets = args.output / "r2"
    assets.mkdir()
    compressed = gzip.compress(data, mtime=0)
    (assets / (name + ".gz")).write_bytes(compressed)
    manifest = dict(release=args.release, bucket="dreamwake-releases", key=key,
                    url=url, sha256=digest, integrity=integrity(data),
                    bytes=len(data), gzip_bytes=len(compressed),
                    content_type="application/wasm", content_encoding="gzip")
    (args.output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps(manifest, indent=2))


if __name__ == "__main__":
    main()
