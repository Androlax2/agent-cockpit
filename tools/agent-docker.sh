#!/usr/bin/env bash
# Isolated rootless Docker daemons for coding agents, one per project in ~/IdeaProjects.
# Each project gets a user agent-<project> with a uid in 2000-2999 (the range the nftables
# table agent_docker_egress confines to greyproxy), a daemon that sees only that project under
# /home, and a socket at <project>/.agent-docker.sock. Greywall mounts only the session's own
# project, so a session reaches its project's daemon and no other.
# Units: system/etc/systemd/system/agent-docker@.service and agent-docker-relay@.service.
set -euo pipefail

PROJECTS_DIR=/home/theo/IdeaProjects
OWNER=theo
GROUP=agent-docker
FIRST_UID=2000
LAST_UID=2999
EGRESS_TABLE="inet agent_docker_egress"

# Greyproxy intercepts TLS and its CA is deliberately not trusted system-wide, so the agent
# daemons get a bundle of their own (system roots plus Greyproxy's CA), named in the unit's
# SSL_CERT_FILE. Run setup again after regenerating Greyproxy's CA.
SYSTEM_CA_BUNDLE=/etc/ssl/cert.pem
GREYPROXY_CA=/home/theo/.local/share/greyproxy/ca-cert.pem
CA_BUNDLE_DIR=/var/lib/agent-docker
CA_BUNDLE=$CA_BUNDLE_DIR/ca-bundle.pem

die() { echo "agent-docker: $*" >&2; exit 1; }

require_root() { [[ $EUID -eq 0 ]] || die "run it with sudo"; }

require_valid_name() {
    [[ $1 =~ ^[a-z0-9][a-z0-9-]*$ ]] || die "invalid project name: $1"
}

require_egress_firewall() {
    nft list table $EGRESS_TABLE >/dev/null 2>&1 \
        || die "nftables table agent_docker_egress is not loaded: agents would have open network access"
}

next_free_uid() {
    local uid=$FIRST_UID
    while getent passwd "$uid" >/dev/null; do uid=$((uid + 1)); done
    (( uid <= LAST_UID )) || die "the agent uid range $FIRST_UID-$LAST_UID is full"
    echo "$uid"
}

write_ca_bundle() {
    [[ -r $GREYPROXY_CA ]] || die "greyproxy CA not found at $GREYPROXY_CA"
    install -d -m 755 "$CA_BUNDLE_DIR"
    cat "$SYSTEM_CA_BUNDLE" "$GREYPROXY_CA" > "$CA_BUNDLE.new"
    chmod 644 "$CA_BUNDLE.new"
    mv "$CA_BUNDLE.new" "$CA_BUNDLE"
    echo "agent-docker: CA bundle written to $CA_BUNDLE"
}

restart_running_daemons() {
    systemctl try-restart 'agent-docker@*.service'
}

setup() {
    require_root
    getent group "$GROUP" >/dev/null || groupadd --system "$GROUP"
    if ! id -nG "$OWNER" | tr ' ' '\n' | grep -qx "$GROUP"; then
        usermod -aG "$GROUP" "$OWNER"
        echo "added $OWNER to $GROUP: log out and back in before using the sockets"
    fi
    require_egress_firewall
    write_ca_bundle
    restart_running_daemons
    echo "agent-docker: ready"
}

add() {
    require_root
    local name=$1 user=agent-$1 project=$PROJECTS_DIR/$1
    require_valid_name "$name"
    [[ -d $project ]] || die "no such project: $project"
    getent group "$GROUP" >/dev/null || die "run make agent-docker-setup first"
    [[ -r $CA_BUNDLE ]] || die "no CA bundle at $CA_BUNDLE: run make agent-docker-setup first"
    require_egress_firewall
    if ! id "$user" >/dev/null 2>&1; then
        useradd --uid "$(next_free_uid)" --gid "$GROUP" --create-home --home-dir "/var/lib/$user" \
            --shell /usr/bin/nologin "$user"
    fi
    # useradd already gives regular users a subordinate id range (login.defs); add one only if it did not.
    if ! grep -q "^$user:" /etc/subuid; then
        local start
        start=$(awk -F: '{ end = $2 + $3; if (end > max) max = end } END { print (max < 200000 ? 200000 : max) }' /etc/subuid)
        usermod --add-subuids "$start-$((start + 65535))" --add-subgids "$start-$((start + 65535))" "$user"
    fi
    setfacl -R -m "u:$user:rwX,d:u:$user:rwX,d:u:$OWNER:rwX" "$project"
    systemctl enable --now "agent-docker@$name" "agent-docker-relay@$name"
    echo "agent-docker: $name ready, socket $project/.agent-docker.sock, greyproxy client $name-docker"
}

remove() {
    require_root
    local name=$1 user=agent-$1 project=$PROJECTS_DIR/$1
    require_valid_name "$name"
    systemctl disable --now "agent-docker-relay@$name" "agent-docker@$name" 2>/dev/null || true
    if [[ -d $project ]]; then
        setfacl -R -x "u:$user,d:u:$user" "$project" 2>/dev/null || true
        rm -f "$project/.agent-docker.sock"
    fi
    if id "$user" >/dev/null 2>&1; then
        userdel -r "$user"
    fi
    echo "agent-docker: $name removed (its images and containers are gone; greyproxy rules for $name-docker are kept)"
}

status() {
    local user name socket
    if [[ -r $CA_BUNDLE ]]; then
        echo "CA bundle: $CA_BUNDLE, written $(stat -c %y "$CA_BUNDLE" | cut -d. -f1)"
    else
        echo "CA bundle: missing (run make agent-docker-setup)"
    fi
    getent passwd | awk -F: -v first="$FIRST_UID" -v last="$LAST_UID" '$3 >= first && $3 <= last { print $1 }' |
        while read -r user; do
            name=${user#agent-}
            socket=$( [[ -S $PROJECTS_DIR/$name/.agent-docker.sock ]] && echo present || echo missing )
            printf '%-20s daemon=%-8s socket=%s\n' "$name" "$(systemctl is-active "agent-docker@$name" || true)" "$socket"
        done
}

case ${1:-} in
    setup) setup ;;
    add) add "${2:?usage: agent-docker.sh add <project>}" ;;
    remove) remove "${2:?usage: agent-docker.sh remove <project>}" ;;
    status) status ;;
    *) echo "usage: agent-docker.sh setup | add <project> | remove <project> | status" >&2; exit 2 ;;
esac
