#!/usr/bin/env bash
# Keep the server box's Dante NIC for Dante traffic only. Run as root from an
# unpacked release directory: sudo DC_IFACE=<dante nic> ./dante-guard.sh
#
# 1. Firewall (dante-guard.nft, unit dante-guard.service, loaded before the
#    network), as dstack docs/two-networks.md section 3: out only to the
#    Dante subnet and Dante's groups, and only the dcontroller user, the
#    users in DC_GUARD_USERS (further Dante devices on this PC, space-
#    separated), IGMP, PTP and (on a DHCP leg) DHCP; inbound takes replies,
#    DHCP on a DHCP leg, PTP, mDNS (with unicast answers from 5353), UDP
#    multicast, the meter ports and, with a device user listed, the device
#    ports only.
# 2. ARP: the NIC answers and announces only its own address.
# 3. Routing (NetworkManager): no default route, no DNS, no IPv6 on the NIC;
#    its subnet lives in table 100 (metric 900), used only by sockets bound
#    to the NIC's address. Other services keep reaching hosts in that subnet through the
#    general LAN. DC_GUARD_SKIP_ROUTING=1 skips this step (NIC not managed by
#    NetworkManager, e.g. in CI).
# Idempotent: a re-run rewrites the same files and reloads.
set -euo pipefail

src=$(cd "$(dirname "$0")" && pwd)
: "${DC_IFACE:?set DC_IFACE to the Dante interface}"
[[ "$DC_IFACE" =~ ^[A-Za-z0-9._-]+$ ]] && [ -e "/sys/class/net/$DC_IFACE" ] \
    || { echo "no such interface: $DC_IFACE" >&2; exit 1; }
id dcontroller >/dev/null 2>&1 || { echo "user dcontroller missing: run install.sh first" >&2; exit 1; }
# Further Dante devices on this PC, by user (config guard_users; install.sh
# passes them): they may send on the NIC and get the device ports.
users='"dcontroller"'
devices=0
for u in ${DC_GUARD_USERS:-}; do
    [[ "$u" =~ ^[a-z_][a-z0-9_-]{0,31}$ ]] || { echo "not a user name: $u" >&2; exit 1; }
    id -u "$u" >/dev/null 2>&1 || { echo "no such user: $u" >&2; exit 1; }
    users="$users, \"$u\""
    devices=1
done
echo "dante-guard: Dante device users $users"
for f in dante-guard.nft dante-guard.service; do
    test -f "$src/$f" || { echo "missing $src/$f" >&2; exit 1; }
done

net=$(ip -4 -o route show dev "$DC_IFACE" scope link proto kernel table all | awk '{print $1; exit}')
[[ "$net" =~ ^[0-9.]+/[0-9]+$ ]] || { echo "no IPv4 subnet on $DC_IFACE" >&2; exit 1; }
addr=$(ip -4 -o addr show dev "$DC_IFACE" | awk '{split($4, a, "/"); print a[1]; exit}')
[[ "$addr" =~ ^[0-9.]+$ ]] || { echo "no IPv4 address on $DC_IFACE" >&2; exit 1; }
# DHCP rules only on a leg whose address came from DHCP.
dhcp=0
ip -4 -o addr show dev "$DC_IFACE" | grep -qw dynamic && dhcp=1

if [ "${DC_GUARD_SKIP_ROUTING:-0}" != 1 ]; then
    con=$(nmcli -g GENERAL.CONNECTION device show "$DC_IFACE")
    test -n "$con" || { echo "$DC_IFACE has no NetworkManager connection" >&2; exit 1; }
    echo "routing: connection '$con', subnet $net in table 100 for $addr, metric 900"
    nmcli connection modify "$con" ipv4.never-default yes ipv4.ignore-auto-dns yes \
        ipv4.route-table 100 ipv4.routing-rules "priority 100 from $addr table 100" \
        ipv4.route-metric 900 ipv6.method disabled
    nmcli connection up "$con" >/dev/null
fi

install -d -m 0755 /etc/dcontroller
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
sed -e "s/@IFACE@/$DC_IFACE/g" -e "s|@NET@|$net|g" -e "s|@USERS@|$users|" "$src/dante-guard.nft" > "$tmp"
[ "$dhcp" = 1 ] || sed -i '/# dhcp$/d' "$tmp"
[ "$devices" = 1 ] || sed -i '/# devices$/d' "$tmp"
nft -c -f "$tmp"
install -m 0644 "$tmp" /etc/dcontroller/dante-guard.nft
printf '# Dante NIC answers ARP only for its own address and announces only it.\n# Slash keys: an interface name may hold a dot (a VLAN, enp2s0.70).\nnet/ipv4/conf/%s/arp_ignore = 1\nnet/ipv4/conf/%s/arp_announce = 2\n' \
    "$DC_IFACE" "$DC_IFACE" > /etc/sysctl.d/60-dante-guard.conf
sysctl -q -p /etc/sysctl.d/60-dante-guard.conf
install -m 0644 "$src/dante-guard.service" /etc/systemd/system/dante-guard.service
systemctl daemon-reload
systemctl enable -q dante-guard.service
systemctl restart dante-guard.service
nft list table inet dante_guard >/dev/null
echo "dante-guard active on $DC_IFACE"
