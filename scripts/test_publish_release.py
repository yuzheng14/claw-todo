"""Offline release publication tests. No GitHub network calls are made."""

import contextlib
import copy
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import quote

import publish_release as publisher


class PublishTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.tag = "v0.1.0"
        self.repo = "example/claw-todo"
        self.calls = []
        self.release = None
        self.remote_bytes = {}
        self.fail_upload = None
        self.corrupt_upload = None
        self.make_assets()
        self.mock = patch.object(publisher, "gh", side_effect=self.fake_gh).start()
        self.addCleanup(patch.stopall)

    def make_assets(self):
        self.names = [f"claw-todo-{self.tag}-darwin-arm64.tar.gz", f"claw-todo-skill-{self.tag}.zip", "SHA256SUMS"]
        self.contents = {self.names[0]: b"binary archive", self.names[1]: b"skill archive"}
        self.contents["SHA256SUMS"] = "".join(
            f"{hashlib.sha256(data).hexdigest()}  {name}\n" for name, data in self.contents.items()
        ).encode()
        for name, data in self.contents.items():
            (self.directory / name).write_bytes(data)

    def existing(self, draft=False, immutable=False, missing=()):
        self.release = {"draft": draft, "immutable": immutable, "assets": [], "body": "Human notes", "prerelease": False}
        for name in self.names:
            if name not in missing:
                self.remote_bytes[name] = self.contents[name]
                self.release["assets"].append(self.asset(name))

    def asset(self, name):
        return {"name": name, "state": "uploaded", "digest": f"sha256:{hashlib.sha256(self.remote_bytes[name]).hexdigest()}"}

    def fake_gh(self, *args, missing_ok=False):
        self.calls.append(args)
        if args[:2] == ("api", "graphql"):
            self.assertIn(f"owner={self.repo.split('/')[0]}", args)
            self.assertIn(f"name={self.repo.split('/')[1]}", args)
            self.assertIn(f"tag={self.tag}", args)
            node = {"databaseId": 42} if self.release is not None else None
            return json.dumps({"data": {"repository": {"release": node}}})
        if args == ("api", f"repos/{self.repo}/releases/tags/{quote(self.tag, safe='')}"):
            if self.release is None or self.release["draft"]:
                if missing_ok:
                    return None
                raise publisher.ReleaseError("Release not found")
            return json.dumps(self.release)
        if args == ("api", f"repos/{self.repo}/releases/42"):
            self.assertIsNotNone(self.release)
            return json.dumps(self.release)
        if args[:2] == ("release", "create"):
            self.release = {"draft": True, "assets": []}
        elif args[:2] == ("release", "upload"):
            path = Path(args[3])
            if path.name == self.fail_upload:
                raise publisher.ReleaseError("upload interrupted")
            self.remote_bytes[path.name] = b"corrupted" if path.name == self.corrupt_upload else path.read_bytes()
            self.release["assets"] = [asset for asset in self.release["assets"] if asset["name"] != path.name]
            self.release["assets"].append(self.asset(path.name))
        elif args[:2] == ("release", "download"):
            name = args[args.index("--pattern") + 1]
            directory = Path(args[args.index("--dir") + 1])
            (directory / name).write_bytes(self.remote_bytes[name])
        elif args[:2] == ("release", "edit"):
            self.release["draft"] = False
        else:
            self.fail(f"Unexpected gh call: {args}")
        return ""

    def publish(self, **kwargs):
        with contextlib.redirect_stdout(io.StringIO()):
            publisher.publish(self.repo, self.tag, self.directory, **kwargs)

    def writes(self):
        return [call for call in self.calls if call[:2] in (("release", "create"), ("release", "upload"), ("release", "edit"))]

    def uploads(self):
        return [call for call in self.calls if call[:2] == ("release", "upload")]

    def test_new_stable_release_is_draft_until_all_assets_are_verified(self):
        self.publish()
        create = self.writes()[0]
        self.assertIn("--verify-tag", create)
        self.assertIn("--draft", create)
        self.assertIn("--generate-notes", create)
        self.assertNotIn("--latest", " ".join(create))
        self.assertEqual([Path(call[3]).name for call in self.uploads()], self.names)
        self.assertEqual(self.calls[-2][0], "api")
        self.assertEqual(self.calls[-1][-1], "--draft=false")
        self.assertFalse(self.release["draft"])

    def test_new_prerelease_does_not_become_latest(self):
        for path in self.directory.iterdir():
            path.unlink()
        self.tag = "v0.2.0-rc.1"
        self.make_assets()
        self.publish()
        self.assertIn("--prerelease", self.writes()[0])
        self.assertIn("--latest=false", self.writes()[0])

    def test_build_metadata_hyphen_is_not_a_prerelease(self):
        for path in self.directory.iterdir():
            path.unlink()
        self.tag = "v0.2.0+build-arm64"
        self.make_assets()
        self.publish()
        self.assertNotIn("--prerelease", self.writes()[0])
        self.assertNotIn("--latest=false", self.writes()[0])

    def test_upload_failure_leaves_draft_unpublished(self):
        self.fail_upload = self.names[1]
        with self.assertRaisesRegex(publisher.ReleaseError, "interrupted"):
            self.publish()
        self.assertTrue(self.release["draft"])
        self.assertFalse(any(call[:2] == ("release", "edit") for call in self.calls))

    def test_corrupt_upload_leaves_draft_unpublished(self):
        self.corrupt_upload = self.names[1]
        with self.assertRaisesRegex(publisher.ReleaseError, "verification failed"):
            self.publish()
        self.assertTrue(self.release["draft"])

    def test_existing_same_published_release_has_no_writes(self):
        self.existing()
        original = copy.deepcopy(self.release)
        self.publish()
        self.assertEqual(self.release, original)
        self.assertEqual(self.writes(), [])

    def test_existing_draft_is_resumed_and_published(self):
        self.existing(draft=True, missing=self.names[1:])
        self.publish()
        self.assertEqual([Path(call[3]).name for call in self.uploads()], self.names[1:])
        self.assertFalse(self.release["draft"])
        self.assertEqual(self.release["body"], "Human notes")
        self.assertTrue(any(call[:2] == ("api", "graphql") for call in self.calls))
        self.assertIn(("api", f"repos/{self.repo}/releases/42"), self.calls)
        self.assertFalse(any(call[:2] == ("release", "create") for call in self.calls))

    def test_conflict_preflight_prevents_even_earlier_missing_asset_upload(self):
        self.existing(missing=[self.names[0]])
        self.release["assets"][-1]["digest"] = "sha256:" + "0" * 64
        with self.assertRaisesRegex(publisher.ReleaseError, "differs"):
            self.publish()
        self.assertEqual(self.writes(), [])

    def test_replace_opt_in_only_clobbers_different_asset(self):
        self.existing(missing=[self.names[0]])
        self.release["assets"][-1]["digest"] = "sha256:" + "0" * 64
        self.publish(replace_assets=True)
        self.assertNotIn("--clobber", self.uploads()[0])
        self.assertIn("--clobber", self.uploads()[1])
        self.assertEqual(Path(self.uploads()[1][3]).name, "SHA256SUMS")
        self.assertEqual(self.release["body"], "Human notes")
        self.assertFalse(any(call[:2] == ("release", "edit") for call in self.calls))

    def test_immutable_release_refuses_missing_and_different_assets(self):
        for missing in ([], [self.names[0]]):
            with self.subTest(missing=missing):
                self.calls.clear()
                self.existing(immutable=True, missing=missing)
                self.release["assets"][-1]["digest"] = "sha256:" + "0" * 64
                with self.assertRaisesRegex(publisher.ReleaseError, "immutable"):
                    self.publish(replace_assets=True)
                self.assertEqual(self.writes(), [])

    def test_identical_immutable_release_is_idempotent(self):
        self.existing(immutable=True)
        self.publish()
        self.assertEqual(self.writes(), [])

    def test_missing_digest_downloads_and_compares_exact_asset(self):
        self.existing()
        self.release["assets"][0].pop("digest")
        self.publish()
        downloads = [call for call in self.calls if call[:2] == ("release", "download")]
        self.assertEqual(len(downloads), 2)
        self.assertEqual(downloads[0][downloads[0].index("--pattern") + 1], self.names[0])
        self.assertEqual(self.writes(), [])

    def test_incomplete_remote_asset_is_not_accepted_by_digest(self):
        self.existing()
        self.release["assets"][0]["state"] = "new"
        with self.assertRaisesRegex(publisher.ReleaseError, "differs"):
            self.publish()
        self.assertEqual(self.writes(), [])

    def test_bad_checksum_fails_before_any_github_call(self):
        (self.directory / self.names[0]).write_bytes(b"tampered")
        with self.assertRaisesRegex(publisher.ReleaseError, "does not match"):
            self.publish()
        self.assertEqual(self.calls, [])

    def test_unexpected_file_is_rejected(self):
        (self.directory / "extra.txt").write_text("extra")
        with self.assertRaisesRegex(publisher.ReleaseError, "exactly"):
            self.publish()
        self.assertEqual(self.calls, [])

    def test_symlink_is_rejected(self):
        path = self.directory / self.names[0]
        path.unlink()
        path.symlink_to(self.directory / self.names[1])
        with self.assertRaisesRegex(publisher.ReleaseError, "symlinks"):
            self.publish()
        self.assertEqual(self.calls, [])

    def test_invalid_tags_are_rejected_before_network(self):
        for tag in ("main", "v01.2.3", "v1.2.3-01", "v1.2.3;echo hi", "v1.2.3-rc..1"):
            with self.subTest(tag=tag):
                self.tag = tag
                with self.assertRaisesRegex(publisher.ReleaseError, "SemVer"):
                    self.publish()
        self.assertEqual(self.calls, [])

    def test_api_authentication_error_does_not_create_a_release(self):
        self.mock.side_effect = publisher.ReleaseError("HTTP 403")
        with self.assertRaisesRegex(publisher.ReleaseError, "403"):
            self.publish()
        self.assertEqual(self.mock.call_count, 1)

    def test_graphql_failure_does_not_create_release(self):
        for failure in (publisher.ReleaseError("GraphQL HTTP 403"), '{"errors":[{"message":"denied"}]}', "[]"):
            with self.subTest(failure=failure):
                self.mock.reset_mock()
                self.mock.side_effect = [None, failure]
                with self.assertRaises(publisher.ReleaseError):
                    self.publish()
                self.assertEqual(self.mock.call_count, 2)
                self.assertEqual(self.mock.call_args_list[-1].args[:2], ("api", "graphql"))


class GhTests(unittest.TestCase):
    @patch.object(publisher.subprocess, "run")
    def test_404_requires_null_graphql_release_to_confirm_absence(self, run):
        missing = subprocess.CompletedProcess([], 1, "", "gh: Not Found (HTTP 404)\n")
        null_release = subprocess.CompletedProcess([], 0, '{"data":{"repository":{"release":null}}}', "")
        run.side_effect = [missing, null_release]
        self.assertIsNone(publisher.get_release("owner/repo", "v1.0.0", missing_ok=True))
        self.assertEqual(run.call_args.args[0][:3], ["gh", "api", "graphql"])
        run.side_effect = [missing, null_release]
        with self.assertRaisesRegex(publisher.ReleaseError, "not found"):
            publisher.get_release("owner/repo", "v1.0.0")

    @patch.object(publisher.subprocess, "run")
    def test_non_404_rest_errors_do_not_fall_back(self, run):
        for code in (401, 403, 429, 500):
            run.reset_mock()
            run.return_value = subprocess.CompletedProcess([], 1, "", f"gh: Error (HTTP {code})")
            with self.assertRaisesRegex(publisher.ReleaseError, str(code)):
                publisher.get_release("owner/repo", "v1.0.0", missing_ok=True)
            self.assertEqual(run.call_count, 1)

    @patch.object(publisher.subprocess, "run")
    def test_draft_lookup_fetches_rest_release_by_database_id(self, run):
        draft = {"assets": [], "draft": True}
        run.side_effect = [
            subprocess.CompletedProcess([], 1, "", "gh: Not Found (HTTP 404)"),
            subprocess.CompletedProcess([], 0, '{"data":{"repository":{"release":{"databaseId":42}}}}', ""),
            subprocess.CompletedProcess([], 0, json.dumps(draft), ""),
        ]
        self.assertEqual(publisher.get_release("owner/repo", "v1.0.0"), draft)
        self.assertEqual(run.call_args.args[0], ["gh", "api", "repos/owner/repo/releases/42"])

    @patch.object(publisher.subprocess, "run")
    def test_malformed_graphql_payloads_do_not_mean_absence(self, run):
        for payload in ("[]", "{}", '{"data":{"repository":null}}',
                        '{"data":{"repository":{"release":{}}}}',
                        '{"data":{"repository":{"release":{"databaseId":true}}}}',
                        '{"errors":[{"message":"denied"}],"data":{"repository":{"release":null}}}'):
            with self.subTest(payload=payload):
                run.side_effect = [
                    subprocess.CompletedProcess([], 1, "", "gh: Not Found (HTTP 404)"),
                    subprocess.CompletedProcess([], 0, payload, ""),
                ]
                with self.assertRaises(publisher.ReleaseError):
                    publisher.get_release("owner/repo", "v1.0.0", missing_ok=True)

    @patch.object(publisher.subprocess, "run")
    def test_graphql_auth_error_and_id_lookup_404_are_not_absence(self, run):
        missing = subprocess.CompletedProcess([], 1, "", "gh: Not Found (HTTP 404)")
        found = subprocess.CompletedProcess([], 0, '{"data":{"repository":{"release":{"databaseId":42}}}}', "")
        for responses in ([missing, subprocess.CompletedProcess([], 1, "", "gh: Forbidden (HTTP 403)")],
                          [missing, found, missing]):
            run.side_effect = responses
            with self.assertRaises(publisher.ReleaseError):
                publisher.get_release("owner/repo", "v1.0.0", missing_ok=True)

    @patch.object(publisher.subprocess, "run")
    def test_malformed_api_payload_fails(self, run):
        for payload in ("not JSON", "[]", '{"assets": []}'):
            run.return_value = subprocess.CompletedProcess([], 0, payload, "")
            with self.assertRaises(publisher.ReleaseError):
                publisher.get_release("owner/repo", "v1.0.0", missing_ok=True)


if __name__ == "__main__":
    unittest.main()
