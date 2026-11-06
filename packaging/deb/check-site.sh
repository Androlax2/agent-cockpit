#!/usr/bin/env bash
# The published APT site as apt sees it (#563, docs/packaging/deb.md#publication): wait until
# URL serves TAG's packages, download the suite's indexes and every package they list, then
# check the signature against KEYRING and every hash with repository.py verify.
#
# Usage: check-site.sh URL TAG KEYRING [TIMEOUT_SECONDS]
set -euo pipefail

if [ $# -lt 3 ] || [ $# -gt 4 ]; then
    printf 'usage: %s URL TAG KEYRING [TIMEOUT_SECONDS]\n' "$0" >&2
    exit 2
fi
url=${1%/}
tag=$2
keyring=$(realpath "$3")
timeout=${4:-600}
packaging=$(realpath "$(dirname "$0")")
channel=${tag#*-}
channel=${channel%%.*}
version=${tag/-/"~"}-1
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# One fresh copy of the suite: its indexes, and every package they list.
mirror_suite() {
    local mirror=$1 architecture filename
    fetch() {
        mkdir -p "$(dirname "$mirror/$1")"
        # file: only for the tests' local sites.
        curl --proto '=https,file' --tlsv1.2 -fsSL --retry 3 -o "$mirror/$1" "$url/$1"
    }
    for name in InRelease Release Release.gpg; do
        fetch "dists/$channel/$name" || return 1
    done
    for architecture in amd64 arm64; do
        local index=dists/$channel/main/binary-$architecture/Packages
        fetch "$index" && fetch "$index.gz" || return 1
        if ! grep -qxF "Version: $version" "$mirror/$index"; then
            printf 'the %s index does not list torlek %s yet\n' "$architecture" "$version" >&2
            return 1
        fi
        while read -r filename; do
            fetch "$filename" || return 1
        done < <(sed -n 's/^Filename: //p' "$mirror/$index")
    done
}

# GitHub Pages serves a pushed branch after its own deployment, usually within a minute, and
# its cache may hand out an older file for a while: retry until one whole copy verifies.
deadline=$((SECONDS + timeout))
attempt=0
while :; do
    attempt=$((attempt + 1))
    mirror=$work/attempt-$attempt
    if mirror_suite "$mirror" &&
        python3 "$packaging/repository.py" verify --repo "$mirror" --suite "$channel" \
            --keyring "$keyring"; then
        printf 'check-site.sh: %s serves torlek %s, signed and complete\n' "$url" "$version"
        exit 0
    fi
    rm -rf "$mirror"
    if [ "$SECONDS" -ge "$deadline" ]; then
        printf 'check-site.sh: no verified copy of torlek %s at %s after %ss\n' \
            "$version" "$url" "$timeout" >&2
        exit 1
    fi
    sleep 20
done
