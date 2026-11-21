import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("transbox_release", Path(__file__).resolve().parents[1] / "scripts/release.py")
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseGuards(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        (self.root / "crate").mkdir()
        (self.root / "Cargo.toml").write_text('[workspace]\nmembers = ["crate"]\n[workspace.package]\nversion = "0.1.0-alpha.1"\n')
        (self.root / "crate/Cargo.toml").write_text('[package]\nname = "app"\nversion.workspace = true\n')
        (self.root / "Cargo.lock").write_text('version = 4\n[[package]]\nname = "app"\nversion = "0.1.0-alpha.1"\n')
        (self.root / "CHANGELOG.md").write_text('# Changelog\n\n## [Unreleased]\n\n## [0.1.0-alpha.1] - 2026-11-22\n\n- Initial prototype.\n\n## [0.0.1] - 2026-11-01\n\n- Older notes.\n')

    def test_accepts_matching_tag_and_extracts_only_its_notes(self):
        self.assertEqual(release.check(self.root, "v0.1.0-alpha.1"), "0.1.0-alpha.1")
        notes = release.release_notes(self.root)
        self.assertIn("Initial prototype.", notes)
        self.assertNotIn("Older notes.", notes)
        self.assertNotIn("Unreleased", notes)

    def test_rejects_mismatched_tag(self):
        with self.assertRaisesRegex(ValueError, "must match"):
            release.check(self.root, "v0.2.0")

    def test_rejects_stale_lockfile(self):
        path = self.root / "Cargo.lock"
        path.write_text(path.read_text().replace("0.1.0-alpha.1", "0.0.1"))
        with self.assertRaisesRegex(ValueError, "stale"):
            release.check(self.root)

    def test_rejects_missing_or_empty_changelog(self):
        for text in ["## [Unreleased]\n", "## [0.1.0-alpha.1] - 2026-11-22\n\n"]:
            with self.subTest(text=text):
                (self.root / "CHANGELOG.md").write_text(text)
                with self.assertRaises(ValueError):
                    release.check(self.root)

    def test_rejects_non_inherited_crate_version(self):
        path = self.root / "crate/Cargo.toml"
        path.write_text('[package]\nname = "app"\nversion = "0.1.0-alpha.1"\n')
        with self.assertRaisesRegex(ValueError, "inherit"):
            release.check(self.root)

    def test_rejects_invalid_versions_before_use_in_asset_names(self):
        for value in ["01.0.0", "1.2", "1.2.3-alpha.01", "1.2.3/../../", "1.2.3\n", "v1.2.3"]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                release.validate_version(value)


if __name__ == "__main__":
    unittest.main()
