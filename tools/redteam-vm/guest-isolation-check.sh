#!/usr/bin/env bash
# Runs inside the guest as "redteam", from "redteam-vm.sh check". It reports what the guest
# can see of the host and fails when a host-sharing sign is present. The marker file shows
# whether the guest came from the clean snapshot: it must be absent on the first check after
# "create" or "reset", and present on a second check without one.
set -uo pipefail

failed=0
fail() {
    echo "FAIL: $*"
    failed=1
}

marker=$HOME/marker-after-clean
if [[ -e $marker ]]; then echo "marker: present (not a clean guest)"; else echo "marker: absent (clean guest)"; fi
touch "$marker"

echo "user: $(id)"
home_folders=$(find /home -mindepth 1 -maxdepth 1 -printf '%f ')
echo "homes: $home_folders"
echo "ssh dir: $(find ~/.ssh -mindepth 1 -maxdepth 1 -printf '%f ' 2>/dev/null)"
echo "torlek: $("$HOME/.local/bin/torlek" version 2>&1 | head -1)"
echo "torlek daemon: $(systemctl --user is-active torlek 2>&1)"

# Section 1 needs a provider program and, separately, a credential. Neither is a host-sharing
# sign, so a missing one is reported (and the program's absence fails the check, as the guest
# cannot run section 1), never mixed into the other. The daemon's own provider list is the
# authority on whether the agent is usable; only whether a credential file exists is read,
# never its content.
provider_list=$(PATH=$HOME/.local/bin:$PATH torlek provider list 2>&1 </dev/null)
if grep -qE '^claude\b' <<<"$provider_list"; then
    echo "agent claude: listed by the daemon ($(claude --version 2>&1 </dev/null | head -1))"
else
    echo "agent claude: NOT listed by the daemon"
    failed=1
fi
if [[ -n $(find ~/.config/torlek/provider-credentials/claude -maxdepth 1 -type f -size +0c 2>/dev/null) ]]; then
    echo "credential claude: present"
else
    echo "credential claude: absent"
fi

shared_mounts=$(grep -E ' (9p|virtiofs|fuse\.sshfs|nfs[0-9]*|cifs) ' /proc/mounts || true)
[[ -z $shared_mounts ]] || fail "shared mounts: $shared_mounts"
for home_folder in $home_folders; do
    [[ $home_folder == arch || $home_folder == redteam ]] || fail "unexpected home folder: $home_folder"
done
if sudo -n true 2>/dev/null; then fail "redteam has passwordless sudo"; fi
[[ -z $(find /dev/virtio-ports -mindepth 1 2>/dev/null) ]] || fail "a virtio serial channel to the host exists"

# What the host's network position gives the guest. Not a failure either way: it measures
# the reach docs/redteam-vm.md describes. The host listens on its loopback forward port for the
# guest's ssh while the guest runs, so an ssh banner from 10.0.2.2:<port> proves host loopback
# services are reachable.
if curl -sS -m 5 -o /dev/null https://example.com 2>/dev/null; then
    echo "internet: reachable"
else
    echo "internet: unreachable"
fi
forward_port=${TORLEK_REDTEAM_SSH_PORT:?the host passes its forward port}
host_banner=$(timeout 5 bash -c "exec 3<>/dev/tcp/10.0.2.2/$forward_port && head -c 8 <&3" 2>/dev/null)
if [[ -n $host_banner ]]; then
    echo "host loopback via 10.0.2.2: reachable (read $host_banner)"
else
    echo "host loopback via 10.0.2.2: unreachable"
fi

exit "$failed"
