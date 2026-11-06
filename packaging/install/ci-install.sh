#!/usr/bin/env bash
# The release workflow's install.sh check (#1047, docs/packaging/install-script.md): as root in a
# fresh Ubuntu or Arch container, serve the release's own assets from a loopback stand-in for
# GitHub (tools/tests/fake_release_server.py), run the attached install.sh as an ordinary user
# under the distribution's /bin/sh, check the installed command, run `torlek doctor`, and run
# install.sh again to show that it changes nothing. Never run it on a host: it installs packages
# and adds a user.
#
# Usage: ci-install.sh DIST VERSION
#   DIST holds install.sh, SHA256SUMS and torlek-vVERSION-linux-x86_64.tar.gz.
set -euo pipefail

if [ $# -ne 2 ]; then
    printf 'usage: %s DIST VERSION\n' "$0" >&2
    exit 2
fi
dist=$(realpath "$1")
version=$2
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

# install.sh needs curl beyond a base image; python3 runs the stand-in server only.
if command -v apt-get >/dev/null; then
    export DEBIAN_FRONTEND=noninteractive
    apt-get update
    apt-get install --no-install-recommends --yes curl python3
elif command -v pacman >/dev/null; then
    # The keyring first: the pinned image's own keyring cannot verify packages signed since.
    pacman -Sy --noconfirm archlinux-keyring
    pacman -Su --noconfirm --needed curl python
else
    printf '%s\n' 'ci-install.sh: neither apt-get nor pacman is here' >&2
    exit 1
fi
printf '/bin/sh is %s\n' "$(readlink -f /bin/sh)"

tag=v$version
bundle=torlek-$tag-linux-x86_64
(cd "$dist" && sha256sum --check --ignore-missing SHA256SUMS)
grep -Eq '^[0-9a-f]{64}  install\.sh$' "$dist/SHA256SUMS"

releases=$(mktemp -d)
mkdir "$releases/$tag"
cp "$dist/SHA256SUMS" "$dist/$bundle.tar.gz" "$releases/$tag/"
printf '[{"tag_name":"%s","draft":false,"prerelease":true}]\n' "$tag" >"$releases/releases.json"
python3 "$repository/tools/tests/fake_release_server.py" "$releases" >"$releases/origin" &
server=$!
trap 'kill "$server"' EXIT
for _ in $(seq 100); do
    if [ -s "$releases/origin" ]; then break; fi
    sleep 0.1
done
origin=$(cat "$releases/origin")

useradd --create-home installer
home=/home/installer
install -o installer -m 0644 "$dist/install.sh" "$home/install.sh"
as_installer() {
    (cd "$home" && runuser -u installer -- env -i HOME="$home" \
        PATH="$home/.local/bin:/usr/local/bin:/usr/bin:/bin" TORLEK_INSTALL_ORIGIN="$origin" "$@")
}
expect_line() {
    local wanted=$1 output=$2
    if ! grep -qF -- "$wanted" <<<"$output"; then
        printf 'expected %q in:\n%s\n' "$wanted" "$output" >&2
        exit 1
    fi
}

first_run=$(as_installer sh install.sh --version "$tag")
printf '%s\n' "$first_run"
expect_line "installed Torlek $tag at $home/.local/lib/torlek/$bundle" "$first_run"
test "$(readlink "$home/.local/bin/torlek")" = "$home/.local/lib/torlek/$bundle/bin/torlek"

version_output=$(as_installer timeout 60 torlek version --all)
printf '%s\n' "$version_output"
expect_line "build release" "$version_output"

# No provider, tmux or sandbox engine is installed here, so doctor exits 1 for a missing
# prerequisite; its own lines must still show the release build it runs.
doctor_status=0
doctor_output=$(as_installer timeout 60 torlek doctor) || doctor_status=$?
printf '%s\n' "$doctor_output"
if [ "$doctor_status" -gt 1 ]; then
    printf 'torlek doctor exited %s\n' "$doctor_status" >&2
    exit 1
fi
expect_line "build release" "$doctor_output"

second_run=$(as_installer sh install.sh --version "$tag")
printf '%s\n' "$second_run"
expect_line "Torlek $tag is already installed" "$second_run"
printf 'install.sh %s: installed, ran torlek doctor, and re-ran as a no-op\n' "$tag"
