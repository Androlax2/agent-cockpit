#!/bin/sh
# Runs inside the declared WSL 2 distribution, started by torlek-wsl.ps1 through
# `wsl.exe --cd <Scoop app directory> --exec /bin/sh ./install-in-wsl.sh` (docs/packaging/windows.md).
#
# Usage: install-in-wsl.sh check TAG
#        install-in-wsl.sh install TAG
#
# The working directory holds the release's SHA256SUMS and its linux-x86_64 archive, saved by
# Scoop under a name without an archive extension so that Scoop does not unpack it on NTFS.
# `check` only reads. `install` unpacks into ~/.local/lib/torlek/vTAG and points
# ~/.local/lib/torlek/current at it; it never removes or replaces an installed version.
set -eu

fail() {
    printf 'torlek-wsl: %s\n' "$1" >&2
    exit 1
}

[ "$#" -eq 2 ] || fail "usage: install-in-wsl.sh check|install TAG"
action=$1
tag=$2
case "$action" in
check | install) ;;
*) fail "unknown action '$action': use check or install" ;;
esac
# The release tag grammar of tools/release.py, without the v and without stable (none exists).
printf '%s\n' "$tag" | grep -Eqx '(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)-(alpha|beta)\.[1-9][0-9]*' ||
    fail "'$tag' is not a Torlek prerelease tag (X.Y.Z-alpha.N or X.Y.Z-beta.N)"

kernel=$(uname -r)
case "$kernel" in
*microsoft-standard-WSL2*) ;;
*) fail "kernel '$kernel' is not a WSL 2 kernel. Torlek needs WSL 2; converting a distribution is your decision: wsl.exe --set-version <distribution> 2" ;;
esac
machine=$(uname -m)
[ "$machine" = x86_64 ] ||
    fail "this distribution runs on $machine, but the Windows package carries only the linux-x86_64 archive. Install the linux-aarch64 archive inside WSL by hand (docs/packaging/windows.md)"
[ "$(id -u)" -ne 0 ] ||
    fail "running as root. Torlek installs per user: set the distribution's default user to your own account, then retry"
for tool in sha256sum tar awk; do
    command -v "$tool" >/dev/null 2>&1 || fail "$tool is missing from this distribution; install it with its package manager, then retry"
done

bundle="torlek-v$tag-linux-x86_64"
archive="$bundle.archive"
[ -f SHA256SUMS ] || fail "SHA256SUMS is missing from $(pwd); reinstall the Scoop package"
[ -f "$archive" ] || fail "$archive is missing from $(pwd); reinstall the Scoop package"
expected=$(awk -v name="$bundle.tar.gz" '$2 == name && $1 ~ /^[0-9a-f]+$/ && length($1) == 64 { print $1 }' SHA256SUMS)
[ "$(printf '%s\n' "$expected" | grep -c .)" -eq 1 ] ||
    fail "SHA256SUMS lists $bundle.tar.gz $(printf '%s\n' "$expected" | grep -c .) times with a valid digest, not once"
actual=$(sha256sum <"$archive" | awk '{ print $1 }')
[ "$actual" = "$expected" ] ||
    fail "integrity check failed: $archive has SHA-256 $actual, SHA256SUMS says $expected. Nothing was installed; reinstall the Scoop package and report it if it repeats"

root="$HOME/.local/lib/torlek"
target="$root/v$tag"
if [ "$action" = check ]; then
    current=$(readlink "$root/current" 2>/dev/null || true)
    printf 'WSL 2 distribution ready for Torlek %s (%s, %s).\n' "$tag" "$machine" "$kernel"
    if [ -d "$target" ]; then
        printf 'Installed at %s; current points at %s.\n' "$target" "${current:-nothing}"
    else
        printf 'Not installed yet: torlek-wsl install would unpack it into %s.\n' "$target"
    fi
    exit 0
fi

[ ! -e "$target" ] ||
    fail "$target already exists and is kept. To use it, run: ln -sfn v$tag $root/current"
# Every member must sit under the one top directory, so the archive cannot write elsewhere.
members=$(tar -tzf "$archive") || fail "$archive is not a readable .tar.gz"
printf '%s\n' "$members" | while IFS= read -r member; do
    case "$member" in
    "$bundle" | "$bundle/" | "$bundle"/*) ;;
    *) fail "archive member '$member' is outside $bundle/; nothing was installed" ;;
    esac
    case "/$member/" in
    */../*) fail "archive member '$member' climbs out of its directory; nothing was installed" ;;
    esac
done || exit 1
# A release bundle holds only files and directories (tools/release.py); a link could point a
# later member outside the bundle.
types=$(tar -tvzf "$archive" | cut -c1 | sort -u)
! printf '%s\n' "$types" | grep -qv '^[-d]$' ||
    fail "$archive holds links or special files, which a Torlek bundle never does; nothing was installed"
mkdir -p "$root"
staging=$(mktemp -d "$root/.staging.XXXXXX")
tar -xzf "$archive" -C "$staging" --no-same-owner
[ -x "$staging/$bundle/bin/torlek" ] || {
    rm -rf "$staging"
    fail "$archive has no executable bin/torlek; nothing was installed"
}
mv "$staging/$bundle" "$target"
rmdir "$staging"
ln -sfn "v$tag" "$root/current"
printf 'Installed Torlek %s at %s; %s/current points at it.\n' "$tag" "$target" "$root"
printf 'Next: run "torlek doctor" from Windows, or %s/current/bin/torlek doctor inside WSL.\n' "$root"
