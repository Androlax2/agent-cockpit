#!/usr/bin/env bash
# Runs inside the guest as the throwaway "redteam" user, from "redteam-vm.sh create". It is
# the README's "Install from the command line", step 1, plus the daemon install; "torlek
# init" is left to the red-team session. Sent as one argument: commands get /dev/null.
set -euo pipefail

repository=Androlax2/agent-cockpit
version=$(curl -fsSL "https://api.github.com/repos/$repository/releases?per_page=1" </dev/null |
    sed -n 's/.*"tag_name": *"v\([^"]*\)".*/\1/p')
archive="torlek-v${version:?GitHub named no release}-linux-$(uname -m)"

download_dir=$(mktemp -d)
trap 'rm -rf "$download_dir"' EXIT
cd "$download_dir"
curl -fLO "https://github.com/$repository/releases/download/v$version/$archive.tar.gz" </dev/null
curl -fLO "https://github.com/$repository/releases/download/v$version/SHA256SUMS" </dev/null
sha256sum --check --ignore-missing SHA256SUMS
mkdir -p ~/.local/lib/torlek ~/.local/bin
tar -xzf "$archive.tar.gz" -C ~/.local/lib/torlek
ln -sfn ~/.local/lib/torlek/"$archive"/bin/torlek ~/.local/bin/torlek

export PATH=$HOME/.local/bin:$PATH
torlek version </dev/null
torlek daemon install </dev/null
