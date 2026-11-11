#!/usr/bin/env python3
"""Run archive pinning against a small, independently authored Rust crate."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


BUILD_SCRIPT = Path(__file__).with_name('build-from-archive.sh').resolve()
WINDOWS_TARGET = 'x86_64-pc-windows-gnu'


class ArchiveTargets(unittest.TestCase):
    def setUp(self):
        test_root = BUILD_SCRIPT.parent.parent / 'target' / 'archive-tests'
        test_root.mkdir(parents=True, exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(prefix='bead-archive-test-',
                                                     dir=test_root)
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.environment = os.environ.copy()
        self.environment['BEAD_BUILD_SCRATCH_ROOT'] = str(self.root / 'build-scratch')
        self.repository = self.root / 'source'
        self.repository.mkdir()
        (self.repository / 'src').mkdir()
        (self.repository / 'Cargo.toml').write_text(
            '[package]\nname = "bead"\nversion = "0.0.1"\nedition = "2021"\n'
            '[workspace]\n', encoding='utf-8')
        (self.repository / 'src/main.rs').write_text(
            'fn main() { println!("bead archive fixture 0.0.1"); }\n',
            encoding='utf-8')
        (self.repository / 'build.rs').write_text(
            'fn main() {\n'
            '  let output = std::process::Command::new("git")\n'
            '    .args(["rev-parse", "--show-toplevel"]).output().expect("run git");\n'
            '  assert!(!output.status.success(), "archive discovered a Git checkout: {}",\n'
            '    String::from_utf8_lossy(&output.stdout));\n'
            '}\n', encoding='utf-8')
        self.run_command('git', 'init', '--quiet')
        self.run_command('git', 'config', 'user.name', 'Archive test')
        self.run_command('git', 'config', 'user.email', 'archive@example.invalid')
        self.run_command('cargo', 'generate-lockfile', '--offline')
        self.run_command('git', 'add', '--', 'Cargo.toml', 'Cargo.lock', 'src/main.rs', 'build.rs')
        self.run_command('git', 'commit', '--quiet', '-m', 'Create fixture')
        self.commit = self.run_command('git', 'rev-parse', 'HEAD').stdout.strip()
        rust_version = self.run_command('rustc', '-vV').stdout
        self.host = next(line.split(': ', 1)[1] for line in rust_version.splitlines()
                         if line.startswith('host: '))
        (self.repository / 'sentinel.txt').write_text('keep staged data\n', encoding='utf-8')
        self.run_command('git', 'add', '--', 'sentinel.txt')
        self.original_index = (self.repository / '.git/index').read_bytes()
        self.original_status = self.run_command('git', 'status', '--porcelain').stdout

    def run_command(self, *command, check=True):
        """Run a bounded child in the disposable repository and retain its diagnostics."""
        result = subprocess.run(command, cwd=self.repository, env=self.environment,
                                text=True, capture_output=True, timeout=180)
        if check:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def build(self, name, *arguments, check=True):
        return self.run_command('bash', str(BUILD_SCRIPT), self.commit, '--name', name,
                                '--out', str(self.root / 'pins'), *arguments, check=check)

    def assert_source_unchanged(self):
        self.assertEqual(self.run_command('git', 'rev-parse', 'HEAD').stdout.strip(),
                         self.commit)
        self.assertEqual(self.run_command('git', 'status', '--porcelain').stdout,
                         self.original_status)
        self.assertEqual((self.repository / '.git/index').read_bytes(), self.original_index)

    def assert_receipt(self, filename, target, execution):
        binary = self.root / 'pins' / filename
        receipt = json.loads(binary.with_name(binary.name + '.metadata.json').read_text())
        self.assertEqual(receipt['git_commit_sha'], self.commit)
        self.assertEqual(receipt['binary_sha256'], hashlib.sha256(binary.read_bytes()).hexdigest())
        self.assertEqual(receipt['binary_size_bytes'], binary.stat().st_size)
        self.assertEqual(receipt['build_target'], target)
        self.assertEqual(receipt['build_host_target'], self.host)
        self.assertEqual(receipt['execution_verification'], execution)
        self.assertEqual(receipt['source'], 'git-archive')
        self.assertEqual(Path(receipt['build_scratch_path']).parent,
                         Path(self.environment['BEAD_BUILD_SCRATCH_ROOT']))
        self.assertFalse(Path(receipt['build_scratch_path']).exists())
        self.assert_source_unchanged()
        return binary, receipt

    def test_native_build_preserves_source_and_runs_the_binary(self):
        self.environment['GIT_DIR'] = str(self.repository / '.git')
        self.environment['GIT_WORK_TREE'] = str(self.repository)
        self.build('native')
        binary, receipt = self.assert_receipt('native', self.host, 'executed')
        self.assertEqual(self.run_command(str(binary), '--version').stdout.strip(),
                         'bead archive fixture 0.0.1')
        self.assertEqual(receipt['embedded_version_string'], 'bead archive fixture 0.0.1')

    def test_windows_build_records_foreign_execution_as_pending(self):
        self.assertNotEqual(self.host, WINDOWS_TARGET,
                            'Run this cross-compilation check on a non-Windows host')
        self.build('windows', '--target', WINDOWS_TARGET)
        binary, receipt = self.assert_receipt('windows.exe', WINDOWS_TARGET, 'pending')
        self.assertEqual(binary.read_bytes()[:2], b'MZ')
        self.assertIsNone(receipt['embedded_version_string'])
        self.assertIn('--target ' + WINDOWS_TARGET, receipt['build_command'])

    def test_existing_pin_and_unknown_options_are_refused(self):
        pins = self.root / 'pins'
        pins.mkdir()
        existing = pins / 'keep'
        existing.write_bytes(b'previous pin\n')
        result = self.build('keep', check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(existing.read_bytes(), b'previous pin\n')
        self.assertIn('refusing to overwrite', result.stderr)
        result = self.build('invalid', '--not-a-build-option', check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('unknown argument', result.stderr)
        self.assertFalse((pins / 'invalid').exists())
        result = self.build('invalid-target', '--target', 'not-a-rust-target', check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((pins / 'invalid-target').exists())
        self.assertFalse((pins / 'invalid-target.metadata.json').exists())
        self.assert_source_unchanged()


if __name__ == '__main__':
    unittest.main(verbosity=2)
