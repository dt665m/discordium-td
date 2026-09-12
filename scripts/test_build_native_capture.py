"""Fake Cargo messages and files only; never builds or launches a native game."""
import copy
import json
from pathlib import Path
import runpy
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

MODULE = runpy.run_path(str(Path(__file__).with_name('build-native-capture.py')))


class NativeCaptureTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.workspace = self.root / 'sources/game'
        self.workspace.mkdir(parents=True)
        (self.workspace / 'Cargo.toml').write_text('[workspace]\n')
        (self.workspace / 'Cargo.lock').write_text('# fake lockfile\n')
        self.rows = []
        self.emitted = {}
        for name, package in MODULE['PACKAGE_DIRS'].items():
            folder = self.workspace / package
            (folder / 'src').mkdir(parents=True)
            (folder / 'Cargo.toml').write_text('[package]\n')
            (folder / 'src/main.rs').write_text('fn main() {}\n')
            executable = self.root / 'custom-cache' / f'emitted-{name}'
            executable.parent.mkdir(exist_ok=True)
            # Minimal fake header passes native format validation; never execute.
            executable.write_bytes(b'\xcf\xfa\xed\xfe' + (0x100000c).to_bytes(4, 'little') + name.encode())
            executable.chmod(0o700)
            self.emitted[name] = executable
            self.rows.append({'reason': 'compiler-artifact', 'package_id': f'path+file://{folder}#0.1.0',
                              'manifest_path': str(folder / 'Cargo.toml'),
                              'target': {'name': name, 'kind': ['bin'], 'src_path': str(folder / 'src/main.rs')},
                              'profile': {'opt_level': '3', 'test': False, 'debug_assertions': False},
                              'executable': str(executable), 'fresh': True})
        self.rows.append({'reason': 'build-finished', 'success': True})
        self.manifest = self.root / 'source-manifest.json'
        self.manifest.write_text(json.dumps({'files': [
            {'path': str(path.relative_to(self.root / 'sources')), 'sha256': MODULE['digest'](path)}
            for path in self.workspace.rglob('*') if path.is_file()]}))
        self.output = self.root / 'capture'
        self.args = MODULE['parser']().parse_args([
            '--workspace', str(self.workspace), '--source-manifest', str(self.manifest),
            '--expected-source-sha256', MODULE['digest'](self.manifest), '--output', str(self.output),
            '--target-dir', str(self.root / 'custom-cache')])
        self.platform_patch = patch('sys.platform', 'darwin')
        self.machine_patch = patch.object(MODULE['platform'], 'machine', return_value='arm64')
        self.platform_patch.start()
        self.machine_patch.start()
        self.addCleanup(self.platform_patch.stop)
        self.addCleanup(self.machine_patch.stop)

    def fake_cargo(self, command, **kwargs):
        self.assertEqual(command, ['cargo', 'build', '--release', '--locked', '--offline',
                                  '--message-format=json-render-diagnostics', '-p', 'dreamwake_server',
                                  '-p', 'dreamwake_client', '--bin', 'game_server', '--bin', 'dreamwake'])
        self.assertEqual(kwargs['cwd'], self.workspace)
        self.assertEqual(kwargs['env']['CARGO_TARGET_DIR'], str(self.root / 'custom-cache'))
        self.assertEqual(kwargs['env']['CARGO_INCREMENTAL'], '0')
        self.assertEqual(kwargs['env']['CARGO_ENCODED_RUSTFLAGS'], '')
        kwargs['stdout'].write(''.join(json.dumps(row) + '\n' for row in self.rows))
        kwargs['stderr'].write('FAKE CARGO; NO BUILD EXECUTED\n')
        return subprocess.CompletedProcess(command, 0)

    def capture(self, process=None):
        with patch.object(MODULE['subprocess'], 'run', side_effect=process or self.fake_cargo) as run:
            result = MODULE['capture'](self.args)
        self.assertEqual(run.call_count, 1)
        return result

    def test_only_matching_emitted_pair_captured_with_release_native_evidence(self):
        # Deliberate stale defaults must never be used instead of Cargo's paths.
        # Keep stale files outside captured sources so the source inventory stays exact.
        stale = self.root / 'target/release'
        stale.mkdir(parents=True)
        for name in MODULE['NAMES']:
            (stale / name).write_bytes(b'STALE')
        result = self.capture()
        self.assertEqual(result['status'], 'complete')
        self.assertEqual(result['source_manifest_sha256'], self.args.expected_source_sha256)
        self.assertEqual(result['profiles'], {'game_server': 'release', 'dreamwake': 'release'})
        for name in MODULE['NAMES']:
            copied = self.output / 'binaries' / name
            self.assertEqual(copied.read_bytes(), self.emitted[name].read_bytes())
            self.assertEqual(copied.stat().st_mode & 0o777, 0o500)
            self.assertEqual(result['artifacts'][name]['path'], f'binaries/{name}')
            self.assertEqual(result['artifacts'][name]['source_path'], str(self.emitted[name]))
            self.assertEqual(result['artifacts'][name]['profile']['opt_level'], '3')
            self.assertEqual(result['artifacts'][name]['native_format'], 'mach-o')
        self.assertEqual(json.loads((self.output / 'native-provenance.json').read_text()), result)
        self.assertFalse((self.output / 'native-provenance.json').stat().st_mode & 0o222)

    def test_failed_process_never_publishes_complete_capture(self):
        def failure(command, **kwargs):
            self.fake_cargo(command, **kwargs)
            return subprocess.CompletedProcess(command, 1)
        with self.assertRaisesRegex(ValueError, 'build failed'):
            self.capture(failure)
        self.assertTrue((self.output / 'build.log').exists())
        self.assertFalse((self.output / 'native-provenance.json').exists())
        self.assertFalse((self.output / 'binaries').exists())

    def test_ambient_rustflags_cannot_override_recorded_optimization(self):
        with patch.dict(MODULE['os'].environ, {'RUSTFLAGS': '-C opt-level=0', 'CARGO_ENCODED_RUSTFLAGS': '-C\x1fopt-level=0'}):
            result = self.capture()
        self.assertEqual(result['env']['CARGO_ENCODED_RUSTFLAGS'], '')

    def test_rejects_unoptimized_incomplete_ambiguous_or_wrong_source_artifacts(self):
        originals = copy.deepcopy(self.rows)
        mutations = [lambda rows: rows[0]['profile'].update(opt_level='0'),
                     lambda rows: rows[1].pop('profile'),
                     lambda rows: rows[1]['profile'].update(test=True),
                     lambda rows: rows[1].update(manifest_path=str(self.root / 'other/Cargo.toml')),
                     lambda rows: rows[1]['target'].update(src_path=str(self.root / 'other/main.rs')),
                     lambda rows: rows.pop(1),
                     lambda rows: rows.append(copy.deepcopy(rows[0])),
                     lambda rows: rows[-1].update(success=False),
                     lambda rows: rows.pop(),
                     lambda rows: rows[1].update(executable='relative/dreamwake')]
        for mutation in mutations:
            with self.subTest(mutation=mutation):
                self.rows = copy.deepcopy(originals)
                mutation(self.rows)
                with self.assertRaises((ValueError, KeyError)):
                    self.capture()
                self.assertFalse((self.output / 'native-provenance.json').exists())
                shutil.rmtree(self.output)

    def test_rejects_non_native_and_symlink_executables(self):
        client = self.emitted['dreamwake']
        original = client.read_bytes()
        for kind in ('wasm', 'script', 'wrong_cpu', 'symlink', 'missing'):
            with self.subTest(kind=kind):
                if client.exists() or client.is_symlink():
                    client.unlink()
                if kind == 'symlink':
                    client.symlink_to(self.emitted['game_server'])
                elif kind != 'missing':
                    client.write_bytes({'wasm': b'\0asm', 'script': b'#!/bin/sh\n',
                                        'wrong_cpu': original[:4] + (0x1000007).to_bytes(4, 'little')}[kind])
                    client.chmod(0o700)
                with self.assertRaises((ValueError, OSError)):
                    self.capture()
                self.assertFalse((self.output / 'native-provenance.json').exists())
                shutil.rmtree(self.output)

    def test_wrong_source_identity_or_existing_output_fails_before_build(self):
        for failure in ('sha', 'drift', 'existing', 'target_inside_sources'):
            with self.subTest(failure=failure):
                args = copy.copy(self.args)
                if failure == 'sha':
                    args.expected_source_sha256 = '0' * 64
                elif failure == 'drift':
                    (self.workspace / 'unexpected.txt').write_text('drift')
                elif failure == 'existing':
                    self.output.mkdir()
                else:
                    args.target_dir = self.workspace / 'target'
                with patch.object(MODULE['subprocess'], 'run') as run, self.assertRaises(ValueError):
                    MODULE['capture'](args)
                run.assert_not_called()
                self.assertFalse((self.output / 'native-provenance.json').exists())
                (self.workspace / 'unexpected.txt').unlink(missing_ok=True)
                if self.output.exists():
                    shutil.rmtree(self.output)

    def test_source_drift_during_build_never_publishes_capture(self):
        def drift(command, **kwargs):
            result = self.fake_cargo(command, **kwargs)
            (self.workspace / 'Cargo.toml').write_text('drift')
            return result
        with self.assertRaisesRegex(ValueError, 'sources changed'):
            self.capture(drift)
        self.assertFalse((self.output / 'native-provenance.json').exists())

    def test_binary_drift_during_copy_never_publishes_capture(self):
        copy_file = shutil.copy2
        def drift(source, destination):
            copy_file(source, destination)
            source.write_bytes(b'changed after copying')
        with patch.object(MODULE['shutil'], 'copy2', side_effect=drift), self.assertRaisesRegex(ValueError, 'changed during capture'):
            self.capture()
        self.assertFalse((self.output / 'native-provenance.json').exists())


if __name__ == '__main__':
    unittest.main()
