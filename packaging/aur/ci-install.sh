#!/usr/bin/env bash
# The release workflow's clean Arch installs (#562, docs/packaging/aur.md): as root in a fresh
# archlinux:base-devel container, render torlek-bin, build it as a user, install it with pacman,
# check the installed bundle, then remove it and check that the user's files are untouched.
# Never run it on a host: it changes the system it runs on.
#
# Usage: ci-install.sh DIST VERSION CHANNEL REVISION
#          install-arch, a candidate: DIST holds SHA256SUMS and
#          torlek-vVERSION-linux-x86_64.tar.gz; CHANNEL is release.py's (`dev` for a dry run's
#          X.Y.Z-dev.N, which renders with --development).
#        ci-install.sh --published
#          install-arch-published, the committed packaging/aur/torlek-bin recipe: it must be
#          the rendering of its release's published SHA256SUMS, and makepkg downloads the
#          archives it names.
set -euo pipefail

usage() {
    printf 'usage: %s DIST VERSION CHANNEL REVISION\n       %s --published\n' "$0" "$0" >&2
    exit 2
}
repository=$(realpath "$(dirname "$0")/../..")
committed=$repository/packaging/aur/torlek-bin
is_published=false
if [ $# -eq 1 ] && [ "$1" = --published ]; then
    is_published=true
    version=$(sed -n 's/^_tag=//p' "$committed/PKGBUILD")
    package_release=$(sed -n 's/^pkgrel=//p' "$committed/PKGBUILD")
    channel=published
    if [ -z "$version" ] || [ -z "$package_release" ]; then
        printf '%s: no _tag or pkgrel line\n' "$committed/PKGBUILD" >&2
        exit 1
    fi
elif [ $# -eq 4 ]; then
    dist=$(realpath "$1")
    version=$2
    channel=$3
    revision=$4
else
    usage
fi
# A GitHub Actions job container only: toolbox or distrobox also set $container, but share
# the user's home and outlive the run.
if [ "${GITHUB_ACTIONS:-}" != true ] || [ ! -f /.dockerenv ]; then
    printf '%s\n' 'ci-install.sh: refusing to run outside a GitHub Actions job container' >&2
    exit 1
fi
if [ "$(id -u)" -ne 0 ]; then
    printf '%s\n' 'ci-install.sh: run as root inside the container' >&2
    exit 1
fi

# The keyring first: the pinned image's own keyring cannot verify packages signed since.
pacman -Sy --noconfirm archlinux-keyring
pacman -Su --noconfirm --needed git curl tmux python namcap
useradd --create-home builder
home=/home/builder
as_builder() {
    runuser -u builder -- env HOME="$home" "$@"
}

recipe=$home/torlek-bin
render_options=()
if [ "$channel" = dev ]; then render_options+=(--development); fi
if $is_published; then
    upstream=https://github.com/Androlax2/agent-cockpit
    sums=$home/SHA256SUMS
    as_builder curl --proto '=https' --tlsv1.2 -fsSLo "$sums" \
        "$upstream/releases/download/v$version/SHA256SUMS"
    render_options+=(--pkgrel "$package_release")
    # verify-install as the release ran it, at its own tagged commit: main's moves on with
    # later bundles.
    as_builder git clone --quiet --depth 1 --branch "v$version" "$upstream.git" "$home/tagged"
    revision=$(as_builder git -C "$home/tagged" rev-parse HEAD)
    verify_tool=$home/tagged/tools/release.py
else
    sums=$dist/SHA256SUMS
    verify_tool=$repository/tools/release.py
fi
as_builder python3 "$repository/packaging/aur/render.py" render "${render_options[@]}" \
    --tag "$version" --sums "$sums" --out "$recipe"
if $is_published; then
    # What an AUR push carries is the committed pair, so it must be this rendering, byte for byte.
    diff -u "$recipe/PKGBUILD" "$committed/PKGBUILD"
    diff -u "$recipe/.SRCINFO" "$committed/.SRCINFO"
    diff -u "$recipe/torlek-bin.install" "$committed/torlek-bin.install"
    # No Arch container runs on aarch64 here: makepkg downloads that archive and checks it
    # against the recipe's checksum, and nothing installs it.
    (cd "$recipe" && as_builder env CARCH=aarch64 makepkg --verifysource --noprogressbar)
else
    install -o builder -m 0644 "$dist/torlek-v$version-linux-x86_64.tar.gz" "$recipe/"
fi
# Without --nodeps: makepkg refuses unless every runtime dependency resolves on Arch.
(cd "$recipe" && as_builder makepkg --noconfirm --noprogressbar)
# check-srcinfo refuses a --development recipe, so compare with makepkg's directly.
(cd "$recipe" && as_builder makepkg --printsrcinfo) | diff -u - "$recipe/.SRCINFO"

# The recipe's own tests, which skip on the Ubuntu runners for want of makepkg.
(cd "$home" && as_builder python3 -m unittest discover -s "$repository/tools/tests" \
    -p test_aur_package.py -v)

packages=("$recipe"/torlek-bin-*.pkg.tar.zst)
# For the submission's record (docs/packaging/aur.md explains each expected finding); namcap
# exits 0 whatever it finds, so an error-level finding fails here.
findings=$(cd "$recipe" && as_builder namcap PKGBUILD "${packages[@]}")
printf 'namcap:\n%s\n' "$findings"
if grep -q ' E: ' <<< "$findings"; then
    printf '%s\n' 'namcap reports an error-level finding' >&2
    exit 1
fi

pacman -U --noconfirm "${packages[@]}"
test "$(readlink -f /usr/bin/torlek)" = /usr/lib/torlek/bin/torlek
as_builder python3 "$verify_tool" verify-install /usr/lib/torlek --revision "$revision"
# The masks helper's units (#390), from a release whose bundle carries them; no systemd runs
# here, so torlek-bin.install can only enable the socket.
if [ -d /usr/lib/torlek/share/torlek/systemd ]; then
    for unit in torlek-engine-masks.socket torlek-engine-masks@.service; do
        cmp "/usr/lib/torlek/share/torlek/systemd/$unit" "/usr/lib/systemd/system/$unit"
    done
    test -L /etc/systemd/system/sockets.target.wants/torlek-engine-masks.socket
fi

# Through the public command: the link must still find the bundle's TUI and manifest.
expect_line() {
    local wanted=$1 output=$2
    if ! grep -qxF -- "$wanted" <<< "$output"; then
        printf 'expected the line %q in:\n%s\n' "$wanted" "$output" >&2
        exit 1
    fi
}
version_output=$(cd "$home" && as_builder timeout 60 /usr/bin/torlek version --all)
expect_line "build release" "$version_output"
expect_line "bundle manifest /usr/lib/torlek/share/torlek/manifest.json:" "$version_output"
tui_output=$(cd "$home" && as_builder timeout 60 /usr/bin/torlek tui --version)
expect_line "torlek-tui $version" "$tui_output"

# No provider or sandbox engine is installed here, so doctor exits 1 for a missing
# prerequisite; its own lines must still show the packaged release build and profile.
doctor_status=0
doctor_output=$(cd "$home" && as_builder timeout 60 torlek doctor) || doctor_status=$?
printf '%s\n' "$doctor_output"
if [ "$doctor_status" -gt 1 ]; then
    printf 'torlek doctor exited %s\n' "$doctor_status" >&2
    exit 1
fi
expect_line "build release" "$doctor_output"
config_dir=$(sed -n 's/^config dir //p' <<< "$doctor_output")
state_dir=$(sed -n 's/^state dir //p' <<< "$doctor_output")
for directory in "$config_dir" "$state_dir"; do
    case $directory in
        "$home"/*) ;;
        *) printf 'doctor names a profile directory outside the home: %q\n' "$directory" >&2
           exit 1 ;;
    esac
    as_builder mkdir -p "$directory"
    printf kept | as_builder tee "$directory/kept-on-removal" > /dev/null
done

home_contents() {
    find "$home" -path "$recipe" -prune -o -type f -print0 | sort -z | xargs -0 sha256sum
    find "$home" -path "$recipe" -prune -o -print | sort
}
home_contents > /tmp/home-before
pacman -R --noconfirm torlek-bin
home_contents > /tmp/home-after
diff -u /tmp/home-before /tmp/home-after
for removed in /usr/bin/torlek /usr/lib/torlek /usr/share/licenses/torlek-bin \
    /usr/lib/systemd/system/torlek-engine-masks.socket \
    /usr/lib/systemd/system/torlek-engine-masks@.service \
    /etc/systemd/system/sockets.target.wants/torlek-engine-masks.socket; do
    if [ -e "$removed" ] || [ -L "$removed" ]; then
        printf '%s remains after pacman -R\n' "$removed" >&2
        exit 1
    fi
done
printf 'torlek-bin %s (%s): built, installed, verified and removed on Arch\n' "$version" "$channel"
