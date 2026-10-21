#!/usr/bin/env bash
# Build and install greywall from the dotfiles branch of github.com/Androlax2/greywall: v0.3.7 plus
# upstream PRs #103, #112, #113 and #114 (denyRead masks applied last). Without them a sandbox can
# read greyproxy's CA key and session key, and ~/.ssh/id_*.
#   make greywall-patch
# Move the pins below to the next tip of that branch, or to an upstream release once it ships them.
# Sessions started before the install keep the old mounts: restart them afterwards.
set -euo pipefail

build_dir="${XDG_CACHE_HOME:-$HOME/.cache}/dotfiles-build/greywall"
target="$HOME/.local/bin/greywall"

greywall_commit=16f1909f14f63ed5013ba88bafa2458bfe0059f4 # branch dotfiles
greywall_sha256=5867535cfcce9b204ccd1bcc2daf8a7b1920eddc2ae68a83d13a458f97a6b21c
# The tun2socks greywall embeds, as its Makefile pins it (same file v0.3.7's release ships).
tun2socks_url=https://github.com/xjasonlyu/tun2socks/releases/download/v2.5.2/tun2socks-linux-amd64.zip
tun2socks_sha256=4aa7737009a9f06b9f4957c4fc12932ae0cd2039471d2ae4e5d4666ffeb40a2e

[[ $(uname -m) == x86_64 ]] || { echo "greywall-patch: only x86_64 is pinned" >&2; exit 1; }

fetch_verified() {
    local url=$1 sha256=$2 destination=$3
    curl -sSfL "$url" -o "$destination"
    echo "$sha256  $destination" | sha256sum --check --quiet
}

rm -rf "$build_dir"
mkdir -p "$build_dir/src"
fetch_verified "https://github.com/Androlax2/greywall/archive/$greywall_commit.tar.gz" "$greywall_sha256" "$build_dir/greywall.tar.gz"
fetch_verified "$tun2socks_url" "$tun2socks_sha256" "$build_dir/tun2socks.zip"
tar -xzf "$build_dir/greywall.tar.gz" --strip-components=1 -C "$build_dir/src"

cd "$build_dir/src"
mkdir -p internal/sandbox/bin
unzip -q -o "$build_dir/tun2socks.zip" tun2socks-linux-amd64 -d internal/sandbox/bin

# On the host, the tests that start a real bubblewrap sandbox run too (they skip inside greywall).
# They get an empty HOME outside /tmp: bwrap cannot mount over our stowed ~/.gitconfig symlink,
# and greywall ignores paths under /tmp.
test_home="$build_dir/home"
mkdir -p "$test_home"
env -u XDG_CONFIG_HOME -u XDG_DATA_HOME -u XDG_CACHE_HOME \
    GOPATH="$(go env GOPATH)" GOCACHE="$(go env GOCACHE)" GOMODCACHE="$(go env GOMODCACHE)" HOME="$test_home" \
    go test -mod=readonly -count=1 ./...
CGO_ENABLED=0 go build -mod=readonly -trimpath \
    -ldflags "-s -w -X main.version=0.3.7+dotfiles -X main.buildTime=$(date -u +%Y-%m-%dT%H:%M:%SZ) -X main.gitCommit=$greywall_commit" \
    -o "$build_dir/greywall" ./cmd/greywall

if [[ -e $target ]]; then
    cp --preserve=mode "$target" "$target.previous"
fi
install -m 0755 "$build_dir/greywall" "$target.new"
mv "$target.new" "$target"
# sed, not head: head exits early, greywall gets SIGPIPE, and pipefail fails the script (141).
"$target" --version | sed -n 2p
echo "greywall-patch: installed; the previous binary is $target.previous. Restart the sessions."
