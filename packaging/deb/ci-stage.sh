#!/usr/bin/env bash
# The Debian workflow's staging (#563, docs/packaging/deb.md): build the torlek packages of one
# published release, and of the release before it when one is given, check them with dpkg-deb
# and lintian, then sign three APT repositories with throwaway keys for the install job: a
# good one, one signed by a key the client lacks, and one whose newest packages changed after
# signing. The keys live in a temporary GNUPGHOME and only the good key's public half is kept.
#
# Usage: ci-stage.sh INPUTS FIXTURE TAG DATE [PREVIOUS_TAG PREVIOUS_DATE]
#   INPUTS/TAG holds that release's SHA256SUMS and both Linux archives, already checked
#   against the checksums and their build provenance; DATE is its publication day. FIXTURE
#   must not exist.
set -euo pipefail

if [ $# -ne 4 ] && [ $# -ne 6 ]; then
    printf 'usage: %s INPUTS FIXTURE TAG DATE [PREVIOUS_TAG PREVIOUS_DATE]\n' "$0" >&2
    exit 2
fi
inputs=$(realpath "$1")
fixture=$2
tags=("$3")
dates=("$4")
if [ $# -eq 6 ]; then
    tags+=("$5")
    dates+=("$6")
fi
packaging=$(realpath "$(dirname "$0")")
# Debian policy wants an address; .invalid cannot be anyone's (RFC 2606).
maintainer='Torlek CI fixture <ci-fixture@example.invalid>'
channel=${tags[0]#*-}
channel=${channel%%.*}
if [ -e "$fixture" ]; then
    printf 'ci-stage.sh: %s exists\n' "$fixture" >&2
    exit 1
fi
mkdir -p "$fixture"
fixture=$(realpath "$fixture")
pool=$fixture/good/pool/main/t/torlek
newest_version=${tags[0]/-/"~"}-1

for index in "${!tags[@]}"; do
    tag=${tags[$index]}
    for platform in linux-x86_64 linux-aarch64; do
        python3 "$packaging/build.py" --tag "$tag" --sums "$inputs/$tag/SHA256SUMS" \
            --archive "$inputs/$tag/torlek-v$tag-$platform.tar.gz" \
            --maintainer "$maintainer" --date "${dates[$index]}" --out "$pool"
    done
done

for package in "$pool"/*.deb; do
    dpkg-deb --info "$package"
    # Errors fail the job; every other finding is printed for the publication checklist.
    lintian --fail-on error --display-info --tag-display-limit 0 "$package"
done

# The programs' own shared-library needs, on the runner's architecture: every term must be
# in the package's Depends exactly, so a raised glibc floor cannot pass unseen.
newest_amd64=$pool/torlek_${newest_version}_amd64.deb
shlibs=$(mktemp -d)
dpkg-deb --extract "$newest_amd64" "$shlibs/root"
mkdir "$shlibs/debian"
printf 'Source: torlek\n\nPackage: torlek\nArchitecture: any\n' >"$shlibs/debian/control"
needed=$(cd "$shlibs" && dpkg-shlibdeps -O "$shlibs"/root/usr/lib/torlek/bin/*)
printf 'dpkg-shlibdeps: %s\n' "$needed"
declared=$(dpkg-deb --field "$newest_amd64" Depends)
IFS=',' read -ra terms <<<"${needed#shlibs:Depends=}"
for term in "${terms[@]}"; do
    term=${term# }
    if [[ ", $declared," != *", $term,"* ]]; then
        printf 'Depends %q lacks the dpkg-shlibdeps term %q\n' "$declared" "$term" >&2
        exit 1
    fi
done

GNUPGHOME=$(mktemp -d)
export GNUPGHOME
trap 'gpgconf --kill gpg-agent; rm -rf "$GNUPGHOME"' EXIT
throwaway_key() {
    gpg --batch --pinentry-mode loopback --passphrase '' \
        --quick-gen-key "$1" ed25519 sign never 2>/dev/null
    gpg --batch --with-colons --list-keys "$1" | awk -F: '$1 == "fpr" { print $10; exit }'
}
key=$(throwaway_key 'Torlek CI fixture <ci-fixture@example.invalid>')
other_key=$(throwaway_key 'Someone else <ci-other@example.invalid>')
gpg --batch --armor --export "$key" >"$fixture/torlek-test.asc"

python3 "$packaging/repository.py" index --repo "$fixture/good" --suite "$channel"
python3 "$packaging/repository.py" sign --repo "$fixture/good" --suite "$channel" --key "$key"
python3 "$packaging/repository.py" verify --repo "$fixture/good" --suite "$channel" \
    --keyring "$fixture/torlek-test.asc"

# apt refuses this index before fetching any package, so it needs no pool.
mkdir -p "$fixture/wrong-key/dists"
cp -r "$fixture/good/dists/$channel" "$fixture/wrong-key/dists/"
python3 "$packaging/repository.py" sign --repo "$fixture/wrong-key" --suite "$channel" \
    --key "$other_key"

# Only the newest packages, which apt would fetch, each with one byte changed, same size.
mkdir -p "$fixture/tampered/pool/main/t/torlek"
cp -r "$fixture/good/dists" "$fixture/tampered/"
for package in "$pool"/torlek_"$newest_version"_*.deb; do
    changed=$fixture/tampered/pool/main/t/torlek/$(basename "$package")
    cp "$package" "$changed"
    size=$(stat -c %s "$changed")
    printf 'X' | dd of="$changed" bs=1 seek=$((size - 1)) conv=notrunc status=none
done
if python3 "$packaging/repository.py" verify --repo "$fixture/tampered" --suite "$channel" \
    --keyring "$fixture/torlek-test.asc" 2>/dev/null; then
    printf '%s\n' 'ci-stage.sh: the tampered repository still verifies' >&2
    exit 1
fi

sha256sum "$pool"/*.deb
