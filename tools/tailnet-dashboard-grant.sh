#!/usr/bin/env bash
# Prints the grants that keep greyproxy's dashboard, published on this machine's tailnet port PORT
# (make agent-dashboard-tailnet), to one phone. Tailscale grants only add access: the default
# {"src": ["*"], "dst": ["*"], "ip": ["*"]} lets every device reach every port, so it is replaced by
# one that leaves PORT out, everywhere, and a second grant gives PORT on this machine back to the
# phone alone. Paste the result over the default grant in the admin console's access controls.
#   tools/tailnet-dashboard-grant.sh <phone name, as tailscale status shows it> <port>
set -euo pipefail

phone=${1:?usage: tailnet-dashboard-grant.sh <phone name> <port>}
port=${2:?usage: tailnet-dashboard-grant.sh <phone name> <port>}
[[ $port =~ ^[0-9]+$ ]] && (( port > 1 && port < 65535 )) || { echo "not a port between 2 and 65534: $port" >&2; exit 2; }

status=$(tailscale status --json) || { echo "tailscale status failed: is Tailscale up?" >&2; exit 1; }
own_addresses=$(jq -c '.Self.TailscaleIPs // []' <<< "$status")
phone_addresses=$(jq -c --arg phone "$phone" '
    [.Peer // {} | .[] | select(.HostName == $phone or ((.DNSName // "") | split(".")[0]) == ($phone | ascii_downcase))]
    | if length == 1 then .[0].TailscaleIPs else length end' <<< "$status")
case $phone_addresses in
    0) echo "no device named $phone in tailscale status" >&2; exit 1 ;;
    \[*) ;;
    *) echo "$phone_addresses devices named $phone in tailscale status: use the MagicDNS name" >&2; exit 1 ;;
esac
[[ $own_addresses != "[]" ]] || { echo "tailscale status gives no address for this machine" >&2; exit 1; }

jq -n --argjson own "$own_addresses" --argjson phone "$phone_addresses" --argjson port "$port" '
    {grants: [
        {src: ["*"], dst: ["*"],
         ip: ["tcp:1-\($port - 1)", "tcp:\($port + 1)-65535", "udp:*", "icmp:*", "58:*"]},
        {src: $phone, dst: $own, ip: ["tcp:\($port)"]}
    ]}'
echo "Replace the default grant with these two (the first keeps everything else as it was, except tcp:$port on every device); the second is $phone's." >&2
