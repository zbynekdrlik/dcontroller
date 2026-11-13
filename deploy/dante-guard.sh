#!/usr/bin/env bash
# Keep the server box's Dante NIC for Dante traffic only. Run as root from an
# unpacked release directory: sudo DC_IFACE=<dante nic> ./dante-guard.sh
#
# 1. Firewall: dstack's generic Dante-leg filter (dante-leg.nft, dstack
#    tools/linux at the pinned tag, docs/two-networks.md "Moving a PC to the
#    generic filter", dstack #264), the same file on every PC, naming no
#    device and no user. It is installed at /etc/dstack/dante-leg.nft and
#    loaded by dante-guard.service (before the network) with
#    `nft -D dante_leg=<iface>`. dcontroller's earlier per-box table
#    `inet dante_guard` is removed once the generic one is loaded.
# 2. avahi (when installed) does not announce on the NIC (deny-interfaces).
# 3. ARP: the NIC answers and announces only its own address.
# 4. Routing (NetworkManager): no default route, no DNS, no IPv6 on the NIC;
#    its subnet lives in table 100 (metric 900), used only by sockets bound
#    to the NIC's address. Other services keep reaching hosts in that subnet
#    through the general LAN. DC_GUARD_SKIP_ROUTING=1 skips this step (NIC not
#    managed by NetworkManager, e.g. in CI).
# Idempotent: a re-run rewrites the same files and reloads.
set -euo pipefail

src=$(cd "$(dirname "$0")" && pwd)
: "${DC_IFACE:?set DC_IFACE to the Dante interface}"
[[ "$DC_IFACE" =~ ^[A-Za-z0-9._-]+$ ]] && [ -e "/sys/class/net/$DC_IFACE" ] \
    || { echo "no such interface: $DC_IFACE" >&2; exit 1; }
for f in dante-leg.nft dante-guard.service; do
    test -f "$src/$f" || { echo "missing $src/$f" >&2; exit 1; }
done

addr=$(ip -4 -o addr show dev "$DC_IFACE" | awk '{split($4, a, "/"); print a[1]; exit}')
[[ "$addr" =~ ^[0-9.]+$ ]] || { echo "no IPv4 address on $DC_IFACE" >&2; exit 1; }

if [ "${DC_GUARD_SKIP_ROUTING:-0}" != 1 ]; then
    con=$(nmcli -g GENERAL.CONNECTION device show "$DC_IFACE")
    test -n "$con" || { echo "$DC_IFACE has no NetworkManager connection" >&2; exit 1; }
    echo "routing: connection '$con', subnet in table 100 for $addr, metric 900"
    nmcli connection modify "$con" ipv4.never-default yes ipv4.ignore-auto-dns yes \
        ipv4.route-table 100 ipv4.routing-rules "priority 100 from $addr table 100" \
        ipv4.route-metric 900 ipv6.method disabled
    nmcli connection up "$con" >/dev/null
fi

nft -c -D "dante_leg=$DC_IFACE" -f "$src/dante-leg.nft"
install -d -m 0755 /etc/dstack
install -m 0644 "$src/dante-leg.nft" /etc/dstack/dante-leg.nft
printf 'DANTE_LEG=%s\n' "$DC_IFACE" > /etc/dstack/dante-leg.env
printf '# Dante NIC answers ARP only for its own address and announces only it.\n# Slash keys: an interface name may hold a dot (a VLAN, enp2s0.70).\nnet/ipv4/conf/%s/arp_ignore = 1\nnet/ipv4/conf/%s/arp_announce = 2\n' \
    "$DC_IFACE" "$DC_IFACE" > /etc/sysctl.d/60-dante-guard.conf
sysctl -q -p /etc/sysctl.d/60-dante-guard.conf
install -m 0644 "$src/dante-guard.service" /etc/systemd/system/dante-guard.service
systemctl daemon-reload
systemctl enable -q dante-guard.service
systemctl restart dante-guard.service
nft list table inet dstack_dante_leg >/dev/null
echo "dante-guard: generic Dante-leg filter active on $DC_IFACE"

# The per-box table of dcontroller 0.1.60 and older goes once the generic one
# is loaded, so the NIC is never unguarded.
if nft list table inet dante_guard >/dev/null 2>&1; then
    nft delete table inet dante_guard
    echo "dante-guard: removed the earlier per-box table inet dante_guard"
fi
rm -f /etc/dcontroller/dante-guard.nft

# avahi must not announce this PC's services on the Dante leg.
conf=/etc/avahi/avahi-daemon.conf
if [ -f "$conf" ]; then
    cur=$(sed -n 's/^deny-interfaces=//p' "$conf" | head -1)
    if [[ ",$cur," != *",$DC_IFACE,"* ]]; then
        if grep -q '^deny-interfaces=' "$conf"; then
            sed -i "s/^deny-interfaces=.*/deny-interfaces=${cur:+$cur,}$DC_IFACE/" "$conf"
        else
            grep -q '^\[server\]' "$conf" || { echo "$conf has no [server] section" >&2; exit 1; }
            sed -i "/^\[server\]/a deny-interfaces=$DC_IFACE" "$conf"
        fi
        echo "avahi: deny-interfaces=$(sed -n 's/^deny-interfaces=//p' "$conf")"
        if systemctl is-active -q avahi-daemon.service; then systemctl restart avahi-daemon.service; fi
    fi
fi
