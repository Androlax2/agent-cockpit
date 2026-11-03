#!/usr/bin/env bash
# The release workflow's clean Arch install (#562, docs/packaging/aur.md): as root in a fresh
# archlinux:base-devel container, render torlek-bin for the candidate, build it as a user,
# install it with pacman, check the installed bundle, then remove it and check that the
# user's files are untouched. Never run it on a host: it changes the system it runs on.
#
# Usage: ci-install.sh DIST VERSION CHANNEL REVISION
#   DIST holds SHA256SUMS and torlek-vVERSION-linux-x86_64.tar.gz; CHANNEL is release.py's
#   (`dev` for a dry run's X.Y.Z-dev.N, which renders with --development).
set -euo pipefail

if [ $# -ne 4 ]; then
    printf 'usage: %s DIST VERSION CHANNEL REVISION\n' "$0" >&2
    exit 2
fi
dist=$(realpath "$1")
version=$2
channel=$3
revision=$4
repository=$(realpath "$(dirname "$0")/../..")
if [ ! -f /.dockerenv ] && [ -z "${container:-}" ]; then
    printf '%s\n' 'ci-install.sh: refusing to run outside a disposable container' >&2
    exit 1
fi
if [ "$(id -u)" -ne 0 ]; then
    printf '%s\n' 'ci-install.sh: run as root inside the container' >&2
    exit 1
fi

pacman -Syu --noconfirm --needed git curl tmux python
useradd --create-home builder
home=/home/builder
as_builder() {
    runuser -u builder -- env HOME="$home" "$@"
}

recipe=$home/torlek-bin
render_options=()
if [ "$channel" = dev ]; then render_options+=(--development); fi
as_builder python3 "$repository/packaging/aur/render.py" render "${render_options[@]}" \
    --tag "$version" --sums "$dist/SHA256SUMS" --out "$recipe"
install -o builder -m 0644 "$dist/torlek-v$version-linux-x86_64.tar.gz" "$recipe/"
# Without --nodeps: makepkg refuses unless every runtime dependency resolves on Arch.
(cd "$recipe" && as_builder makepkg --noconfirm --noprogressbar)
as_builder python3 "$repository/packaging/aur/render.py" check-srcinfo "$recipe"

# The recipe's own tests, which skip on the Ubuntu runners for want of makepkg.
(cd "$home" && as_builder python3 -m unittest discover -s "$repository/tools/tests" \
    -p test_aur_package.py -v)

packages=("$recipe"/torlek-bin-*.pkg.tar.zst)
pacman -U --noconfirm "${packages[@]}"
test "$(readlink -f /usr/bin/torlek)" = /usr/lib/torlek/bin/torlek
as_builder python3 "$repository/tools/release.py" verify-install /usr/lib/torlek \
    --revision "$revision"

# No provider or sandbox engine is installed here, so doctor may report a missing
# prerequisite (exit 1); anything else is a failure of the packaged program.
doctor_status=0
(cd "$home" && as_builder torlek doctor) || doctor_status=$?
if [ "$doctor_status" -gt 1 ]; then
    printf 'torlek doctor exited %s\n' "$doctor_status" >&2
    exit 1
fi

install -d -o builder "$home/.config/torlek"
as_builder touch "$home/.config/torlek/kept-on-removal"
find "$home" -path "$recipe" -prune -o -print | sort > /tmp/home-before
pacman -R --noconfirm torlek-bin
find "$home" -path "$recipe" -prune -o -print | sort > /tmp/home-after
diff -u /tmp/home-before /tmp/home-after
for removed in /usr/bin/torlek /usr/lib/torlek /usr/share/licenses/torlek-bin; do
    if [ -e "$removed" ] || [ -L "$removed" ]; then
        printf '%s remains after pacman -R\n' "$removed" >&2
        exit 1
    fi
done
printf 'torlek-bin %s: built, installed, verified and removed on Arch\n' "$version"
