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
pacman -Su --noconfirm --needed git curl tmux python
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
# check-srcinfo refuses a --development recipe, so compare with makepkg's directly.
(cd "$recipe" && as_builder makepkg --printsrcinfo) | diff -u - "$recipe/.SRCINFO"

# The recipe's own tests, which skip on the Ubuntu runners for want of makepkg.
(cd "$home" && as_builder python3 -m unittest discover -s "$repository/tools/tests" \
    -p test_aur_package.py -v)

packages=("$recipe"/torlek-bin-*.pkg.tar.zst)
pacman -U --noconfirm "${packages[@]}"
test "$(readlink -f /usr/bin/torlek)" = /usr/lib/torlek/bin/torlek
as_builder python3 "$repository/tools/release.py" verify-install /usr/lib/torlek \
    --revision "$revision"

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
for removed in /usr/bin/torlek /usr/lib/torlek /usr/share/licenses/torlek-bin; do
    if [ -e "$removed" ] || [ -L "$removed" ]; then
        printf '%s remains after pacman -R\n' "$removed" >&2
        exit 1
    fi
done
printf 'torlek-bin %s: built, installed, verified and removed on Arch\n' "$version"
