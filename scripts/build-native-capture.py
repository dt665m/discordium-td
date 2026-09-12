#!/usr/bin/env python3
"""Build and capture the optimized native pair from an explicit frozen source identity.

The output directory must not exist. A failed build may leave diagnostic files,
but native-provenance.json is published only after the complete pair and immutable
sources have been verified. No game process is launched.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import runpy
import shutil
import subprocess
import sys
import time

NAMES = ('game_server', 'dreamwake')
PACKAGE_DIRS = {'game_server': 'games/dreamwake/server', 'dreamwake': 'games/dreamwake/client'}


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open('rb') as stream:
        for data in iter(lambda: stream.read(1024 * 1024), b''):
            value.update(data)
    return value.hexdigest()


def optimized_profile(profile: object, name: str) -> None:
    if (not isinstance(profile, dict) or profile.get('opt_level') not in ('1', '2', '3', 's', 'z')
            or profile.get('test') is not False):
        raise ValueError(f'{name}: missing or unoptimized Cargo profile evidence')


def require_optimized_provenance(provenance: object) -> None:
    if not isinstance(provenance, dict) or provenance.get('target') != 'native':
        raise ValueError('paired binary provenance must identify a native build')
    if provenance.get('profiles') != {name: 'release' for name in NAMES}:
        raise ValueError('paired binary provenance requires release profiles for both executables')
    artifacts = provenance.get('artifacts')
    if not isinstance(artifacts, dict) or set(artifacts) != set(NAMES):
        raise ValueError('paired binary provenance is incomplete')
    for name in NAMES:
        artifact = artifacts[name]
        if not isinstance(artifact, dict):
            raise ValueError(f'{name}: missing artifact provenance')
        optimized_profile(artifact.get('profile'), name)


def native_executable(path: Path) -> str:
    """Reject scripts, WebAssembly, foreign OS/CPU artifacts, and symlinks."""
    if path.is_symlink() or not path.is_file() or not os.access(path, os.X_OK):
        raise ValueError(f'ordinary executable missing: {path}')
    with path.open('rb') as stream:
        header = stream.read(4096)
    machine = platform.machine().lower()
    if sys.platform == 'darwin':
        cpu = {'arm64': 0x100000c, 'aarch64': 0x100000c, 'x86_64': 0x1000007}.get(machine)
        endian = {b'\xcf\xfa\xed\xfe': 'little', b'\xfe\xed\xfa\xcf': 'big'}.get(header[:4])
        if cpu is not None and endian and int.from_bytes(header[4:8], endian) == cpu:
            return 'mach-o'
        # Universal Mach-O has a big-endian architecture table.
        if cpu is not None and header[:4] in (b'\xca\xfe\xba\xbe', b'\xca\xfe\xba\xbf'):
            count = int.from_bytes(header[4:8], 'big')
            stride = 20 if header[:4] == b'\xca\xfe\xba\xbe' else 32
            if 0 < count <= (len(header) - 8) // stride and any(
                    int.from_bytes(header[8 + index * stride:12 + index * stride], 'big') == cpu
                    for index in range(count)):
                return 'mach-o-universal'
    elif sys.platform.startswith('linux') and header[:4] == b'\x7fELF' and len(header) >= 20:
        cpu = {'x86_64': 62, 'aarch64': 183, 'arm64': 183, 'riscv64': 243}.get(machine)
        endian = {1: 'little', 2: 'big'}.get(header[5])
        if cpu is not None and endian and int.from_bytes(header[18:20], endian) == cpu:
            return 'elf'
    raise ValueError(f'not a native executable for this host: {path}')


def build_command() -> list[str]:
    return ['cargo', 'build', '--release', '--locked', '--offline', '--message-format=json-render-diagnostics',
            '-p', 'dreamwake_server', '-p', 'dreamwake_client', '--bin', 'game_server', '--bin', 'dreamwake']


def emitted_pair(messages: Path, workspace: Path) -> dict:
    artifacts = {}
    finished = False
    with messages.open() as stream:
        for line in stream:
            if not line.startswith('{'):
                continue
            row = json.loads(line)
            if row.get('reason') == 'build-finished':
                if row.get('success') is not True:
                    raise ValueError('Cargo did not report a successful build')
                finished = True
            if row.get('reason') != 'compiler-artifact':
                continue
            target = row.get('target', {})
            name = target.get('name')
            if name not in NAMES or target.get('kind') != ['bin']:
                continue
            package = workspace / PACKAGE_DIRS[name]
            if (Path(row['manifest_path']).resolve() != package / 'Cargo.toml'
                    or Path(target['src_path']).resolve() != package / 'src/main.rs'):
                raise ValueError(f'{name}: Cargo artifact belongs to a different source workspace')
            optimized_profile(row.get('profile'), name)
            executable = row.get('executable')
            if not isinstance(executable, str) or not Path(executable).is_absolute():
                raise ValueError(f'{name}: Cargo did not emit an absolute executable path')
            path = Path(executable)
            if name in artifacts:
                raise ValueError(f'{name}: ambiguous duplicate Cargo executable artifacts')
            native_format = native_executable(path)
            artifacts[name] = {'source_path': str(path), 'sha256': digest(path),
                               'bytes': path.stat().st_size, 'profile': row['profile'],
                               'package_id': row['package_id'], 'manifest_path': row['manifest_path'],
                               'native_format': native_format}
    if not finished or set(artifacts) != set(NAMES):
        raise ValueError('Cargo did not emit the complete successful native pair')
    return artifacts


def build_pair(workspace: Path, output: Path, target_dir: Path | None = None) -> dict:
    """Shared live/frozen release build; paths always come from Cargo JSON."""
    # Cargo treats an explicitly empty encoded value as an override of all
    # ambient/config rustflags. Otherwise -C opt-level=0 can override the real
    # compiler settings while artifact JSON continues to report profile level 3.
    environment = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_ENCODED_RUSTFLAGS='')
    if target_dir is not None:
        environment['CARGO_TARGET_DIR'] = str(target_dir)
    messages = output / 'build-messages.jsonl'
    started = time.monotonic()
    with messages.open('x') as stdout, (output / 'build.log').open('x') as stderr:
        result = subprocess.run(build_command(), cwd=workspace, env=environment,
                                stdout=stdout, stderr=stderr, check=False)
    if result.returncode:
        raise ValueError(f'paired native build failed ({result.returncode}); see {output / "build.log"}')
    return {'target': 'native', 'host': {'system': platform.system(), 'machine': platform.machine()},
            'profiles': {name: 'release' for name in NAMES}, 'commands': [build_command()],
            'env': {name: environment[name] for name in ('CARGO_INCREMENTAL', 'CARGO_TARGET_DIR', 'CARGO_ENCODED_RUSTFLAGS') if name in environment},
            'build_seconds': time.monotonic() - started,
            'cargo_messages_sha256': digest(messages), 'artifacts': emitted_pair(messages, workspace)}


def capture(args) -> dict:
    workspace = args.workspace.resolve()
    manifest = args.source_manifest.resolve()
    source_root = (args.source_root or manifest.parent / 'sources').resolve()
    output = args.output.resolve()
    if args.output.is_symlink() or output.exists():
        raise ValueError('output directory must be new and must not exist')
    if not re.fullmatch('[0-9a-f]{64}', args.expected_source_sha256) or digest(manifest) != args.expected_source_sha256:
        raise ValueError('source manifest SHA-256 differs from requested identity')
    helpers = runpy.run_path(str(Path(__file__).with_name('qualify-soak.py')))
    files, manifest_sha = helpers['frozen_sources'](manifest, source_root, workspace, output)
    target_dir = (args.target_dir or output / 'target').resolve()
    if target_dir == source_root or target_dir.is_relative_to(source_root):
        raise ValueError('target directory must be outside the immutable source tree')
    output.mkdir(parents=True, mode=0o700)
    provenance = build_pair(workspace, output, target_dir)
    require_optimized_provenance(provenance)

    def unchanged():
        return digest(manifest) == manifest_sha and helpers['unchanged'](source_root, files)

    if not unchanged():
        raise ValueError('frozen sources changed during the native build')
    binaries = output / 'binaries'
    binaries.mkdir(mode=0o700)
    for name in NAMES:
        artifact = provenance['artifacts'][name]
        source = Path(artifact['source_path'])
        destination = binaries / name
        shutil.copy2(source, destination)
        destination.chmod(0o500)
        if digest(source) != artifact['sha256'] or digest(destination) != artifact['sha256']:
            raise ValueError(f'{name}: emitted binary changed during capture')
        native_executable(destination)
        artifact['path'] = f'binaries/{name}'
    if not unchanged():
        raise ValueError('frozen sources changed during native capture')
    provenance.update(format=2, status='complete', source_manifest_sha256=manifest_sha,
                      source_manifest=str(manifest), source_workspace=str(workspace), source_files=len(files),
                      built_utc=datetime.now(timezone.utc).isoformat())
    temporary = output / 'native-provenance.json.tmp'
    temporary.write_text(json.dumps(provenance, indent=2, allow_nan=False) + '\n')
    temporary.chmod(0o400)
    temporary.replace(output / 'native-provenance.json')
    return provenance


def parser() -> argparse.ArgumentParser:
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument('--workspace', type=Path, required=True)
    cli.add_argument('--source-manifest', type=Path, required=True)
    cli.add_argument('--expected-source-sha256', required=True)
    cli.add_argument('--source-root', type=Path, help='defaults to manifest sibling sources')
    cli.add_argument('--output', type=Path, required=True, help='new directory; must not already exist')
    cli.add_argument('--target-dir', type=Path, help='Cargo cache outside frozen sources; defaults to output/target')
    return cli


def main() -> int:
    try:
        args = parser().parse_args()
        capture(args)
        print(f'Captured verified release/native pair: {args.output.resolve() / "native-provenance.json"}')
        return 0
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f'Native capture failed: {error}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
