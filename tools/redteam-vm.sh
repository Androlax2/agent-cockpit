#!/usr/bin/env bash
# Disposable QEMU/KVM guest for the Torlek red-team (#685, docs/redteam-vm.md).
#
# The guest is an Arch Linux cloud image with a throwaway user, Torlek's published release
# installed, and a "clean" snapshot to reset to. The guest gets no shared folder, no host
# device and no credential: its only inputs are the verified image, a generated SSH key and
# the cloud-init seed this script writes. Everything lives under $TORLEK_REDTEAM_DIR.
#
# Commands: fetch | create | start | ssh [COMMAND...] | check | stop | reset | status
#           | destroy [--all]
set -Eeuo pipefail

STATE_DIR=${TORLEK_REDTEAM_DIR:-$HOME/.local/share/torlek-redteam}
SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

# Pinned so a run is reproducible and a swapped image is refused. Bump both together from
# https://geo.mirror.pkgbuild.com/images/ (the .SHA256 file beside the image).
IMAGE_FILE_NAME=${TORLEK_REDTEAM_IMAGE_FILE:-Arch-Linux-x86_64-cloudimg-20261101.620452.qcow2}
IMAGE_URL=${TORLEK_REDTEAM_IMAGE_URL:-https://geo.mirror.pkgbuild.com/images/v20261101.620452/$IMAGE_FILE_NAME}
IMAGE_SHA256=${TORLEK_REDTEAM_IMAGE_SHA256:-794eb4889b39e1f0ed7d5ae998b2a1ed2137db8cf8c3476cf2962c7437ecb003}

SSH_PORT=${TORLEK_REDTEAM_SSH_PORT:-2222}
GUEST_MEMORY_MIB=${TORLEK_REDTEAM_MEMORY_MIB:-4096}
GUEST_CPUS=${TORLEK_REDTEAM_CPUS:-2}
GUEST_DISK_SIZE=${TORLEK_REDTEAM_DISK_SIZE:-30G}
BOOT_TIMEOUT_SECONDS=${TORLEK_REDTEAM_BOOT_TIMEOUT:-300}
KVM_DEVICE=${TORLEK_REDTEAM_KVM_DEVICE:-/dev/kvm}

ADMIN_USER=arch
RED_TEAM_USER=redteam
CLEAN_SNAPSHOT=clean

BASE_IMAGE=$STATE_DIR/$IMAGE_FILE_NAME
DISK=$STATE_DIR/disk.qcow2
SEED_IMAGE=$STATE_DIR/seed.iso
SSH_KEY=$STATE_DIR/id_ed25519
KNOWN_HOSTS=$STATE_DIR/known_hosts
PID_FILE=$STATE_DIR/qemu.pid
CONSOLE_LOG=$STATE_DIR/console.log

die() {
    echo "redteam-vm: $*" >&2
    exit 1
}

require_commands() {
    local missing=()
    for command_name in "$@"; do
        command -v "$command_name" >/dev/null || missing+=("$command_name")
    done
    ((${#missing[@]} == 0)) || die "missing on the host: ${missing[*]} (install them yourself; this script installs nothing)"
}

require_not_root() {
    [[ $EUID -ne 0 ]] || die "run it as your own user: the guest must never be started with root's rights"
}

require_kvm() {
    [[ -r $KVM_DEVICE && -w $KVM_DEVICE ]] || die "$KVM_DEVICE is not readable and writable for $(id -un)"
}

is_running() {
    [[ -f $PID_FILE ]] && kill -0 "$(<"$PID_FILE")" 2>/dev/null
}

require_stopped() {
    ! is_running || die "the guest is running; run: $0 stop"
}

# ssh with nothing taken from the host: no config file, no agent, no default keys, and the
# guest's own host key list, so the owner's ~/.ssh is neither read nor written.
guest_ssh() {
    local user=$1
    shift
    env -u SSH_AUTH_SOCK ssh -F /dev/null -i "$SSH_KEY" -p "$SSH_PORT" \
        -o IdentitiesOnly=yes -o IdentityAgent=none -o ForwardAgent=no -o ForwardX11=no \
        -o UserKnownHostsFile="$KNOWN_HOSTS" -o StrictHostKeyChecking=accept-new \
        -o BatchMode=yes -o ConnectTimeout=5 -o LogLevel=ERROR \
        "$user@127.0.0.1" "$@"
}

# Sends a guest script as one quoted argument, so a command inside it that reads stdin cannot
# swallow the rest of the script.
run_guest_script() {
    local user=$1 script_path=$2 command_prefix=${3:-}
    guest_ssh "$user" "$command_prefix bash -c $(printf '%q' "$(<"$script_path")")"
}

wait_for_ssh() {
    local user=$1 waited=0
    until guest_ssh "$user" true 2>/dev/null; do
        is_running || die "the guest stopped while booting; see $CONSOLE_LOG"
        ((waited < BOOT_TIMEOUT_SECONDS)) || die "no ssh from the guest after ${BOOT_TIMEOUT_SECONDS}s; see $CONSOLE_LOG"
        sleep 3
        waited=$((waited + 3))
    done
}

wait_for_exit() {
    local waited=0
    while is_running; do
        ((waited < BOOT_TIMEOUT_SECONDS)) || die "the guest did not power off within ${BOOT_TIMEOUT_SECONDS}s"
        sleep 1
        waited=$((waited + 1))
    done
}

# QEMU user-mode NAT, as the owner chose. It gives the guest the host's network position:
# the internet, the LAN and the host's loopback services through 10.0.2.2. QEMU's
# restrict=on closes that but also breaks the ssh port forward (checked on QEMU 11.1), so
# the docs tell the owner what the guest can reach and "check" measures it.
start_qemu() {
    qemu-system-x86_64 \
        -name torlek-redteam -machine q35,accel=kvm -cpu host \
        -smp "$GUEST_CPUS" -m "$GUEST_MEMORY_MIB" \
        -display none -serial "file:$CONSOLE_LOG" -monitor none \
        -drive "file=$DISK,if=virtio,format=qcow2,discard=unmap" \
        -drive "file=$SEED_IMAGE,if=virtio,format=raw,media=cdrom,readonly=on" \
        -netdev "user,id=net0,hostfwd=tcp:127.0.0.1:$SSH_PORT-:22" -device virtio-net-pci,netdev=net0 \
        -device virtio-rng-pci \
        -daemonize -pidfile "$PID_FILE"
}

stop_guest() {
    is_running || return 0
    guest_ssh "$ADMIN_USER" sudo systemctl poweroff 2>/dev/null || true
    local waited=0
    while is_running && ((waited < 30)); do
        sleep 1
        waited=$((waited + 1))
    done
    if is_running; then
        kill "$(<"$PID_FILE")"
        wait_for_exit
    fi
    rm -f "$PID_FILE"
}

fetch_image() {
    require_commands curl sha256sum
    mkdir -p "$STATE_DIR"
    chmod 700 "$STATE_DIR"
    if [[ -f $BASE_IMAGE ]]; then
        echo "redteam-vm: image already downloaded: $BASE_IMAGE"
        return
    fi
    local partial_download=$BASE_IMAGE.partial
    curl --fail --no-progress-meter --location --proto '=https' --output "$partial_download" "$IMAGE_URL"
    local actual_sha256
    actual_sha256=$(sha256sum "$partial_download" | cut -d' ' -f1)
    if [[ $actual_sha256 != "$IMAGE_SHA256" ]]; then
        rm -f "$partial_download"
        die "image checksum mismatch: expected $IMAGE_SHA256, got $actual_sha256; nothing kept"
    fi
    chmod 400 "$partial_download"
    mv "$partial_download" "$BASE_IMAGE"
    echo "redteam-vm: image verified: $BASE_IMAGE"
}

generate_throwaway_key() {
    ssh-keygen -q -t ed25519 -N '' -C torlek-redteam-throwaway -f "$SSH_KEY"
}

# Only the throwaway public key goes into the guest. The seed is built with bsdtar so no
# extra host tool is needed; cloud-init finds an ISO labelled CIDATA.
write_seed_image() {
    local seed_files public_key
    seed_files=$(mktemp -d "$STATE_DIR/seed.XXXXXX")
    public_key=$(<"$SSH_KEY.pub")
    cat >"$seed_files/meta-data" <<EOF
instance-id: torlek-redteam
local-hostname: torlek-redteam
EOF
    cat >"$seed_files/user-data" <<EOF
#cloud-config
ssh_pwauth: false
disable_root: true
ssh_authorized_keys:
  - $public_key
users:
  - default
  - name: $RED_TEAM_USER
    gecos: Throwaway red-team user
    shell: /bin/bash
    lock_passwd: true
    ssh_authorized_keys:
      - $public_key
EOF
    bsdtar --format iso9660 --options iso9660:volume-id=CIDATA -cf "$SEED_IMAGE" \
        -C "$seed_files" user-data meta-data
    rm -f "$seed_files/user-data" "$seed_files/meta-data"
    rmdir "$seed_files"
}

command_fetch() {
    fetch_image
}

command_create() {
    require_not_root
    require_kvm
    require_commands qemu-system-x86_64 qemu-img ssh ssh-keygen bsdtar
    [[ -f $BASE_IMAGE ]] || die "no verified image; run: $0 fetch"
    [[ ! -e $DISK ]] || die "a guest already exists; run: $0 destroy"
    require_stopped
    trap 'echo "redteam-vm: create failed; inspect $CONSOLE_LOG, then run: $0 destroy" >&2' ERR

    generate_throwaway_key
    write_seed_image
    rm -f "$KNOWN_HOSTS"
    qemu-img create -q -f qcow2 -F qcow2 -b "$BASE_IMAGE" "$DISK" "$GUEST_DISK_SIZE"

    echo "redteam-vm: first boot, to provision (a few minutes)"
    start_qemu
    wait_for_ssh "$ADMIN_USER"
    guest_ssh "$ADMIN_USER" 'sudo cloud-init status --wait' >/dev/null
    run_guest_script "$ADMIN_USER" "$SCRIPT_DIR/redteam-vm/guest-provision.sh" sudo
    run_guest_script "$RED_TEAM_USER" "$SCRIPT_DIR/redteam-vm/guest-install-torlek.sh"
    stop_guest
    qemu-img snapshot -c "$CLEAN_SNAPSHOT" "$DISK"
    echo "redteam-vm: guest created; snapshot '$CLEAN_SNAPSHOT' taken. Next: $0 start"
}

command_start() {
    [[ $# -eq 0 ]] || die "unknown start option: $1"
    require_not_root
    require_kvm
    require_commands qemu-system-x86_64 ssh
    [[ -f $DISK ]] || die "no guest; run: $0 create"
    require_stopped

    echo "redteam-vm: the guest has NAT network access: it reaches the internet, the LAN" >&2
    echo "redteam-vm: and host loopback services through 10.0.2.2 (docs/redteam-vm.md)." >&2
    start_qemu
    wait_for_ssh "$RED_TEAM_USER"
    echo "redteam-vm: guest up. Connect with: $0 ssh"
}

command_ssh() {
    is_running || die "the guest is not running; run: $0 start"
    guest_ssh "$RED_TEAM_USER" "$@"
}

command_check() {
    is_running || die "the guest is not running; run: $0 start"
    run_guest_script "$RED_TEAM_USER" "$SCRIPT_DIR/redteam-vm/guest-isolation-check.sh"
}

command_stop() {
    require_commands ssh
    stop_guest
    echo "redteam-vm: guest stopped"
}

command_reset() {
    require_commands qemu-img ssh
    [[ -f $DISK ]] || die "no guest; run: $0 create"
    stop_guest
    qemu-img snapshot -a "$CLEAN_SNAPSHOT" "$DISK"
    echo "redteam-vm: guest reset to snapshot '$CLEAN_SNAPSHOT'"
}

command_status() {
    echo "state dir: $STATE_DIR"
    if [[ -f $BASE_IMAGE ]]; then echo "image: downloaded"; else echo "image: missing"; fi
    if [[ -f $DISK ]]; then echo "guest: created"; else echo "guest: not created"; fi
    if is_running; then echo "running: yes (ssh on 127.0.0.1:$SSH_PORT)"; else echo "running: no"; fi
}

# Removes only the files this script writes, then the directory if nothing else is left, so a
# wrong TORLEK_REDTEAM_DIR cannot delete anything it did not create.
command_destroy() {
    require_commands ssh
    local remove_image=no
    case ${1:-} in
    "") ;;
    --all) remove_image=yes ;;
    *) die "unknown destroy option: $1" ;;
    esac
    stop_guest
    rm -f "$DISK" "$SEED_IMAGE" "$SSH_KEY" "$SSH_KEY.pub" "$KNOWN_HOSTS" "$CONSOLE_LOG" "$PID_FILE"
    if [[ $remove_image == yes ]]; then
        chmod u+w "$BASE_IMAGE" 2>/dev/null || true
        rm -f "$BASE_IMAGE" "$BASE_IMAGE.partial"
    fi
    rmdir "$STATE_DIR" 2>/dev/null || true
    echo "redteam-vm: guest destroyed (image kept unless --all)"
}

main() {
    local command_name=${1:-}
    [[ -n $command_name ]] || die "usage: $0 fetch | create | start | ssh [COMMAND...] | check | stop | reset | status | destroy [--all]"
    shift
    case $command_name in
    fetch | create | start | ssh | check | stop | reset | status | destroy) "command_$command_name" "$@" ;;
    *) die "unknown command: $command_name" ;;
    esac
}

main "$@"
