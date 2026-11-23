#!/usr/bin/env bash
# Runs inside the guest as the throwaway "redteam" user, from "redteam-vm.sh create". It is
# the README's "Install from the command line", step 1, plus the daemon install; "torlek
# init" is left to the red-team session. Sent as one argument: commands get /dev/null.
#
# With TORLEK_REDTEAM_BUNDLE (a path relative to the home folder) and
# TORLEK_REDTEAM_BUNDLE_SHA256 set, it installs that release-format bundle, which the host
# uploaded over ssh and checksummed, instead of the latest published release.
set -euo pipefail

repository=Androlax2/agent-cockpit
bundle_file=${TORLEK_REDTEAM_BUNDLE:-}
download_dir=$(mktemp -d)
trap 'rm -rf "$download_dir"; [[ -z $bundle_file ]] || rm -f ~/"$bundle_file"' EXIT

if [[ -n $bundle_file ]]; then
    expected_sha256=${TORLEK_REDTEAM_BUNDLE_SHA256:?the host named no checksum for the bundle}
    archive_path=$HOME/$bundle_file
    archive=$(basename "$bundle_file" .tar.gz)
    actual_sha256=$(sha256sum "$archive_path" </dev/null | cut -d' ' -f1)
    if [[ $actual_sha256 != "$expected_sha256" ]]; then
        echo "bundle checksum mismatch: the host sent $expected_sha256, the guest received $actual_sha256; nothing installed" >&2
        exit 1
    fi
    # Only plain files and folders, all under the bundle's own folder with no ".." part, so
    # extracting cannot write elsewhere through a path or a link.
    entry_names=$(tar -tzf "$archive_path" </dev/null) || {
        echo "bundle is not a readable .tar.gz; nothing installed" >&2
        exit 1
    }
    entry_types=$(tar -tvzf "$archive_path" </dev/null | cut -c1)
    if [[ -z $entry_names ]] || grep -qv '^[-d]$' <<<"$entry_types" ||
        ! awk -v folder="$archive" '
            $0 != folder "/" && index($0, folder "/") != 1 { stray = 1 }
            $0 ~ /(^|\/)\.\.(\/|$)/ { stray = 1 }
            END { exit stray }' <<<"$entry_names"; then
        echo "bundle holds entries other than plain files and folders under $archive/; nothing installed" >&2
        exit 1
    fi
else
    version=$(curl -fsSL "https://api.github.com/repos/$repository/releases?per_page=1" </dev/null |
        sed -n 's/.*"tag_name": *"v\([^"]*\)".*/\1/p')
    archive="torlek-v${version:?GitHub named no release}-linux-$(uname -m)"
    archive_path=$download_dir/$archive.tar.gz
    cd "$download_dir"
    curl -fLO "https://github.com/$repository/releases/download/v$version/$archive.tar.gz" </dev/null
    curl -fLO "https://github.com/$repository/releases/download/v$version/SHA256SUMS" </dev/null
    grep -q " $archive.tar.gz\$" SHA256SUMS || {
        echo "SHA256SUMS has no line for $archive.tar.gz" >&2
        exit 1
    }
    sha256sum --check --ignore-missing SHA256SUMS
fi

mkdir -p ~/.local/lib/torlek ~/.local/bin
tar -xzf "$archive_path" -C ~/.local/lib/torlek
installed_torlek=~/.local/lib/torlek/"$archive"/bin/torlek

# A bundle built without TORLEK_SOURCE_REVISION says "revision unknown", which would leave the
# red-team's results naming no revision; the manifest's revision must be the binary's. Checked
# before the link, so a refused bundle is never on the PATH.
version_output=$("$installed_torlek" version </dev/null)
version_line=${version_output%%$'\n'*}
echo "$version_line"
if [[ -n $bundle_file ]]; then
    manifest_revision=$(sed -n 's/.*"source_revision": *"\([0-9a-f]\{40\}\)".*/\1/p' \
        ~/.local/lib/torlek/"$archive"/share/torlek/manifest.json)
    if [[ -z $manifest_revision || $version_line != *"revision $manifest_revision)" ]]; then
        echo "torlek version does not show the manifest's revision (${manifest_revision:-none}); build the bundle with TORLEK_SOURCE_REVISION set (docs/redteam-vm.md)" >&2
        exit 1
    fi
fi
ln -sfn "$installed_torlek" ~/.local/bin/torlek
export PATH=$HOME/.local/bin:$PATH

# The user manager may still be starting right after enable-linger.
for _ in $(seq 30); do
    systemctl --user show-environment >/dev/null 2>&1 </dev/null && break
    sleep 1
done
torlek daemon install </dev/null

# The install returns before the daemon's socket exists, and the next provisioning step talks
# to the daemon.
attempts=0
until torlek project list </dev/null >/dev/null 2>&1; do
    attempts=$((attempts + 1))
    if ((attempts > 30)); then
        echo "the daemon did not answer within 30 seconds of its install" >&2
        exit 1
    fi
    sleep 1
done
