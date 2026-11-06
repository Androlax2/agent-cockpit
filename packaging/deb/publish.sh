#!/usr/bin/env bash
# The release workflow's APT publication (#563, docs/packaging/deb.md#publication): add one
# release's packages to the APT site, sign it and push it as the single commit of the site's
# Pages branch. Run only by the release workflow's publish-apt job, in the protected `apt`
# environment; nothing else holds the signing subkey.
#
# Usage: publish.sh INPUTS TAG DATE
#   INPUTS/TAG holds the release's SHA256SUMS and both Linux archives, checked by
#   fetch-release.sh; DATE is its publication day.
# Environment:
#   TORLEK_APT_REMOTE       the site's Git remote (the Pages repository)
#   TORLEK_APT_MAINTAINER   the packages' `Name <address>`
#   TORLEK_APT_SIGNING_KEY  the armored secret signing subkey, without a passphrase
#   TORLEK_APT_PUBLIC_KEY   the committed public key (default: packaging/deb/torlek.asc)
#   TORLEK_APT_KEEP         versions the suite keeps (default: 10)
set -euo pipefail

refuse() {
    printf 'publish.sh: %s\n' "$1" >&2
    exit 1
}
if [ $# -ne 3 ]; then
    printf 'usage: %s INPUTS TAG DATE\n' "$0" >&2
    exit 2
fi
inputs=$(realpath "$1")
tag=$2
date=$3
packaging=$(realpath "$(dirname "$0")")
remote=${TORLEK_APT_REMOTE:?publish.sh: set TORLEK_APT_REMOTE}
maintainer=${TORLEK_APT_MAINTAINER:?publish.sh: set TORLEK_APT_MAINTAINER}
signing_key=${TORLEK_APT_SIGNING_KEY:?publish.sh: set TORLEK_APT_SIGNING_KEY}
unset TORLEK_APT_SIGNING_KEY
public_key=$(realpath "${TORLEK_APT_PUBLIC_KEY:-$packaging/torlek.asc}")
keep=${TORLEK_APT_KEEP:-10}
branch=gh-pages
channel=${tag#*-}
channel=${channel%%.*}
if [ ! -s "$public_key" ]; then
    refuse "$public_key is missing: commit the public key first (docs/packaging/deb.md#signing-key)"
fi

work=$(mktemp -d)
GNUPGHOME=$work/gnupg
export GNUPGHOME
mkdir -m 0700 "$GNUPGHOME"
trap 'gpgconf --kill gpg-agent 2> /dev/null; rm -rf "$work"' EXIT
gpg --batch --quiet --import <<<"$signing_key"
unset signing_key

# Each usable signing (sub)key: its secret is present (not a `#` stub) and it can sign. Every
# one must belong to the committed public key, so a rotation signs with both of its keys.
published_fingerprints=$(gpg --batch --with-colons --show-keys "$public_key" |
    awk -F: '$1 == "fpr" { print $10 }')
signing_fingerprints=$(gpg --batch --with-colons --list-secret-keys |
    awk -F: '/^(sec|ssb):/ { usable = ($12 ~ /s/ && $15 != "#") } $1 == "fpr" && usable { print $10; usable = 0 }')
if [ -z "$signing_fingerprints" ]; then
    refuse 'TORLEK_APT_SIGNING_KEY holds no secret key that can sign'
fi
key_options=()
for fingerprint in $signing_fingerprints; do
    if ! grep -qxF "$fingerprint" <<<"$published_fingerprints"; then
        refuse "signing key $fingerprint is not in $public_key"
    fi
    key_options+=(--key "$fingerprint")
done

# The published site is the pool's only copy: check it against the committed key first.
site=$work/site
lease=
if git ls-remote --exit-code --heads "$remote" "$branch" >"$work/head"; then
    lease=$(cut -f1 "$work/head")
    git clone --quiet --depth 1 --branch "$branch" "$remote" "$work/clone"
    mv "$work/clone" "$site"
    rm -rf "$site/.git"
    for suite in "$site"/dists/*/; do
        [ -d "$suite" ] || continue
        python3 "$packaging/repository.py" verify --repo "$site" --suite "$(basename "$suite")" \
            --keyring "$public_key"
    done
    # index signs every package in the pool, so one no verified index lists would gain a
    # signature it never had.
    if [ -d "$site/pool" ]; then
        indexes=("$site"/dists/*/main/binary-*/Packages)
        listed=
        if [ -e "${indexes[0]}" ]; then
            listed=$(sed -n 's/^Filename: //p' "${indexes[@]}" | sort -u)
        fi
        unlisted=$(cd "$site" && find pool -type f | sort | comm -23 - <(printf '%s\n' "$listed"))
        if [ -n "$unlisted" ]; then
            refuse "the site's pool holds files no signed index lists: $(tr '\n' ' ' <<<"$unlisted")"
        fi
    fi
else
    status=$?
    # 2: the remote answered and has no such branch, so this is the first publication.
    if [ "$status" -ne 2 ]; then
        refuse "git ls-remote $remote failed with $status"
    fi
    mkdir "$site"
fi

pool=$site/pool/main/t/torlek
for platform in linux-x86_64 linux-aarch64; do
    python3 "$packaging/build.py" --tag "$tag" --sums "$inputs/$tag/SHA256SUMS" \
        --archive "$inputs/$tag/torlek-v$tag-$platform.tar.gz" \
        --maintainer "$maintainer" --date "$date" --out "$pool"
done
python3 "$packaging/repository.py" prune --repo "$site" --suite "$channel" --keep "$keep"
python3 "$packaging/repository.py" index --repo "$site" --suite "$channel"
python3 "$packaging/repository.py" sign --repo "$site" --suite "$channel" "${key_options[@]}"
python3 "$packaging/repository.py" verify --repo "$site" --suite "$channel" --keyring "$public_key"
cp "$public_key" "$site/torlek.asc"
# GitHub Pages serves the files as they are, without a Jekyll build.
touch "$site/.nojekyll"

# One commit, no history: earlier packages would otherwise stay in the repository forever.
git -C "$site" init --quiet --initial-branch "$branch"
git -C "$site" add --all
git -C "$site" -c user.name='github-actions[bot]' \
    -c user.email='41898282+github-actions[bot]@users.noreply.github.com' \
    commit --quiet -m "Publish torlek $tag to the $channel suite"
git -C "$site" push --quiet --force-with-lease="refs/heads/$branch:$lease" "$remote" \
    "HEAD:refs/heads/$branch"
printf 'Published torlek %s to the %s suite of %s\n' "$tag" "$channel" "$remote"
