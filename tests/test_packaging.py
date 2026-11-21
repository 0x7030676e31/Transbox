import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
sys.path.insert(0, str(SCRIPTS))
spec = importlib.util.spec_from_file_location("transbox_build", SCRIPTS / "build.py")
build = importlib.util.module_from_spec(spec)
spec.loader.exec_module(build)


class Packaging(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)

    def test_payload_bytes_do_not_depend_on_source_mtime(self):
        source = self.root / "payload"
        source.mkdir()
        file = source / "relay"
        file.write_bytes(b"relay bytes")
        file.chmod(0o755)
        first, second = self.root / "one.zip", self.root / "two.zip"
        build.create_zip(source, first)
        os.utime(file, (1700000000, 1700000000))
        build.create_zip(source, second)
        self.assertEqual(first.read_bytes(), second.read_bytes())
        with zipfile.ZipFile(first) as archive:
            self.assertEqual(archive.read("relay"), b"relay bytes")
            self.assertEqual((archive.getinfo("relay").external_attr >> 16) & 0o111, file.stat().st_mode & 0o111)

    def test_versioned_release_assets_match_the_executable_and_notices(self):
        executable = self.root / "target/x86_64-pc-windows-gnullvm/release/transbox.exe"
        executable.parent.mkdir(parents=True)
        executable.write_bytes(b"test executable")
        (self.root / "Cargo.lock").write_bytes(b"test lock")
        payload = self.root / ".payload/windows"
        (payload / "licenses").mkdir(parents=True)
        (payload.parent / "windows.zip").write_bytes(b"test payload")
        (payload / "licenses/notice.txt").write_text("upstream notice")
        (payload / "browser.dll").write_bytes(b"not part of the notices archive")
        ndk = self.root / "ndk"
        ndk.mkdir()
        (ndk / "source.properties").write_text("Pkg.Revision = 29.0.13846066\n")
        with patch.object(build, "ROOT", self.root), patch.object(build, "source_revision", return_value={"commit": None, "dirty": None}), patch.object(build.subprocess, "check_output", return_value="rustc test\n"):
            build.release_assets(payload, "0.2.0-alpha.1", ndk, ["arm64-v8a"])
        stem = "transbox-v0.2.0-alpha.1-windows-x64"
        dist = self.root / "dist"
        digest = hashlib.sha256(b"test executable").hexdigest()
        self.assertEqual((dist / (stem + ".exe.sha256")).read_text(), f"{digest}  {stem}.exe\n")
        metadata = json.loads((dist / (stem + ".build.json")).read_text())
        self.assertEqual(metadata["version"], "0.2.0-alpha.1")
        self.assertEqual(metadata["executable_sha256"], digest)
        self.assertEqual(metadata["ndk_version"], "29.0.13846066")
        with zipfile.ZipFile(dist / (stem + ".notices.zip")) as archive:
            self.assertEqual(archive.namelist(), ["licenses/notice.txt"])


if __name__ == "__main__":
    unittest.main()
