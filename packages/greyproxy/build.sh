#!/usr/bin/env bash
# Build and install greyproxy from the dotfiles branch of github.com/Androlax2/greyproxy: v0.4.5
# plus the refusal of proxied traffic to greyproxy's own listeners (without it a sandbox reaches the
# API on 127.0.0.1:43080 through the proxy and can turn on allow-all), an optional API token
# (api-token in its data dir; requireApiToken off), per-client proxy secrets (requireClientSecret
# off) and a fail-closed start (it exits 1 when its service cannot start). Both checks stay off
# until greywall and the Docker service send the token and the secret.
# Each fix lands on that branch and goes upstream as a PR.
#   make greyproxy-patch
# Move the pins below to the next tip of that branch, or to an upstream release once it ships them.
# The running service keeps the old binary until it restarts (the script says how).
set -euo pipefail

build_dir="${XDG_CACHE_HOME:-$HOME/.cache}/dotfiles-build/greyproxy"
target="$HOME/.local/bin/greyproxy"

greyproxy_commit=a30e3d426133e77a24a0727833de4a6330d31327 # branch dotfiles
greyproxy_sha256=7518f18e5d52fcde3d0033560478e7026e1fed9ff8418f92c95f673567af5432
# Plain semver, no +dotfiles: greywall parses this version, treats anything else as outdated, and
# `greywall setup` would then replace this build with upstream's release. The commit line tells
# the builds apart.
greyproxy_version=0.4.5

[[ $(uname -m) == x86_64 ]] || { echo "greyproxy-patch: only x86_64 is pinned" >&2; exit 1; }

rm -rf "$build_dir"
mkdir -p "$build_dir/src"
curl -sSfL "https://github.com/Androlax2/greyproxy/archive/$greyproxy_commit.tar.gz" -o "$build_dir/greyproxy.tar.gz"
echo "$greyproxy_sha256  $build_dir/greyproxy.tar.gz" | sha256sum --check --quiet
tar -xzf "$build_dir/greyproxy.tar.gz" --strip-components=1 -C "$build_dir/src"

cd "$build_dir/src"
# An empty HOME, so the tests never touch the live greyproxy's data in ~/.local/share/greyproxy.
test_home="$build_dir/home"
mkdir -p "$test_home"
env -u XDG_CONFIG_HOME -u XDG_DATA_HOME -u XDG_CACHE_HOME \
    GOPATH="$(go env GOPATH)" GOCACHE="$(go env GOCACHE)" GOMODCACHE="$(go env GOMODCACHE)" HOME="$test_home" \
    go test -mod=readonly -count=1 ./...
CGO_ENABLED=0 go build -mod=readonly -trimpath \
    -ldflags "-s -w -X main.version=$greyproxy_version -X main.buildTime=$(date -u +%Y-%m-%dT%H:%M:%SZ) -X main.gitCommit=$greyproxy_commit" \
    -o "$build_dir/greyproxy" ./cmd/greyproxy

if [[ -e $target ]]; then
    cp --preserve=mode "$target" "$target.previous"
fi
install -m 0755 "$build_dir/greyproxy" "$target.new"
mv "$target.new" "$target"
"$target" --version | sed -n 3p
echo "greyproxy-patch: installed; the previous binary is $target.previous."
echo "greyproxy-patch: systemctl --user restart greyproxy (running sessions reconnect on their own)."
