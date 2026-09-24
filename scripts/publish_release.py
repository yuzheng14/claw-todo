#!/usr/bin/env python3
"""Publish verified release assets; replacing existing bytes requires explicit opt-in."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from urllib.parse import quote


class ReleaseError(RuntimeError):
    pass


def gh(*args, missing_ok=False):
    result = subprocess.run(["gh", *args], capture_output=True, text=True, check=False)
    if result.returncode:
        if missing_ok and re.search(r"\(HTTP 404\)", result.stderr):
            return None
        raise ReleaseError(f"gh {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout


def get_release(repo, tag, missing_ok=False):
    output = gh("api", f"repos/{repo}/releases/tags/{quote(tag, safe='')}", missing_ok=True)
    if output is None:
        # The tag endpoint only promises published releases. Resolve a draft's ID via GraphQL.
        owner, name = repo.split("/", 1)
        query = "query($owner:String!,$name:String!,$tag:String!){repository(owner:$owner,name:$name){release(tagName:$tag){databaseId}}}"
        draft = gh("api", "graphql", "-f", f"query={query}", "-f", f"owner={owner}",
                   "-f", f"name={name}", "-f", f"tag={tag}")
        try:
            payload = json.loads(draft)
            if payload.get("errors"):
                raise ReleaseError("GitHub GraphQL release lookup returned errors")
            node = payload["data"]["repository"]["release"]
        except (json.JSONDecodeError, KeyError, TypeError, AttributeError) as error:
            raise ReleaseError("GitHub returned invalid GraphQL release metadata") from error
        if node is None:
            if missing_ok:
                return None
            raise ReleaseError(f"Release {repo}@{tag} not found")
        if not isinstance(node, dict) or type(node.get("databaseId")) is not int or node["databaseId"] <= 0:
            raise ReleaseError("GitHub returned invalid release database ID")
        output = gh("api", f"repos/{repo}/releases/{node['databaseId']}")
    try:
        release = json.loads(output)
    except json.JSONDecodeError as error:
        raise ReleaseError("GitHub returned invalid release JSON") from error
    if not isinstance(release, dict) or not isinstance(release.get("assets"), list):
        raise ReleaseError("GitHub returned invalid release metadata")
    if not isinstance(release.get("draft"), bool):
        raise ReleaseError("GitHub returned no release draft status")
    return release


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def validate_inputs(repo, tag, directory):
    if not re.fullmatch(r"[A-Za-z0-9_-][A-Za-z0-9_.-]*/[A-Za-z0-9_-][A-Za-z0-9_.-]*", repo):
        raise ReleaseError("--repo must be OWNER/REPO")
    semver = re.fullmatch(
        r"v(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)"
        r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
        r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?", tag,
    )
    if not semver or any(
        part.isdigit() and len(part) > 1 and part.startswith("0")
        for part in (semver.group(1) or "").split(".")
    ):
        raise ReleaseError("--tag must be a v-prefixed SemVer, for example v0.1.0")
    names = [f"claw-todo-{tag}-darwin-arm64.tar.gz", f"claw-todo-skill-{tag}.zip", "SHA256SUMS"]
    if not directory.is_dir() or {path.name for path in directory.iterdir()} != set(names):
        raise ReleaseError("Asset directory must contain exactly the two versioned archives and SHA256SUMS")
    if any(not (directory / name).is_file() or (directory / name).is_symlink() for name in names):
        raise ReleaseError("All release assets must be regular files, not symlinks")
    digests = {name: sha256(directory / name) for name in names}
    checksums = {}
    try:
        lines = (directory / "SHA256SUMS").read_text(encoding="utf-8").splitlines()
    except UnicodeError as error:
        raise ReleaseError("SHA256SUMS must be UTF-8 text") from error
    for line in lines:
        match = re.fullmatch(r"([0-9a-f]{64})  (.+)", line)
        if not match or match.group(2) in checksums:
            raise ReleaseError("Invalid or duplicate SHA256SUMS entry")
        checksums[match.group(2)] = match.group(1)
    if checksums != {name: digests[name] for name in names[:-1]}:
        raise ReleaseError("SHA256SUMS does not match the release archives")
    return names, digests, bool(semver.group(1))


def asset_map(release):
    assets = {}
    for asset in release["assets"]:
        if not isinstance(asset, dict) or not isinstance(asset.get("name"), str):
            raise ReleaseError("GitHub returned invalid asset metadata")
        if asset["name"] in assets:
            raise ReleaseError(f"Duplicate remote asset: {asset['name']}")
        assets[asset["name"]] = asset
    return assets


def remote_digest(repo, tag, name, asset):
    digest = asset.get("digest")
    if isinstance(digest, str) and re.fullmatch(r"sha256:[0-9a-f]{64}", digest):
        return digest.removeprefix("sha256:")
    # Older assets may lack API digests. Download only this exact asset to compare bytes.
    with tempfile.TemporaryDirectory(prefix="claw-todo-release-") as temporary:
        gh("release", "download", tag, "--repo", repo, "--pattern", name, "--dir", temporary)
        path = Path(temporary) / name
        if not path.is_file():
            raise ReleaseError(f"GitHub did not provide asset {name}")
        return sha256(path)


def publish(repo, tag, directory, replace_assets=False):
    names, digests, prerelease = validate_inputs(repo, tag, directory)
    release = get_release(repo, tag, missing_ok=True)
    if release is None:
        args = ["release", "create", tag, "--repo", repo, "--verify-tag", "--draft",
                "--generate-notes", "--title", f"claw-todo {tag}"]
        if prerelease:
            args.extend(["--prerelease", "--latest=false"])
        gh(*args)
        release = get_release(repo, tag)

    existing = asset_map(release)
    uploads = []
    # Finish all comparisons before any upload, so a later conflict cannot partially update assets.
    for name in names:
        asset = existing.get(name)
        if asset and asset.get("state") == "uploaded" and remote_digest(repo, tag, name, asset) == digests[name]:
            continue
        if release.get("immutable", False):
            raise ReleaseError("This release is immutable; publish a new version instead")
        if asset and not replace_assets:
            raise ReleaseError(f"Remote asset {name} differs; publish a new version or explicitly use --replace-assets")
        uploads.append((name, asset is not None))

    for name, replace in uploads:
        args = ["release", "upload", tag, str(directory / name), "--repo", repo]
        if replace:
            args.append("--clobber")
        gh(*args)

    verified = get_release(repo, tag)
    uploaded = asset_map(verified)
    for name in names:
        asset = uploaded.get(name)
        if not asset or asset.get("state") != "uploaded" or remote_digest(repo, tag, name, asset) != digests[name]:
            raise ReleaseError(f"Post-upload verification failed for {name}; draft will not be published")
    if verified["draft"]:
        gh("release", "edit", tag, "--repo", repo, "--draft=false")
    print(f"Verified release {repo}@{tag}: {len(uploads)} asset(s) uploaded")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--assets", required=True, type=Path)
    parser.add_argument("--replace-assets", action="store_true", help="Allow non-atomic replacement of different existing assets")
    args = parser.parse_args()
    try:
        publish(args.repo, args.tag, args.assets.resolve(), args.replace_assets)
    except (ReleaseError, OSError) as error:
        print(f"Release failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
