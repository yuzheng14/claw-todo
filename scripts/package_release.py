#!/usr/bin/env python3
"""Build reproducible release archives from a built CLI and tracked skill files."""

import argparse
import gzip
import hashlib
import re
import stat
import subprocess
import sys
import tarfile
import tomllib
import zipfile
from pathlib import Path


REPOSITORY_ROOT = Path(__file__).resolve().parent.parent
SEMVER = re.compile(
    r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
    r"(?:\.(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
)
SKILL_ROOT = Path("skills/claw-todo")
REQUIRED_SKILL_FILES = {
    Path("SKILL.md"),
    Path("references/commands.md"),
    Path("references/protocol.md"),
}


class PackageError(Exception):
    """Invalid release inputs; no release should be published."""


def validate_tag(repository_root: Path, tag: str) -> str:
    if not SEMVER.fullmatch(tag):
        raise PackageError(f"expected a SemVer tag such as v0.1.0, got {tag!r}")
    with (repository_root / "Cargo.toml").open("rb") as manifest:
        version = tomllib.load(manifest)["package"]["version"]
    if version != tag[1:]:
        raise PackageError(f"tag {tag!r} does not match Cargo.toml version {version!r}")
    return version


def require_regular_file(path: Path) -> None:
    if path.is_symlink() or not path.is_file():
        raise PackageError(f"required regular file is missing or is a symlink: {path}")


def tracked_skill_files(repository_root: Path) -> list[tuple[Path, Path]]:
    result = subprocess.run(
        ["git", "-C", str(repository_root), "ls-files", "-z", "--", str(SKILL_ROOT)],
        check=True,
        stdout=subprocess.PIPE,
    )
    files = []
    for entry in result.stdout.decode("utf-8").split("\0"):
        if not entry:
            continue
        relative = Path(entry).relative_to(SKILL_ROOT)
        if any(part.startswith(".") or part == "__pycache__" for part in relative.parts):
            continue
        if relative.suffix in {".pyc", ".pyo", ".swp", ".swo"} or relative.name.endswith("~"):
            continue
        source = repository_root / SKILL_ROOT / relative
        # Reject directory symlinks too, so the archive cannot escape the checkout.
        for parent in [source, *source.parents]:
            if parent == repository_root:
                break
            if parent.is_symlink():
                raise PackageError(f"skill resources cannot be symlinks: {parent}")
        require_regular_file(source)
        files.append((relative, source))
    missing = REQUIRED_SKILL_FILES - {relative for relative, _ in files}
    if missing:
        names = ", ".join(str(path) for path in sorted(missing))
        raise PackageError(f"required tracked skill resources are missing: {names}")
    return sorted(files)


def write_binary_archive(destination: Path, folder: str, binary: Path, license_file: Path) -> None:
    with destination.open("wb") as raw:
        # Omit the gzip filename and wall-clock timestamp for repeatable archives.
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as archive:
                directory = tarfile.TarInfo(folder)
                directory.type = tarfile.DIRTYPE
                directory.mode = 0o755
                archive.addfile(directory)
                for name, source, mode in [
                    ("claw-todo", binary, 0o755),
                    ("LICENSE", license_file, 0o644),
                ]:
                    entry = tarfile.TarInfo(f"{folder}/{name}")
                    entry.size = source.stat().st_size
                    entry.mode = mode
                    with source.open("rb") as content:
                        archive.addfile(entry, content)


def write_skill_archive(destination: Path, files: list[tuple[Path, Path]], license_file: Path) -> None:
    with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for relative, source in [*files, (Path("LICENSE"), license_file)]:
            entry = zipfile.ZipInfo(f"claw-todo/{relative.as_posix()}", date_time=(1980, 1, 1, 0, 0, 0))
            entry.create_system = 3
            mode = 0o755 if source.stat().st_mode & 0o111 else 0o644
            entry.external_attr = (stat.S_IFREG | mode) << 16
            entry.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(entry, source.read_bytes(), compresslevel=9)


def sha256(path: Path) -> str:
    with path.open("rb") as content:
        return hashlib.file_digest(content, "sha256").hexdigest()


def package_release(repository_root: Path, binary: Path, output: Path, tag: str) -> list[Path]:
    repository_root = repository_root.resolve()
    validate_tag(repository_root, tag)
    require_regular_file(binary)
    license_file = repository_root / "LICENSE"
    require_regular_file(license_file)
    files = tracked_skill_files(repository_root)
    if any(relative == Path("LICENSE") for relative, _ in files):
        raise PackageError("skill already contains LICENSE; refusing a duplicate archive entry")
    if output.is_symlink() or (output.exists() and (not output.is_dir() or any(output.iterdir()))):
        raise PackageError(f"output must be a new or empty directory: {output}")
    output.mkdir(parents=True, exist_ok=True)
    folder = f"claw-todo-{tag}-darwin-arm64"
    binary_archive = output / f"{folder}.tar.gz"
    skill_archive = output / f"claw-todo-skill-{tag}.zip"
    write_binary_archive(binary_archive, folder, binary, license_file)
    write_skill_archive(skill_archive, files, license_file)
    checksums = output / "SHA256SUMS"
    checksums.write_text(
        "".join(f"{sha256(path)}  {path.name}\n" for path in [binary_archive, skill_archive]),
        encoding="utf-8",
    )
    return [binary_archive, skill_archive, checksums]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--tag", required=True)
    args = parser.parse_args()
    try:
        assets = package_release(REPOSITORY_ROOT, args.binary, args.output, args.tag)
    except (PackageError, OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(f"release packaging failed: {error}", file=sys.stderr)
        return 1
    for asset in assets:
        print(asset)
    return 0


if __name__ == "__main__":
    sys.exit(main())
