#!/usr/bin/env python3
"""Build the torlek .deb for one published release archive (docs/packaging/deb.md, #563).

Usage: build.py --tag 0.1.0-alpha.1 --sums SHA256SUMS --archive ARCHIVE
                --maintainer 'Name <email>' --date YYYY-MM-DD --out DIR [--debian-revision N]

ARCHIVE is one of the release's two Linux archives; its name gives the Debian architecture
(linux-x86_64 is amd64, linux-aarch64 is arm64). It must match its line in the release's
SHA256SUMS and its unpacked contents its own manifest. The whole bundle goes to
/usr/lib/torlek, with /usr/bin/torlek a symlink into it. The masks helper's systemd units the
bundle carries (#390) go to /usr/lib/systemd/system too, and with them come the maintainer
scripts that enable its socket and disable it on removal; a bundle from before them gets
neither. Owners and timestamps are fixed, so the same inputs give the same bytes.
Nothing here downloads, installs, signs or publishes.
"""

import argparse
import gzip
import hashlib
import importlib.util
import io
import json
import lzma
import re
import tarfile
import tempfile
from datetime import UTC, date, datetime
from email.utils import format_datetime
from pathlib import Path, PurePosixPath

PACKAGING = Path(__file__).resolve().parent / "torlek"
RELEASE_TOOL = Path(__file__).resolve().parents[2] / "tools/release.py"
PACKAGE = "torlek"
# The Alpha1 plan's archives (tools/release.py ALPHA1_PLATFORMS), keyed by dpkg's architecture.
ARCHITECTURES = {"amd64": "linux-x86_64", "arm64": "linux-aarch64"}
BUNDLE_DIRECTORY = "usr/lib/torlek"
DOCUMENTATION_DIRECTORY = f"usr/share/doc/{PACKAGE}"
PUBLIC_COMMAND = "usr/bin/torlek"
PUBLIC_COMMAND_TARGET = "../lib/torlek/bin/torlek"
SYSTEMD_UNIT_DIRECTORY = "usr/lib/systemd/system"
MAINTAINER_SCRIPTS = ("postinst", "prerm", "postrm")
SUMS_LINE = re.compile(r"([0-9a-f]{64})  (\S+)")
# Debian policy 5.6.2: a name and an RFC 5322 address, which a package cannot leave out.
MAINTAINER = re.compile(r"[^<>,\n]+ <[^<>@\s]+@[^<>@\s]+>")
AR_MAGIC = b"!<arch>\n"
DEB_MEMBERS = ["debian-binary", "control.tar.xz", "data.tar.xz"]
DEB_FORMAT = b"2.0\n"


class Refusal(Exception):
    pass


def release_tool():
    spec = importlib.util.spec_from_file_location("release", RELEASE_TOOL)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def tag_channel(tag):
    release = release_tool()
    try:
        channel = release.parse_tag(f"v{tag}")["channel"]
    except release.Refusal as error:
        raise Refusal(f"{tag!r}: not a release tag, given without its v: {error}") from error
    if channel == "stable":
        raise Refusal(f"{tag!r}: no stable release exists to package")
    return channel


def debian_version(tag, debian_revision=1):
    """dpkg sorts `~` before everything, even the end of the string, so `0.1.0~alpha.1` comes
    before `0.1.0~beta.1` before `0.1.0`. A hyphen would end the upstream version, and
    `0.1.0+alpha.1` or `0.1.0.alpha.1` would sort after 0.1.0. The revision counts packaging-only
    rebuilds of the same archive."""
    tag_channel(tag)
    if debian_revision < 1:
        raise Refusal(f"Debian revision {debian_revision}: starts at 1")
    return f"{tag.replace('-', '~', 1)}-{debian_revision}"


def archive_checksums(tag, sums_text):
    checksums = {}
    for number, line in enumerate(sums_text.splitlines(), start=1):
        match = SUMS_LINE.fullmatch(line)
        if not match:
            raise Refusal(f"SHA256SUMS line {number}: not `<sha256>  <file>`: {line!r}")
        digest, name = match.groups()
        if name in checksums:
            raise Refusal(f"SHA256SUMS: {name} is listed twice")
        checksums[name] = digest
    selected = {}
    for platform in ARCHITECTURES.values():
        name = f"torlek-v{tag}-{platform}.tar.gz"
        if name not in checksums:
            raise Refusal(f"SHA256SUMS has no {name}: is it the v{tag} release's file?")
        selected[name] = checksums[name]
    return selected


def listed_archive(tag, sums_text, archive):
    """The architecture of an archive the release's SHA256SUMS lists with exactly these bytes."""
    checksums = archive_checksums(tag, sums_text)
    architectures_by_name = {
        f"torlek-v{tag}-{platform}.tar.gz": (architecture, platform)
        for architecture, platform in ARCHITECTURES.items()
    }
    if archive.name not in architectures_by_name:
        raise Refusal(f"{archive.name}: not one of the v{tag} release's Linux archives")
    architecture, platform = architectures_by_name[archive.name]
    with open(archive, "rb") as contents:
        digest = hashlib.file_digest(contents, "sha256").hexdigest()
    if digest != checksums[archive.name]:
        raise Refusal(f"{archive.name}: sha256 {digest} does not match SHA256SUMS")
    return architecture, platform


def unpack_bundle(archive, top_directory, destination):
    with tarfile.open(archive, "r:gz") as bundle:
        for member in bundle.getmembers():
            parts = PurePosixPath(member.name).parts
            if member.name.startswith("/") or ".." in parts or parts[:1] != (top_directory,):
                raise Refusal(f"{archive.name}: {member.name!r} is outside {top_directory}/")
            if not (member.isfile() or member.isdir()):
                raise Refusal(f"{archive.name}: {member.name!r} is not a file or a directory")
        bundle.extractall(destination, filter="data")
    return destination / top_directory


def refuse_foreign_bundle(bundle, tag, channel, platform):
    """The archive's own manifest must name this release and still describe every file. Which
    programs a bundle holds is the rule of the release that made it, checked when it was cut:
    an earlier release predates programs this checkout's release.py now requires. Each helper
    it lists must still be in it and carry the bundle's version."""
    release = release_tool()
    manifest_file = bundle / release.MANIFEST_PATH
    if not manifest_file.is_file():
        raise Refusal(f"{release.MANIFEST_PATH}: no bundle manifest in the archive")
    manifest = json.loads(manifest_file.read_text())
    expected = {
        "schema": release.MANIFEST_SCHEMA,
        "name": PACKAGE,
        "version": tag,
        "channel": channel,
        "platform": platform,
    }
    problems = [
        f"{field} is {manifest.get(field)!r}, expected {value!r}"
        for field, value in expected.items()
        if manifest.get(field) != value
    ]
    if problems:
        raise Refusal(f"bundle manifest: {'; '.join(problems)}")
    listed = manifest.get("contents")
    if not isinstance(listed, dict):
        raise Refusal("bundle manifest: no contents")
    problems = []
    for relative, expected in sorted(listed.items()):
        path = bundle / relative
        if not path.is_file():
            problems.append(f"{relative}: missing")
        elif release.sha256_of(path) != expected:
            problems.append(f"{relative}: checksum differs from the manifest")
    problems.extend(
        f"{relative}: not listed in the manifest"
        for relative in release.bundle_files(bundle)
        if relative not in listed
    )
    helpers = manifest.get("helpers")
    if not isinstance(helpers, dict) or not helpers:
        problems.append("helpers: none listed")
    else:
        problems.extend(
            f"helpers: {helper} is {version!r}, expected {tag!r}"
            for helper, version in sorted(helpers.items())
            if version != tag
        )
        problems.extend(
            f"helpers: bin/{helper} is not in the bundle"
            for helper in sorted(helpers)
            if f"bin/{helper}" not in listed
        )
    if problems:
        raise Refusal(f"bundle contents differ from its manifest: {'; '.join(problems)}")


def changelog(version, channel, tag, maintainer, release_date):
    """Debian policy 12.7 wants a changelog in every binary package; this one names the
    archive it repackages. The suite is the release channel (docs/packaging/deb.md)."""
    stamp = format_datetime(datetime.combine(release_date, datetime.min.time(), UTC))
    text = (
        f"{PACKAGE} ({version}) {channel}; urgency=medium\n\n"
        f"  * Repackage the published v{tag} release archive.\n\n"
        f" -- {maintainer}  {stamp}\n"
    )
    return gzip.compress(text.encode(), compresslevel=9, mtime=0)


def tar_member(name, kind, mode, timestamp, size=0, target=""):
    member = tarfile.TarInfo(name)
    member.type = kind
    member.mode = mode
    member.size = size
    member.linkname = target
    member.uid = member.gid = 0
    member.uname = member.gname = "root"
    member.mtime = timestamp
    return member


def parent_directories(paths):
    directories = {PurePosixPath(".")}
    for path in paths:
        directories.update(PurePosixPath(path).parents)
    return directories


def compressed_tar(files, timestamp, links=None):
    """A tar.xz of `./`-relative paths, every parent directory listed first, in sorted order.
    GNU format, as dpkg-deb writes it. Every entry has the same time, as with dpkg-deb's
    SOURCE_DATE_EPOCH."""
    links = links or {}
    entries = {PurePosixPath(path): ("file", path) for path in files}
    entries.update({PurePosixPath(path): ("link", path) for path in links})
    entries.update({path: ("directory", path) for path in parent_directories([*files, *links])})
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w", format=tarfile.GNU_FORMAT) as archive:
        for path in sorted(entries, key=lambda entry: entry.parts):
            kind, key = entries[path]
            name = "./" if path.parts == () else f"./{path}"
            if kind == "directory":
                archive.addfile(tar_member(name, tarfile.DIRTYPE, 0o755, timestamp))
            elif kind == "link":
                archive.addfile(
                    tar_member(name, tarfile.SYMTYPE, 0o777, timestamp, target=links[key])
                )
            else:
                contents, mode = files[key]
                member = tar_member(name, tarfile.REGTYPE, mode, timestamp, size=len(contents))
                archive.addfile(member, io.BytesIO(contents))
    return lzma.compress(buffer.getvalue(), format=lzma.FORMAT_XZ)


def installed_files(bundle, maintainer, version, channel, tag, release_date):
    """Package path to (contents, mode). Programs keep release.py's modes: bin/ is 0755."""
    files = {}
    for path in sorted(bundle.rglob("*")):
        if path.is_file():
            relative = path.relative_to(bundle)
            mode = 0o755 if relative.parts[0] == "bin" else 0o644
            files[f"{BUNDLE_DIRECTORY}/{relative.as_posix()}"] = (path.read_bytes(), mode)
    units = bundle / release_tool().SYSTEMD_UNITS
    if units.is_dir():
        for unit in sorted(units.iterdir()):
            files[f"{SYSTEMD_UNIT_DIRECTORY}/{unit.name}"] = (unit.read_bytes(), 0o644)
    files[f"{DOCUMENTATION_DIRECTORY}/copyright"] = ((PACKAGING / "copyright").read_bytes(), 0o644)
    changes = changelog(version, channel, tag, maintainer, release_date)
    files[f"{DOCUMENTATION_DIRECTORY}/changelog.Debian.gz"] = (changes, 0o644)
    return files


def maintainer_scripts(files):
    """The scripts that enable the masks helper's socket, when the package installs its units."""
    if not any(path.startswith(f"{SYSTEMD_UNIT_DIRECTORY}/") for path in files):
        return {}
    return {name: ((PACKAGING / name).read_bytes(), 0o755) for name in MAINTAINER_SCRIPTS}


def installed_size(files, links):
    """dpkg-gencontrol's estimate in KiB: each file rounded up to 1 KiB, one per other entry."""
    file_kibibytes = sum(-(-len(contents) // 1024) for contents, _ in files.values())
    return file_kibibytes + len(links) + len(parent_directories([*files, *links]))


def control_file(version, architecture, maintainer, size):
    values = {
        "@VERSION@": version,
        "@ARCHITECTURE@": architecture,
        "@MAINTAINER@": maintainer,
        "@INSTALLED_SIZE@": str(size),
    }
    template = PACKAGING / "control.in"
    text = template.read_text()
    for token, value in values.items():
        if text.count(token) != 1:
            raise Refusal(f"{template}: expected {token} exactly once")
        text = text.replace(token, value)
    leftover = re.search(r"@[A-Z0-9_]+@", text)
    if leftover:
        raise Refusal(f"{template}: unknown token {leftover.group()}")
    return text


def md5sums(files):
    lines = (
        f"{hashlib.md5(contents, usedforsecurity=False).hexdigest()}  {path}\n"
        for path, (contents, _) in sorted(files.items())
    )
    return "".join(lines)


def ar_archive(members, timestamp):
    """The deb(5) container: ar(5) with fixed owners, mode and time, two-byte aligned."""
    archive = bytearray(AR_MAGIC)
    for name, payload in members:
        header = f"{name:<16}{timestamp:<12}{0:<6}{0:<6}{100644:<8}{len(payload):<10}`\n"
        archive += header.encode("ascii") + payload
        if len(payload) % 2:
            archive += b"\n"
    return bytes(archive)


def deb_members(payload):
    """The members of a .deb this tool writes; any other layout is refused, not guessed at."""
    if not payload.startswith(AR_MAGIC):
        raise Refusal("not an ar archive")
    members = {}
    offset = len(AR_MAGIC)
    while offset < len(payload):
        header = payload[offset : offset + 60]
        if len(header) != 60 or header[58:] != b"`\n":
            raise Refusal(f"ar header at byte {offset} is truncated or malformed")
        name = header[:16].decode("ascii").rstrip(" ").removesuffix("/")
        size = int(header[48:58].decode("ascii"))
        contents = payload[offset + 60 : offset + 60 + size]
        if len(contents) != size:
            raise Refusal(f"ar member {name}: truncated")
        members[name] = contents
        offset += 60 + size + size % 2
    if list(members) != DEB_MEMBERS:
        raise Refusal(f"members {list(members)}, expected {DEB_MEMBERS}")
    if members["debian-binary"] != DEB_FORMAT:
        raise Refusal(f"debian-binary is {members['debian-binary']!r}, expected {DEB_FORMAT!r}")
    return members


def control_text(payload):
    control = lzma.decompress(deb_members(payload)["control.tar.xz"])
    with tarfile.open(fileobj=io.BytesIO(control)) as archive:
        return archive.extractfile("./control").read().decode()


def build(tag, sums, archive, maintainer, release_date, out, debian_revision=1):
    channel = tag_channel(tag)
    version = debian_version(tag, debian_revision)
    if not MAINTAINER.fullmatch(maintainer):
        raise Refusal(f"maintainer {maintainer!r}: not `Name <address>`")
    architecture, platform = listed_archive(tag, sums.read_text(), archive)
    target = out / f"{PACKAGE}_{version}_{architecture}.deb"
    if target.exists():
        raise Refusal(f"{target} exists; build into a directory without it")
    with tempfile.TemporaryDirectory(prefix="torlek-deb-") as folder:
        top_directory = release_tool().archive_name(tag, platform)
        bundle = unpack_bundle(archive, top_directory, Path(folder))
        refuse_foreign_bundle(bundle, tag, channel, platform)
        files = installed_files(bundle, maintainer, version, channel, tag, release_date)
    links = {PUBLIC_COMMAND: PUBLIC_COMMAND_TARGET}
    # The release day, not the epoch: lintian refuses files dated 1970 (package-contains-ancient-file).
    timestamp = int(datetime.combine(release_date, datetime.min.time(), UTC).timestamp())
    control = control_file(version, architecture, maintainer, installed_size(files, links))
    control_files = {
        "control": (control.encode(), 0o644),
        "md5sums": (md5sums(files).encode(), 0o644),
        **maintainer_scripts(files),
    }
    package = ar_archive(
        [
            ("debian-binary", DEB_FORMAT),
            ("control.tar.xz", compressed_tar(control_files, timestamp)),
            ("data.tar.xz", compressed_tar(files, timestamp, links)),
        ],
        timestamp,
    )
    out.mkdir(parents=True, exist_ok=True)
    partial = target.with_name(f".{target.name}.partial")
    partial.write_bytes(package)
    partial.replace(target)
    return target


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--tag", required=True)
    parser.add_argument("--sums", type=Path, required=True)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--maintainer", required=True)
    parser.add_argument("--date", type=date.fromisoformat, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--debian-revision", type=int, default=1)
    arguments = parser.parse_args()
    try:
        target = build(
            arguments.tag,
            arguments.sums,
            arguments.archive,
            arguments.maintainer,
            arguments.date,
            arguments.out,
            arguments.debian_revision,
        )
    except (OSError, ValueError, tarfile.TarError, Refusal) as error:
        parser.exit(1, f"build.py: {error}\n")
    print(f"Built {target}")


if __name__ == "__main__":
    main()
