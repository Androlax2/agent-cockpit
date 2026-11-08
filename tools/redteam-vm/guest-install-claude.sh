#!/usr/bin/env bash
# Runs inside the guest as root, from "redteam-vm.sh create", before Torlek's daemon is
# installed. It installs the provider program a section-1 sandboxed agent session runs (issue
# #1171): Anthropic's native Claude Code binary, at the version and SHA-256 the host script
# pins, passed in as CLAUDE_CODE_VERSION, CLAUDE_CODE_URL and CLAUDE_CODE_SHA256. Nothing from
# the host is copied in and no provider key is needed; the owner drops a credential only at run
# time. Sent as one argument: every command that could read stdin is given /dev/null.
#
# The program goes to a root-owned directory on the daemon's PATH, so the daemon lists it at
# its first start and the throwaway user cannot replace the file.
set -euo pipefail

: "${CLAUDE_CODE_VERSION:?the host names the version to install}"
: "${CLAUDE_CODE_URL:?the host names where to download it}"
: "${CLAUDE_CODE_SHA256:?the host names the checksum to verify it against}"
install_dir=${CLAUDE_CODE_INSTALL_DIR:-/usr/local/bin}

download_dir=$(mktemp -d)
trap 'rm -rf "$download_dir"' EXIT
archive=$download_dir/claude-code.tgz

curl --fail --silent --show-error --location --proto '=https' --output "$archive" "$CLAUDE_CODE_URL" </dev/null
actual_sha256=$(sha256sum "$archive" | cut -d' ' -f1)
if [[ $actual_sha256 != "$CLAUDE_CODE_SHA256" ]]; then
    echo "Claude Code checksum mismatch: expected $CLAUDE_CODE_SHA256, got $actual_sha256; nothing installed" >&2
    exit 1
fi

# The npm package of the native build holds the program as package/claude.
tar -xzf "$archive" -C "$download_dir" package/claude </dev/null
extracted_program=$download_dir/package/claude

# Refuse a build that is not the pinned version before installing it: the daemon's provider
# probe would otherwise fail later, in a place that does not name the cause.
extracted_version=$("$extracted_program" --version </dev/null | awk '{ print $1; exit }') || {
    echo "downloaded claude does not run on this guest (--version failed); nothing installed" >&2
    exit 1
}
if [[ $extracted_version != "$CLAUDE_CODE_VERSION" ]]; then
    echo "downloaded claude reports $extracted_version, not the pinned $CLAUDE_CODE_VERSION; nothing installed" >&2
    exit 1
fi
install -m 0755 "$extracted_program" "$install_dir/claude"
echo "Claude Code $extracted_version installed in $install_dir"
