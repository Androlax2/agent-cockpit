"""The files each package installs, for masks_install.rs's same-files test (#390).

Usage: python3 -I package_roots.py REPOSITORY WORK HELPER

Builds a release.py bundle of stand-in programs whose torlek-sandbox is HELPER, then the .deb
from it with packaging/deb/build.py and the torlek-bin package's files with the rendered
recipe's own package() function, and prints `NAME ROOT` for each package's installed tree.
makepkg and dpkg are not needed: package() runs in bash, and the .deb is unpacked here.
"""

import importlib.util
import io
import lzma
import shutil
import subprocess
import sys
import tarfile
from datetime import date
from pathlib import Path

TAG = "0.1.0-alpha.1"
PLATFORM = "linux-x86_64"


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main(repository, work, helper):
    release = load("release", repository / "tools/release.py")
    deb = load("deb_build", repository / "packaging/deb/build.py")
    aur = load("aur_render", repository / "packaging/aur/render.py")

    binaries = work / "binaries"
    binaries.mkdir()
    for binary in release.BINARIES:
        (binaries / binary).write_text(f"#!/bin/sh\necho {binary}\n")
    shutil.copyfile(helper, binaries / "torlek-sandbox")
    archives = work / "archives"
    bundle = release.package(TAG, "alpha", PLATFORM, "b" * 40, binaries, archives, repository)
    archive = Path(bundle["archive"])
    sums = work / "SHA256SUMS"
    sums.write_text(
        f"{release.sha256_of(archive)}  {archive.name}\n"
        f"{'2' * 64}  torlek-v{TAG}-linux-aarch64.tar.gz\n"
    )

    maintainer = "Torlek test <test@example.invalid>"
    built = deb.build(TAG, sums, archive, maintainer, date(2026, 11, 13), work / "debs")
    deb_root = work / "deb"
    data = lzma.decompress(deb.deb_members(built.read_bytes())["data.tar.xz"])
    with tarfile.open(fileobj=io.BytesIO(data)) as installed:
        installed.extractall(deb_root, filter="tar")

    recipe = work / "aur-recipe"
    recipe.mkdir()
    (recipe / "PKGBUILD").write_text(aur.recipe(TAG, sums.read_text()))
    aur_root = work / "aur"
    aur_root.mkdir()
    subprocess.run(
        ["bash", "-euc", "source PKGBUILD; package", "package"],
        cwd=recipe,
        env={
            "PATH": "/usr/bin:/bin",
            "srcdir": str(archives),
            "pkgdir": str(aur_root),
            "CARCH": "x86_64",
        },
        check=True,
    )

    print(f"deb {deb_root}")
    print(f"aur {aur_root}")


if __name__ == "__main__":
    main(Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3]))
