#!/usr/bin/env python3
"""Offline installation contract checks; never download or run a release binary."""
import hashlib
import io
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
TARGETS = ["aarch64-apple-darwin", "x86_64-apple-darwin", "aarch64-unknown-linux-gnu", "x86_64-unknown-linux-gnu"]

class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name)
        self.artifacts = self.path / "artifacts"
        self.artifacts.mkdir()
        for target in TARGETS:
            archive = self.artifacts / f"edgelord-{target}.tar.gz"
            with tarfile.open(archive, "w:gz") as tar:
                data = b"#!/bin/sh\necho edgelord-fixture\n"
                info = tarfile.TarInfo("edgelord")
                info.size, info.mode = len(data), 0o755
                tar.addfile(info, io.BytesIO(data))
            archive.with_suffix(".gz.sha256").write_text(hashlib.sha256(archive.read_bytes()).hexdigest() + "  " + archive.name + "\n")

    def test_formula_checks_all_four_archives(self):
        output = self.path / "edgelord.rb"
        command = ["python3", str(ROOT / "scripts/update-formula.py"), "0.1.0", str(self.artifacts), str(output)]
        subprocess.run(command, check=True)
        formula = output.read_text()
        for target in TARGETS:
            self.assertIn(f"edgelord-{target}.tar.gz", formula)
        self.assertNotIn("usealtoal", formula)
        archive = next(self.artifacts.glob("*.tar.gz"))
        archive.write_bytes(b"corrupt")
        self.assertNotEqual(subprocess.run(command, capture_output=True).returncode, 0)

    def test_installer_verifies_before_replacing_binary(self):
        mocks = self.path / "bin"
        mocks.mkdir()
        curl = mocks / "curl"
        curl.write_text('''#!/usr/bin/env python3
import os, pathlib, shutil, sys
args = sys.argv[1:]
url = next(a for a in args if a.startswith('https://'))
assert url.startswith('https://github.com/rdekovich/edgelord/releases/download/v0.1.0/')
shutil.copyfile(pathlib.Path(os.environ['FIXTURE_ARTIFACTS']) / url.rsplit('/', 1)[1], args[args.index('-o') + 1])
''')
        curl.chmod(0o755)
        uname = mocks / "uname"
        uname.write_text('#!/bin/sh\ncase "$1" in -s) echo Linux;; -m) echo x86_64;; esac\n')
        uname.chmod(0o755)
        destination = self.path / "install with spaces"
        env = dict(os.environ, PATH=str(mocks) + os.pathsep + os.environ['PATH'], EDGELORD_VERSION="0.1.0", EDGELORD_INSTALL_DIR=str(destination), FIXTURE_ARTIFACTS=str(self.artifacts))
        command = ["sh", str(ROOT / "scripts/install.sh")]
        subprocess.run(command, env=env, check=True, capture_output=True)
        binary = destination / "edgelord"
        self.assertEqual(subprocess.check_output([str(binary)]).strip(), b"edgelord-fixture")
        binary.write_text("keep-existing-installation")
        (self.artifacts / "edgelord-x86_64-unknown-linux-gnu.tar.gz.sha256").write_text("0" * 64)
        self.assertNotEqual(subprocess.run(command, env=env, capture_output=True).returncode, 0)
        self.assertEqual(binary.read_text(), "keep-existing-installation")

if __name__ == '__main__':
    unittest.main()
