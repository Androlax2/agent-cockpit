#!/usr/bin/env python3
"""Index and sign the torlek APT repository, and check it as apt would (docs/packaging/deb.md, #563).

Usage: repository.py index --repo DIR --suite alpha|beta
       repository.py prune --repo DIR --suite SUITE --keep N
       repository.py sign --repo DIR --suite SUITE --key FINGERPRINT [--key FINGERPRINT]
       repository.py verify --repo DIR --suite SUITE --keyring PUBLIC_KEY.asc

`index` reads every .deb under DIR/pool/main/t/torlek/ and writes the suite's Packages
indexes and Release, removing any signature of an earlier index. `prune` deletes the pool's
packages of all but the suite's N newest versions, so the site stays under GitHub Pages' 1 GB
limit; run `index` after it. `sign` writes InRelease and
Release.gpg with the caller's GnuPG keys, each named by its full fingerprint. `verify` checks the
InRelease signature against one public key, then every index and package against the signed
hashes. Nothing here creates keys, downloads or publishes.
"""

import argparse
import gzip
import hashlib
import importlib.util
import re
import subprocess
import tempfile
from datetime import UTC, datetime
from email.utils import format_datetime
from pathlib import Path

BUILD_TOOL = Path(__file__).resolve().parent / "build.py"
POOL = "pool/main/t/torlek"
COMPONENT = "main"
SUITES = ("alpha", "beta")
FINGERPRINT = re.compile(r"[0-9A-F]{40}")
DEBIAN_VERSION = re.compile(
    r"(?P<major>[0-9]+)\.(?P<minor>[0-9]+)\.(?P<patch>[0-9]+)"
    r"~(?P<channel>alpha|beta)\.(?P<number>[0-9]+)-(?P<revision>[0-9]+)"
)
SIGNATURES = ("InRelease", "Release.gpg")


class Refusal(Exception):
    pass


def build_tool():
    spec = importlib.util.spec_from_file_location("deb_build", BUILD_TOOL)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def fields_of(paragraph):
    """A deb822 paragraph's fields; continuation lines stay part of their field."""
    fields = {}
    name = None
    for line in paragraph.splitlines():
        if line[:1] in (" ", "\t") and name is not None:
            fields[name] += "\n" + line
            continue
        name, separator, value = line.partition(":")
        if not separator:
            raise Refusal(f"not a deb822 field: {line!r}")
        fields[name] = value.strip()
    return fields


def sha256_hex(contents):
    return hashlib.sha256(contents).hexdigest()


def package_stanza(repo, deb):
    build = build_tool()
    contents = deb.read_bytes()
    try:
        control = build.control_text(contents)
    except build.Refusal as error:
        raise Refusal(f"{deb.relative_to(repo)}: {error}") from error
    fields = fields_of(control)
    return fields, (
        control.rstrip("\n")
        + f"\nFilename: {deb.relative_to(repo).as_posix()}"
        + f"\nSize: {len(contents)}"
        + f"\nSHA256: {sha256_hex(contents)}\n"
    )


def refuse_misplaced(repo, deb, fields, suite):
    build = build_tool()
    expected = f"{build.PACKAGE}_{fields.get('Version')}_{fields.get('Architecture')}.deb"
    if fields.get("Package") != build.PACKAGE or deb.name != expected:
        raise Refusal(f"{deb.relative_to(repo)}: its control says {expected}")
    if fields["Architecture"] not in build.ARCHITECTURES:
        raise Refusal(
            f"{deb.relative_to(repo)}: architecture {fields['Architecture']} is not built"
        )
    version = DEBIAN_VERSION.fullmatch(fields["Version"])
    if version is None or version["channel"] != suite:
        raise Refusal(
            f"{deb.relative_to(repo)}: version {fields['Version']} is not in the {suite} channel"
        )


def packages_indexes(repo, suite):
    """Packages text per architecture. A suite missing an architecture is refused: publishing
    one half of a release would read as the whole of it."""
    build = build_tool()
    stanzas = {architecture: [] for architecture in build.ARCHITECTURES}
    for deb in sorted((repo / POOL).glob("*.deb")):
        fields, stanza = package_stanza(repo, deb)
        refuse_misplaced(repo, deb, fields, suite)
        stanzas[fields["Architecture"]].append(stanza)
    stray = [path for path in (repo / "pool").rglob("*.deb") if path.parent != repo / POOL]
    if stray:
        raise Refusal(f"{stray[0].relative_to(repo)}: outside {POOL}")
    for architecture, entries in stanzas.items():
        if not entries:
            raise Refusal(f"no {architecture} package in {POOL}")
    return {architecture: "\n".join(entries) for architecture, entries in stanzas.items()}


def version_order(version):
    """dpkg's order for versions of one channel: every part is a number."""
    parts = DEBIAN_VERSION.fullmatch(version)
    return tuple(int(parts[name]) for name in ("major", "minor", "patch", "number", "revision"))


def prune(repo, suite, keep):
    """Both architectures of a version stay or go together, and every package left must still
    belong to the suite, as `index` requires."""
    refuse_unknown_suite(suite)
    if keep < 1:
        raise Refusal(f"--keep {keep}: keep at least the newest version")
    packages_by_version = {}
    for deb in sorted((repo / POOL).glob("*.deb")):
        fields, _ = package_stanza(repo, deb)
        refuse_misplaced(repo, deb, fields, suite)
        packages_by_version.setdefault(fields["Version"], []).append(deb)
    newest_first = sorted(packages_by_version, key=version_order, reverse=True)
    removed = [deb for version in newest_first[keep:] for deb in packages_by_version[version]]
    for deb in removed:
        deb.unlink()
    return newest_first[keep:]


def release_file(suite, architectures, indexes, now):
    """No Valid-Until: it would need the signing key online to re-sign on a schedule
    (docs/decisions/2026-11-03-563-deb-apt-packaging.md)."""
    lines = [
        "Origin: Torlek",
        "Label: Torlek",
        f"Suite: {suite}",
        f"Codename: {suite}",
        f"Date: {format_datetime(now, usegmt=True)}",
        f"Architectures: {' '.join(architectures)}",
        f"Components: {COMPONENT}",
        f"Description: Torlek {suite} packages, built from published release archives",
        "SHA256:",
    ]
    lines.extend(
        f" {sha256_hex(contents)} {len(contents):>16} {path}"
        for path, contents in sorted(indexes.items())
    )
    return "\n".join(lines) + "\n"


def refuse_unknown_suite(suite):
    if suite not in SUITES:
        raise Refusal(f"suite {suite!r}: one of {', '.join(SUITES)}")


def index(repo, suite, now=None):
    refuse_unknown_suite(suite)
    texts = packages_indexes(repo, suite)
    indexes = {}
    for architecture, text in texts.items():
        directory = f"{COMPONENT}/binary-{architecture}"
        indexes[f"{directory}/Packages"] = text.encode()
        indexes[f"{directory}/Packages.gz"] = gzip.compress(text.encode(), mtime=0)
    dists = repo / "dists" / suite
    for name in SIGNATURES:
        (dists / name).unlink(missing_ok=True)
    for path, contents in indexes.items():
        (dists / path).parent.mkdir(parents=True, exist_ok=True)
        (dists / path).write_bytes(contents)
    release = release_file(suite, sorted(texts), indexes, now or datetime.now(UTC))
    (dists / "Release").write_text(release)
    return dists / "Release"


def run_gpg(program, *arguments):
    try:
        result = subprocess.run(
            [program, *arguments], capture_output=True, text=True, check=False, timeout=60
        )
    except FileNotFoundError as error:
        raise Refusal(f"{program} is missing: install GnuPG to sign or verify") from error
    return result


def sign(repo, suite, keys):
    """Two keys sign during a rotation, so clients holding either one keep verifying."""
    refuse_unknown_suite(suite)
    if not keys:
        raise Refusal("no signing key given")
    for key in keys:
        if not FINGERPRINT.fullmatch(key):
            raise Refusal(f"key {key!r}: give the full 40-character fingerprint in capitals")
    dists = repo / "dists" / suite
    release = dists / "Release"
    if not release.is_file():
        raise Refusal(f"{release} is missing; run index first")
    signing = ["--batch", "--armor", "--digest-algo", "SHA512"]
    for key in keys:
        signing += ["--local-user", f"{key}!"]
    for name, mode in (("InRelease", "--clearsign"), ("Release.gpg", "--detach-sign")):
        partial = dists / f".{name}.partial"
        partial.unlink(missing_ok=True)
        result = run_gpg("gpg", *signing, mode, "--output", str(partial), str(release))
        if result.returncode != 0:
            partial.unlink(missing_ok=True)
            raise Refusal(f"gpg {mode} failed: {result.stderr.strip()}")
        partial.replace(dists / name)


def signed_release(dists, keyring, work):
    """The Release text InRelease signs, read only from gpgv's output: text outside the
    signature never counts."""
    binary_keyring = work / "keyring.gpg"
    result = run_gpg("gpg", "--batch", "--dearmor", "--output", str(binary_keyring), str(keyring))
    if result.returncode != 0:
        raise Refusal(f"{keyring}: not an OpenPGP public key: {result.stderr.strip()}")
    release = work / "Release"
    for arguments in (
        ["--output", str(release), str(dists / "InRelease")],
        [str(dists / "Release.gpg"), str(dists / "Release")],
    ):
        result = run_gpg("gpgv", "--status-fd", "1", "--keyring", str(binary_keyring), *arguments)
        if not is_trusted_signature(result.stdout):
            raise Refusal(f"{arguments[-1]}: signature refused: {result.stderr.strip()}")
    return release.read_text()


def is_trusted_signature(status):
    """apt's rule (methods/gpgv.cc), not gpgv's exit code: one good signature by a key in the
    keyring, and no bad, expired or revoked one. A signature by a key the client lacks is
    ignored, which is what lets a rotation sign with the old and the new key."""
    keywords = [line.split()[1:] for line in status.splitlines() if line.startswith("[GNUPG:] ")]
    refusing = {"BADSIG", "EXPSIG", "EXPKEYSIG", "REVKEYSIG"}
    if any(words and words[0] in refusing for words in keywords):
        return False
    # ERRSIG's sixth argument is the reason; 9 is "no public key".
    if any(words[0] == "ERRSIG" and words[6:7] != ["9"] for words in keywords if words):
        return False
    return any(words and words[0] == "VALIDSIG" for words in keywords)


def refuse_hash_mismatch(path, size, digest, label):
    if not path.is_file():
        raise Refusal(f"{label}: missing")
    contents = path.read_bytes()
    if len(contents) != size or sha256_hex(contents) != digest:
        raise Refusal(f"{label}: size or SHA256 differs from the signed index")


def listed_hashes(release_fields):
    listed = {}
    for line in release_fields.get("SHA256", "").splitlines():
        if not line.strip():
            continue
        digest, size, path = line.split()
        listed[path] = (int(size), digest)
    return listed


def verify(repo, suite, keyring):
    refuse_unknown_suite(suite)
    dists = repo / "dists" / suite
    with tempfile.TemporaryDirectory(prefix="torlek-apt-") as folder:
        release_fields = fields_of(signed_release(dists, keyring, Path(folder)))
    if release_fields.get("Suite") != suite:
        raise Refusal(f"signed Release is for suite {release_fields.get('Suite')!r}, not {suite}")
    listed = listed_hashes(release_fields)
    for path, (size, digest) in sorted(listed.items()):
        refuse_hash_mismatch(dists / path, size, digest, f"dists/{suite}/{path}")
    checked = 0
    for architecture in release_fields["Architectures"].split():
        path = f"{COMPONENT}/binary-{architecture}/Packages"
        if path not in listed:
            raise Refusal(f"signed Release lists no {path}")
        for stanza in (dists / path).read_text().split("\n\n"):
            fields = fields_of(stanza)
            missing = sorted({"Filename", "Size", "SHA256"} - fields.keys())
            if missing:
                raise Refusal(f"{path}: a stanza has no {', '.join(missing)}")
            label = f"{fields['Filename']} (from {path})"
            refuse_hash_mismatch(
                repo / fields["Filename"], int(fields["Size"]), fields["SHA256"], label
            )
            checked += 1
    return checked


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("index", "prune", "sign", "verify"):
        command = commands.add_parser(name)
        command.add_argument("--repo", type=Path, required=True)
        command.add_argument("--suite", required=True)
        if name == "prune":
            command.add_argument("--keep", type=int, required=True)
        if name == "sign":
            command.add_argument("--key", action="append", required=True)
        if name == "verify":
            command.add_argument("--keyring", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        if arguments.command == "index":
            print(f"Wrote {index(arguments.repo, arguments.suite)}; sign it before publishing")
        elif arguments.command == "prune":
            removed = prune(arguments.repo, arguments.suite, arguments.keep)
            print(f"Removed {', '.join(removed) or 'no version'}; index again")
        elif arguments.command == "sign":
            sign(arguments.repo, arguments.suite, arguments.key)
            print(f"Signed dists/{arguments.suite}/InRelease and Release.gpg")
        else:
            checked = verify(arguments.repo, arguments.suite, arguments.keyring)
            print(f"Signature and hashes hold for {checked} packages in {arguments.suite}")
    except (OSError, ValueError, subprocess.TimeoutExpired, Refusal) as error:
        parser.exit(1, f"repository.py: {error}\n")


if __name__ == "__main__":
    main()
