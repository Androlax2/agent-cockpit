#!/usr/bin/env bash
# Download one published release's SHA256SUMS and both Linux archives, then check the
# archives against the checksums and their build provenance (docs/packaging/deb.md, #563).
# Every .deb is built only from archives this script accepted.
#
# Usage: fetch-release.sh REPOSITORY TAG DIR
#   REPOSITORY is owner/name; TAG has no v; DIR/TAG must not exist. Needs gh and GH_TOKEN.
set -euo pipefail

if [ $# -ne 3 ]; then
    printf 'usage: %s REPOSITORY TAG DIR\n' "$0" >&2
    exit 2
fi
repository=$1
tag=$2
destination=$3/$tag
if [ -e "$destination" ]; then
    printf 'fetch-release.sh: %s exists\n' "$destination" >&2
    exit 1
fi
platforms=(linux-x86_64 linux-aarch64)
patterns=(-p SHA256SUMS)
for platform in "${platforms[@]}"; do
    patterns+=(-p "torlek-v$tag-$platform.tar.gz")
done
gh release download "v$tag" --repo "$repository" -D "$destination" "${patterns[@]}"
for platform in "${platforms[@]}"; do
    archive=torlek-v$tag-$platform.tar.gz
    (cd "$destination" && awk -v name="$archive" '$2 == name' SHA256SUMS | sha256sum --check --strict)
    gh attestation verify "$destination/$archive" --repo "$repository"
done
