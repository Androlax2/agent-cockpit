#!/usr/bin/env bash
# The Debian workflow's clean install (#563, docs/packaging/deb.md): as root in a fresh
# Debian or Ubuntu container, check that apt refuses the fixture repository signed by another
# key and the one whose packages changed after signing, then install torlek from the good
# one, upgrading from the previous release when one is given, check the installed bundle, and
# remove and purge it while the user's files stay untouched. Never run it on a host: it
# changes the system it runs on.
#
# Usage: ci-install.sh URL FIXTURE TAG REVISION [PREVIOUS_TAG]
#   URL serves ci-stage.sh's good/, wrong-key/ and tampered/ repositories; FIXTURE holds its
#   torlek-test.asc and TAG's release.py; REVISION is TAG's commit.
set -euo pipefail

if [ $# -ne 4 ] && [ $# -ne 5 ]; then
    printf 'usage: %s URL FIXTURE TAG REVISION [PREVIOUS_TAG]\n' "$0" >&2
    exit 2
fi
url=$1
fixture=$2
tag=$3
revision=$4
previous_tag=${5:-}
if [ "${GITHUB_ACTIONS:-}" != true ] || [ ! -f /.dockerenv ]; then
    printf '%s\n' 'ci-install.sh: refusing to run outside a GitHub Actions job container' >&2
    exit 1
fi
if [ "$(id -u)" -ne 0 ]; then
    printf '%s\n' 'ci-install.sh: run as root inside the container' >&2
    exit 1
fi

export DEBIAN_FRONTEND=noninteractive
channel=${tag#*-}
channel=${channel%%.*}
debian_version() {
    printf '%s-1\n' "${1/-/"~"}"
}
# shellcheck source=/dev/null
. /etc/os-release
printf 'Testing on %s\n' "$PRETTY_NAME"

install -D -m 0644 "$fixture/torlek-test.asc" /etc/apt/keyrings/torlek.asc
use_repository() {
    rm -rf /var/lib/apt/lists/*
    apt-get clean
    cat >/etc/apt/sources.list.d/torlek.sources <<SOURCE
Types: deb
URIs: $url/$1/
Suites: $channel
Components: main
Signed-By: /etc/apt/keyrings/torlek.asc
SOURCE
}

# The distribution's own lists first, so each refusal below is the fixture's.
apt-get update
if [ "$ID" = debian ]; then
    # Debian 13's tmux is 3.5a: the documented route takes 3.6 or later from backports.
    cat >/etc/apt/sources.list.d/backports.sources <<SOURCE
Types: deb
URIs: http://deb.debian.org/debian
Suites: $VERSION_CODENAME-backports
Components: main
Signed-By: /usr/share/keyrings/debian-archive-keyring.gpg
SOURCE
fi

use_repository wrong-key
if apt-get update; then
    printf '%s\n' 'apt-get update accepted an index signed by another key' >&2
    exit 1
fi
if apt-cache show torlek >/dev/null 2>&1; then
    printf '%s\n' 'apt lists torlek from an index signed by another key' >&2
    exit 1
fi

use_repository tampered
apt-get update
if [ "$ID" = debian ]; then
    apt-get install --yes --no-install-recommends -t "$VERSION_CODENAME-backports" tmux
fi
tampered_output=$(apt-get install --yes --download-only torlek 2>&1) && {
    printf '%s\n' "$tampered_output"
    printf '%s\n' 'apt-get downloaded a package changed after signing' >&2
    exit 1
}
printf '%s\n' "$tampered_output"
if ! grep -qiE 'hash sum mismatch|hashes of expected file' <<<"$tampered_output"; then
    printf '%s\n' 'apt-get failed, but not on the changed package' >&2
    exit 1
fi

use_repository good
apt-get update
apt-cache policy torlek
useradd --create-home tester
home=/home/tester
as_tester() {
    (cd "$home" && runuser -u tester -- env HOME="$home" "$@")
}
expect_line() {
    local wanted=$1 output=$2
    if ! grep -qxF -- "$wanted" <<<"$output"; then
        printf 'expected the line %q in:\n%s\n' "$wanted" "$output" >&2
        exit 1
    fi
}
expect_installed() {
    local installed
    installed=$(dpkg-query --show --showformat '${Version}' torlek)
    if [ "$installed" != "$(debian_version "$1")" ]; then
        printf 'dpkg has torlek %s installed, expected %s\n' "$installed" "$(debian_version "$1")" >&2
        exit 1
    fi
    dpkg --verify torlek
    test "$(readlink -f /usr/bin/torlek)" = /usr/lib/torlek/bin/torlek
    local version_output
    version_output=$(as_tester timeout 60 torlek version --all)
    expect_line "build release" "$version_output"
    expect_line "bundle manifest /usr/lib/torlek/share/torlek/manifest.json:" "$version_output"
    local tui_output
    tui_output=$(as_tester timeout 60 torlek tui --version)
    expect_line "torlek-tui $1" "$tui_output"
}
# No provider or sandbox engine is installed, so doctor exits 1 for a missing prerequisite;
# its own lines must still show the packaged release build and the profile's directories.
keep_profile_files() {
    local doctor_status=0 doctor_output
    doctor_output=$(as_tester timeout 60 torlek doctor) || doctor_status=$?
    printf '%s\n' "$doctor_output"
    if [ "$doctor_status" -gt 1 ]; then
        printf 'torlek doctor exited %s\n' "$doctor_status" >&2
        exit 1
    fi
    expect_line "build release" "$doctor_output"
    local directory
    for directory in "$(sed -n 's/^config dir //p' <<<"$doctor_output")" \
        "$(sed -n 's/^state dir //p' <<<"$doctor_output")"; do
        case $directory in
        "$home"/*) ;;
        *)
            printf 'doctor names a profile directory outside the home: %q\n' "$directory" >&2
            exit 1
            ;;
        esac
        as_tester mkdir -p "$directory"
        printf '%s' "$1" | as_tester tee "$directory/kept-$1" >/dev/null
    done
}

if [ -n "$previous_tag" ]; then
    apt-get install --yes "torlek=$(debian_version "$previous_tag")"
    expect_installed "$previous_tag"
    keep_profile_files "$previous_tag"
fi
# Without --no-install-recommends, as a user would type it.
apt-get install --yes torlek
expect_installed "$tag"
keep_profile_files "$tag"
# The release's own clean-install check, daemon start included, through the packaged path.
apt-get install --yes --no-install-recommends python3
as_tester python3 "$fixture/release.py" verify-install /usr/lib/torlek --revision "$revision"

home_contents() {
    find "$home" -type f -print0 | sort -z | xargs -0 sha256sum
    find "$home" -print | sort
}
home_contents >/root/home-before
for action in remove purge; do
    apt-get "$action" --yes torlek
    home_contents >"/root/home-after-$action"
    diff -u /root/home-before "/root/home-after-$action"
done
if dpkg-query --show --showformat '${db:Status-Status}' torlek 2>/dev/null | grep -qvx not-installed; then
    printf '%s\n' 'dpkg still records torlek after purge' >&2
    exit 1
fi
for removed in /usr/bin/torlek /usr/lib/torlek /usr/share/doc/torlek; do
    if [ -e "$removed" ] || [ -L "$removed" ]; then
        printf '%s remains after apt-get purge\n' "$removed" >&2
        exit 1
    fi
done
printf 'torlek %s: refused tampered sources, installed%s, verified and removed on %s\n' \
    "$(debian_version "$tag")" "${previous_tag:+ over $(debian_version "$previous_tag")}" \
    "$PRETTY_NAME"
