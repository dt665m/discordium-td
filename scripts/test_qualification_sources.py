"""Source-integrity tests; never build or launch a qualification workload."""
import hashlib
import json
from pathlib import Path
import runpy
import tempfile
import unittest

HELPERS = runpy.run_path(str(Path(__file__).with_name('qualify-soak.py')))


class FrozenSourcesTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.base = self.root / 'sources'
        self.workspace = self.base / 'game'
        self.workspace.mkdir(parents=True)
        self.source = self.workspace / 'Cargo.toml'
        self.source.write_text('[workspace]\n')
        self.files = [{'path': 'game/Cargo.toml', 'sha256': hashlib.sha256(self.source.read_bytes()).hexdigest()}]
        self.manifest = self.root / 'manifest.json'
        self.write_manifest()

    def write_manifest(self):
        self.manifest.write_text(json.dumps({'files': self.files}))

    def validate(self, **kwargs):
        return HELPERS['frozen_sources'](self.manifest, self.base, kwargs.get('workspace', self.workspace), kwargs.get('output', self.root / 'out'))

    def test_no_git_capture_and_external_output(self):
        files, digest = self.validate()
        self.assertEqual(files, self.files)
        self.assertEqual(digest, hashlib.sha256(self.manifest.read_bytes()).hexdigest())

    def test_changed_missing_and_added_sources_rejected(self):
        self.source.write_text('changed')
        with self.assertRaises(ValueError): self.validate()
        self.source.unlink()
        with self.assertRaises(ValueError): self.validate()
        self.source.write_text('[workspace]\n')
        (self.workspace / 'injected.rs').write_text('unexpected')
        with self.assertRaises(ValueError): self.validate()

    def test_traversal_absolute_and_duplicate_entries_rejected(self):
        for path in ['../outside', str(self.source), 'game/Cargo.toml']:
            self.files.append({'path': path, 'sha256': self.files[0]['sha256']})
            self.write_manifest()
            with self.assertRaises(ValueError): self.validate()
            self.files.pop()

    def test_symlink_substitution_rejected(self):
        target = self.root / 'outside'
        target.write_bytes(self.source.read_bytes())
        self.source.unlink()
        self.source.symlink_to(target)
        with self.assertRaises(ValueError): self.validate()

    def test_wrong_workspace_and_output_inside_sources_rejected(self):
        with self.assertRaises(ValueError): self.validate(workspace=self.root / 'live')
        with self.assertRaises(ValueError): self.validate(output=self.workspace / 'out')
        with self.assertRaises(ValueError): self.validate(workspace=self.base / 'unlisted')

    def test_manifest_and_file_drift_are_independently_detectable(self):
        files, before = self.validate()
        self.manifest.write_text(self.manifest.read_text() + '\n')
        self.assertNotEqual(HELPERS['digest'](self.manifest), before)
        self.assertTrue(HELPERS['unchanged'](self.base, files))
        self.source.write_text('changed')
        self.assertFalse(HELPERS['unchanged'](self.base, files))


if __name__ == '__main__':
    unittest.main()
