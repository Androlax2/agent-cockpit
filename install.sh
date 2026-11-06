#!/bin/sh
# Installs Torlek for the current user from a published GitHub release (docs/packaging/install-script.md).
#
#   curl -fsSL <release>/install.sh | sh                        newest alpha release
#   curl -fsSL <release>/install.sh | sh -s -- --version vTAG   one published release
#   sh install.sh --check [--version vTAG]                      verify only, change nothing
#
# It downloads only the release's torlek-vTAG-linux-ARCH.tar.gz and SHA256SUMS, checks the
# archive against SHA256SUMS (and its build provenance when a logged-in gh is present) before
# unpacking anything, then unpacks it into ~/.local/lib/torlek/torlek-vTAG-linux-ARCH and links
# ~/.local/bin/torlek to it: the README's archive layout, which `torlek update` recognises.
# Re-running installs a newer release beside the old one, or changes nothing. It never runs as
# root, never calls sudo, and never touches Torlek's config, state or daemon.
#
# The Windows route (packaging/windows/install-in-wsl.sh) runs this same script on the files
# Scoop downloaded and verified: --archive FILE --sums FILE --version vTAG, without the network.
#
# TORLEK_INSTALL_ORIGIN=http://127.0.0.1:PORT replaces GitHub with a loopback test server
# (tools/tests/fake_release_server.py); no other origin is accepted.
set -eu

REPOSITORY=Androlax2/agent-cockpit
PREFIX=install.sh

say() {
    printf '%s: %s\n' "$PREFIX" "$1"
}

warn() {
    printf '%s: warning: %s\n' "$PREFIX" "$1" >&2
}

fail() {
    printf '%s: %s\n' "$PREFIX" "$1" >&2
    exit 1
}

usage() {
    cat <<'USAGE'
Usage: install.sh [--version vTAG] [--check]
       install.sh --version vTAG --archive FILE --sums FILE [--check]

Installs Torlek into ~/.local/lib/torlek and links ~/.local/bin/torlek, for this user only.
  --version vTAG  a published release, such as v0.1.0-alpha.4 (default: the newest alpha)
  --check         download and verify, report what would change, change nothing
  --archive FILE  use this already downloaded release archive instead of GitHub
  --sums FILE     the SHA256SUMS of the same release, required with --archive
USAGE
}

# The release tag grammar of tools/release.py: no stable release exists yet. A dry run's
# X.Y.Z-dev.N is never published: only the loopback test server and the files the release
# workflow's install-scoop job passes with --archive carry one.
is_release_tag() {
    printf '%s\n' "$1" | grep -Eqx 'v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)-(alpha|beta)\.[1-9][0-9]*'
}

is_dry_run_tag() {
    printf '%s\n' "$1" | grep -Eqx 'v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)-dev\.(0|[1-9][0-9]*)'
}

is_alpha_tag() {
    printf '%s\n' "$1" | grep -Eqx 'v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)-alpha\.[1-9][0-9]*'
}

fetch() {
    curl --proto "$protocol" --proto-redir "$protocol" --tlsv1.2 --fail --silent --show-error \
        --location --retry 3 --output "$2" "$1"
}

# GitHub's JSON is one line; each field of a release object becomes one line here. A string
# value cannot hold a bare quote, so release notes cannot forge a "tag_name" line.
release_fields() {
    # shellcheck disable=SC2020 # three characters, each to a newline
    tr ',{}' '\n\n\n' <"$1"
}

newest_alpha_tag() {
    fetch "$api/releases?per_page=100" "$work/releases.json" ||
        fail "could not read the release list from $api/releases"
    release_fields "$work/releases.json" | sed -n 's/^ *"tag_name": *"\([^"]*\)" *$/\1/p' |
        while IFS= read -r candidate; do
            if is_alpha_tag "$candidate"; then
                printf '%s\n' "$candidate"
                break
            fi
        done
}

# Unauthenticated, GitHub answers 404 for a draft or a tag without a release.
require_published() {
    fetch "$api/releases/tags/$1" "$work/release.json" ||
        fail "$1 is not a published Torlek release, or GitHub could not be reached (curl says why above; releases: https://github.com/$REPOSITORY/releases)"
    tag_pattern=$(printf '%s\n' "$1" | sed 's/\./\\./g')
    if ! release_fields "$work/release.json" | grep -Eqx " *\"tag_name\": *\"$tag_pattern\" *" ||
        ! release_fields "$work/release.json" | grep -Eqx ' *"draft": *false *'; then
        fail "$1 is not a published Torlek release: GitHub's answer does not describe it"
    fi
}

listed_checksum() {
    awk -v name="$1" '$2 == name && $1 ~ /^[0-9a-f]+$/ && length($1) == 64 { print $1 }' "$2"
}

verify_checksum() {
    expected=$(listed_checksum "$bundle.tar.gz" "$sums")
    count=$(printf '%s\n' "$expected" | grep -c . || true)
    [ "$count" -eq 1 ] ||
        fail "SHA256SUMS lists $bundle.tar.gz $count times with a valid digest, not once; nothing was installed"
    actual=$(sha256sum <"$archive" | awk '{ print $1 }')
    [ "$actual" = "$expected" ] ||
        fail "integrity check failed: $archive has SHA-256 $actual, SHA256SUMS says $expected. Nothing was installed; retry, and report it if it repeats"
    say "checksum matches SHA256SUMS ($actual)"
}

verify_provenance() {
    if [ -n "$origin" ]; then
        say "provenance not checked: the test server's builds are not attested"
        return
    fi
    if is_dry_run_tag "$tag"; then
        say "provenance not checked: a dry run's $tag has no release tag to be attested from"
        return
    fi
    if ! command -v gh >/dev/null 2>&1; then
        say "provenance not checked: gh is not installed (the checksum above was checked)"
        return
    fi
    if ! gh auth status >/dev/null 2>&1; then
        say "provenance not checked: gh is not logged in (the checksum above was checked)"
        return
    fi
    gh attestation verify "$archive" --repo "$REPOSITORY" \
        --signer-workflow "$REPOSITORY/.github/workflows/release.yml" \
        --source-ref "refs/tags/$tag" --deny-self-hosted-runners >/dev/null ||
        fail "gh attestation verify refused $bundle.tar.gz: it is not this repository's release build of $tag. Nothing was installed"
    say "provenance verified by gh: built by $REPOSITORY's release workflow from $tag"
}

# Every member must sit under the one top directory and be a plain file or directory
# (tools/release.py), so unpacking cannot write outside the bundle.
verify_members() {
    members=$(tar -tzf "$archive") || fail "$archive is not a readable .tar.gz; nothing was installed"
    printf '%s\n' "$members" | while IFS= read -r member; do
        case "$member" in
        "$bundle" | "$bundle/" | "$bundle"/*) ;;
        *) fail "archive member '$member' is outside $bundle/; nothing was installed" ;;
        esac
        case "/$member/" in
        */../*) fail "archive member '$member' climbs out of its directory; nothing was installed" ;;
        esac
    done || exit 1
    types=$(tar -tvzf "$archive" | cut -c1 | sort -u)
    ! printf '%s\n' "$types" | grep -qv '^[-d]$' ||
        fail "$archive holds links or special files, which a Torlek bundle never does; nothing was installed"
}

unpack() {
    mkdir -p "$library"
    staging=$(mktemp -d "$library/.staging.XXXXXX")
    tar -xzf "$archive" -C "$staging" --no-same-owner
    [ -x "$staging/$bundle/bin/torlek" ] || {
        rm -rf "$staging"
        fail "$archive has no executable bin/torlek; nothing was installed"
    }
    mv "$staging/$bundle" "$target"
    rmdir "$staging"
}

# The link may be replaced only when this script or the README made it.
linked_release() {
    [ -e "$link" ] || [ -L "$link" ] || return 0
    [ -L "$link" ] ||
        fail "$link exists and is not a link to a Torlek release; move it away first. Nothing was installed"
    current=$(readlink "$link")
    case "$current" in
    "$library"/torlek-v*/bin/torlek) printf '%s\n' "$current" ;;
    *) fail "$link points at $current, not at a release in $library; move it away first. Nothing was installed" ;;
    esac
}

print_next_steps() {
    case ":$PATH:" in
    *":$bin_dir:"*) command=torlek ;;
    *)
        warn "$bin_dir is not in your PATH: add it (for example in ~/.profile), or run $link directly"
        command=$link
        ;;
    esac
    say "next: run \"$command doctor\", then \"$command daemon install\" (it replaces a running daemon; agent sessions keep running), then \"$command init\""
}

main() {
    tag=
    mode=install
    archive=
    sums=
    while [ "$#" -gt 0 ]; do
        case "$1" in
        --version | --archive | --sums)
            [ "$#" -ge 2 ] || fail "$1 needs a value (see --help)"
            case "$1" in
            --version) tag=$2 ;;
            --archive) archive=$2 ;;
            --sums) sums=$2 ;;
            esac
            shift 2
            ;;
        --check)
            mode=check
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *) fail "unknown argument '$1' (see --help)" ;;
        esac
    done

    origin=${TORLEK_INSTALL_ORIGIN-}
    api=https://api.github.com/repos/$REPOSITORY
    downloads=https://github.com/$REPOSITORY/releases/download
    protocol='=https'
    if [ -n "$origin" ]; then
        printf '%s\n' "$origin" | grep -Eqx 'http://127\.0\.0\.1:[1-9][0-9]*' ||
            fail "TORLEK_INSTALL_ORIGIN must be http://127.0.0.1:PORT, a loopback test server; unset it to install from GitHub"
        api=$origin/api
        downloads=$origin/download
        protocol='=http'
        warn "installing from the test server $origin, not from GitHub"
    fi

    if [ -n "$tag" ]; then
        is_release_tag "$tag" || { [ -n "$origin$archive" ] && is_dry_run_tag "$tag"; } ||
            fail "'$tag' is not a Torlek release tag (vX.Y.Z-alpha.N or vX.Y.Z-beta.N)"
    fi
    if [ -n "$archive" ] || [ -n "$sums" ]; then
        [ -n "$archive" ] && [ -n "$sums" ] && [ -n "$tag" ] ||
            fail "--archive, --sums and --version go together"
        [ -f "$archive" ] || fail "$archive is missing"
        [ -f "$sums" ] || fail "$sums is missing"
    fi

    system=$(uname -s)
    [ "$system" = Linux ] ||
        fail "this is $system; Torlek releases exist only for Linux (and WSL 2) today"
    machine=$(uname -m)
    case "$machine" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) fail "this machine is $machine; Torlek releases exist only for x86_64 and aarch64" ;;
    esac
    [ "$(id -u)" -ne 0 ] ||
        fail "running as root. Torlek installs per user and never uses root or sudo: run this as your own account"
    [ -n "${HOME-}" ] && [ -d "$HOME" ] || fail "HOME is not set to a directory"
    needed="tar sha256sum awk mktemp"
    [ -n "$archive" ] || needed="curl $needed"
    for tool in $needed; do
        command -v "$tool" >/dev/null 2>&1 ||
            fail "$tool is missing; install it with your package manager, then run this again"
    done

    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    trap 'exit 1' HUP INT TERM

    if [ -z "$archive" ]; then
        if [ -n "$tag" ]; then
            require_published "$tag"
        else
            tag=$(newest_alpha_tag)
            [ -n "$tag" ] || fail "GitHub lists no published alpha release of $REPOSITORY"
        fi
    fi

    library=$HOME/.local/lib/torlek
    bin_dir=$HOME/.local/bin
    link=$bin_dir/torlek
    bundle=torlek-$tag-linux-$arch
    target=$library/$bundle
    wanted_link=$target/bin/torlek
    previous=$(linked_release)

    if [ -d "$target" ]; then
        [ -x "$wanted_link" ] ||
            fail "$target exists but has no executable bin/torlek; move it away, then run this again"
        if [ "$previous" = "$wanted_link" ]; then
            say "Torlek $tag is already installed at $target; nothing changed"
            print_next_steps
            exit 0
        fi
        if [ "$mode" = check ]; then
            say "Torlek $tag is unpacked at $target; installing would point $link at it"
            exit 0
        fi
        mkdir -p "$bin_dir"
        ln -sfn "$wanted_link" "$link"
        say "Torlek $tag was already unpacked at $target; $link now points at it"
        print_next_steps
        exit 0
    fi

    if [ -z "$archive" ]; then
        say "downloading Torlek $tag for linux-$arch"
        sums=$work/SHA256SUMS
        archive=$work/$bundle.tar.gz
        fetch "$downloads/$tag/SHA256SUMS" "$sums" || fail "could not download SHA256SUMS of $tag"
        fetch "$downloads/$tag/$bundle.tar.gz" "$archive" ||
            fail "could not download $bundle.tar.gz: does $tag have a linux-$arch archive?"
    else
        say "release $tag from $archive and $sums, not looked up on GitHub"
    fi
    verify_checksum
    verify_provenance
    verify_members

    if [ "$mode" = check ]; then
        say "verified; installing would unpack Torlek $tag into $target and point $link at it"
        exit 0
    fi
    unpack
    mkdir -p "$bin_dir"
    ln -sfn "$wanted_link" "$link"
    say "installed Torlek $tag at $target; $link points at it"
    if [ -n "$previous" ]; then
        say "the previous release stays in $(dirname "$(dirname "$previous")"); remove it once $tag works"
    fi
    print_next_steps
}

# Everything above only defines functions, so a download cut short by `curl | sh` runs nothing.
main "$@"
