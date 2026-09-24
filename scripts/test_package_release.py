"""Run with: python3 -m unittest discover -s scripts -p 'test_*.py'."""

import hashlib
import os
import stat
import subprocess
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path

from package_release import PackageError, package_release, validate_tag


class PackageReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="claw-todo-package-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.skill = self.root / "skills/claw-todo"
        (self.skill / "references").mkdir(parents=True)
        (self.root / "Cargo.toml").write_text('[package]\nversion = "0.1.0"\n')
        (self.root / "LICENSE").write_text("MIT license fixture\n")
        (self.skill / "SKILL.md").write_text("---\nname: claw-todo\n---\n")
        (self.skill / "references/commands.md").write_text("commands\n")
        (self.skill / "references/protocol.md").write_text("protocol\n")
        self.binary = self.root / "claw-todo"
        self.binary.write_bytes(b"binary fixture\n")
        # The archive must mark the binary executable regardless of checkout mode.
        self.binary.chmod(0o644)
        self.output = self.root / "dist"
        self.git("init", "--quiet")
        self.git("add", "skills")

    def git(self, *arguments):
        subprocess.run(
            ["git", "-C", str(self.root), *arguments],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )

    def package(self, **overrides):
        arguments = {
            "repository_root": self.root,
            "binary": self.binary,
            "output": self.output,
            "tag": "v0.1.0",
        }
        arguments.update(overrides)
        return package_release(**arguments)

    def test_archives_and_checksums(self):
        assets = self.package()
        self.assertEqual(
            [path.name for path in assets],
            ["claw-todo-v0.1.0-darwin-arm64.tar.gz", "claw-todo-skill-v0.1.0.zip", "SHA256SUMS"],
        )
        folder = "claw-todo-v0.1.0-darwin-arm64"
        with tarfile.open(assets[0]) as archive:
            self.assertEqual(archive.getnames(), [folder, f"{folder}/claw-todo", f"{folder}/LICENSE"])
            self.assertEqual(archive.getmember(f"{folder}/claw-todo").mode, 0o755)
            self.assertEqual(archive.getmember(f"{folder}/LICENSE").mode, 0o644)
            self.assertEqual(archive.extractfile(f"{folder}/claw-todo").read(), self.binary.read_bytes())
            for entry in archive.getmembers():
                self.assertEqual((entry.mtime, entry.uid, entry.gid), (0, 0, 0))
        with zipfile.ZipFile(assets[1]) as archive:
            self.assertEqual(
                set(archive.namelist()),
                {
                    "claw-todo/SKILL.md",
                    "claw-todo/references/commands.md",
                    "claw-todo/references/protocol.md",
                    "claw-todo/LICENSE",
                },
            )
            self.assertEqual(archive.read("claw-todo/LICENSE"), (self.root / "LICENSE").read_bytes())
            self.assertEqual(archive.getinfo("claw-todo/SKILL.md").external_attr >> 16, stat.S_IFREG | 0o644)
        expected = "".join(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n" for path in assets[:2])
        self.assertEqual(assets[2].read_text(), expected)

    def test_reproducible_despite_source_timestamps(self):
        first = self.package()
        for path in [self.binary, self.root / "LICENSE", self.skill / "SKILL.md"]:
            os.utime(path, (1234567890, 1234567890))
        second = self.package(output=self.root / "another-output")
        self.assertEqual([path.read_bytes() for path in first], [path.read_bytes() for path in second])

    def test_only_tracked_non_cache_resources(self):
        (self.skill / "untracked.md").write_text("must not ship\n")
        (self.skill / ".DS_Store").write_text("must not ship\n")
        (self.skill / "__pycache__").mkdir()
        (self.skill / "__pycache__/module.pyc").write_bytes(b"must not ship")
        (self.skill / "extra.md").write_text("ship this tracked resource\n")
        script = self.skill / "helper.sh"
        script.write_text("#!/bin/sh\n")
        script.chmod(0o755)
        self.git("add", "skills/claw-todo/.DS_Store", "skills/claw-todo/__pycache__", "skills/claw-todo/extra.md", "skills/claw-todo/helper.sh")
        with zipfile.ZipFile(self.package()[1]) as archive:
            self.assertIn("claw-todo/extra.md", archive.namelist())
            self.assertIn("claw-todo/helper.sh", archive.namelist())
            self.assertNotIn("claw-todo/untracked.md", archive.namelist())
            self.assertNotIn("claw-todo/.DS_Store", archive.namelist())
            self.assertFalse(any("__pycache__" in name for name in archive.namelist()))
            self.assertEqual(archive.getinfo("claw-todo/helper.sh").external_attr >> 16, stat.S_IFREG | 0o755)

    def test_version_mismatch(self):
        with self.assertRaisesRegex(PackageError, "does not match"):
            self.package(tag="v0.2.0")
        self.assertFalse(self.output.exists())

    def test_invalid_tags(self):
        for tag in ["0.1.0", "v01.1.0", "v0.1", "v0.1.0\n", "v0.1.0-01", "v0.1.0-a..b", "v0.1.0/evil", "v0.1.0+", "v٠.1.0"]:
            with self.subTest(tag=tag), self.assertRaisesRegex(PackageError, "SemVer"):
                self.package(tag=tag)
        self.assertFalse(self.output.exists())

    def test_prerelease_and_build_metadata(self):
        for version in ["1.2.3-rc.1", "1.2.3-0", "1.2.3+build.007", "1.2.3-01abc+build.7"]:
            with self.subTest(version=version):
                (self.root / "Cargo.toml").write_text(f'[package]\nversion = "{version}"\n')
                self.assertEqual(validate_tag(self.root, f"v{version}"), version)

    def test_missing_binary(self):
        with self.assertRaisesRegex(PackageError, "required regular file"):
            self.package(binary=self.root / "missing")
        self.assertFalse(self.output.exists())

    def test_missing_license(self):
        (self.root / "LICENSE").unlink()
        with self.assertRaisesRegex(PackageError, "required regular file"):
            self.package()
        self.assertFalse(self.output.exists())

    def test_missing_reference(self):
        (self.skill / "references/protocol.md").unlink()
        with self.assertRaisesRegex(PackageError, "required regular file"):
            self.package()
        self.assertFalse(self.output.exists())

    def test_untracked_required_reference(self):
        self.git("rm", "--cached", "skills/claw-todo/references/protocol.md")
        with self.assertRaisesRegex(PackageError, "required tracked skill"):
            self.package()
        self.assertFalse(self.output.exists())

    def test_tracked_symlink_is_rejected(self):
        (self.skill / "linked.md").symlink_to(self.root / "LICENSE")
        self.git("add", "skills/claw-todo/linked.md")
        with self.assertRaisesRegex(PackageError, "cannot be symlinks"):
            self.package()
        self.assertFalse(self.output.exists())

    def test_nonempty_output_is_not_modified(self):
        self.output.mkdir()
        existing = self.output / "keep.txt"
        existing.write_text("existing data\n")
        with self.assertRaisesRegex(PackageError, "new or empty"):
            self.package()
        self.assertEqual(list(self.output.iterdir()), [existing])
        self.assertEqual(existing.read_text(), "existing data\n")

    def test_empty_output_is_allowed(self):
        self.output.mkdir()
        self.assertEqual(len(self.package()), 3)

    def test_symlink_output_is_rejected(self):
        target = self.root / "real-output"
        target.mkdir()
        self.output.symlink_to(target, target_is_directory=True)
        with self.assertRaisesRegex(PackageError, "new or empty"):
            self.package()
        self.assertEqual(list(target.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
