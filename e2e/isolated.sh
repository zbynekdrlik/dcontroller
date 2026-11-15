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
# Needs unprivileged user namespaces (Ubuntu 24.04 restricts them through
# AppArmor: kernel.apparmor_restrict_unprivileged_userns=0, or a profile).
set -euo pipefail

if [ "${DC_E2E_NETNS:-}" != 1 ]; then
    exec unshare --user --map-root-user --net env DC_E2E_NETNS=1 "$0" "$@"
fi
ip link set lo up
ip link add e2e0 type dummy
ip link set e2e0 multicast on up
ip addr add 10.199.0.1/24 dev e2e0
ip route add default via 10.199.0.254 dev e2e0 onlink
exec "$@"
