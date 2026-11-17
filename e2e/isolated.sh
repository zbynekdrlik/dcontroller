#!/usr/bin/env bash
# Runs a command (the E2E suite) in a network namespace of its own
# (dcontroller #95). The suite's server, its test device and the specs'
# packets (fake dstack peers, mDNS floods, 96 s Dante browsers) are all on
# one host and multicast loops back, so they need no real network; on a
# real one they reach every Dante device on it. Inside: `lo` and a dummy
# interface `e2e0` (multicast on, its own subnet, the default route), and
# nothing else. `playwright.config.ts` refuses to run anywhere else.
#
#   e2e/isolated.sh npx playwright test [args]
#
# Needs unprivileged user namespaces. Ubuntu 24.04 restricts them through
# AppArmor: CI lifts it (kernel.apparmor_restrict_unprivileged_userns=0);
# on dev1 this account has its own unshare with an AppArmor profile
# (airuleset#1337), used when present.
set -euo pipefail

if [ "${DC_E2E_NETNS:-}" != 1 ]; then
    u=/usr/local/lib/airuleset/userns/dcontroller/unshare
    [ -x "$u" ] || u=unshare
    exec "$u" --user --map-root-user --net --mount env DC_E2E_NETNS=1 "$0" "$@"
fi
# dstack reads an interface's MAC from /sys/class/net: a sysfs of this
# namespace, not the host's.
mount -t sysfs sysfs /sys
ip link set lo up
ip link add e2e0 type dummy
ip link set e2e0 multicast on up
ip addr add 10.199.0.1/24 dev e2e0
# A second address for the bridge test device (#108).
ip addr add 10.199.0.2/24 dev e2e0
ip route add default via 10.199.0.254 dev e2e0 onlink
exec "$@"
