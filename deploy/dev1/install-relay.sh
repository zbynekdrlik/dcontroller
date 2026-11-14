#!/usr/bin/env bash
# Install or update the mDNS alarm relay (dcontroller #88) as the account that
# holds dcontroller's peer channel: DC_URL=http://<server>:<port> ./install-relay.sh
# from a checkout. Keeps an existing env file; the cursor (state) stays.
set -euo pipefail
src=$(cd "$(dirname "$0")/../.." && pwd)
lib=$HOME/.local/lib/dcontroller
units=$HOME/.config/systemd/user
env=$HOME/.config/dcontroller/mdns-relay.env
python3 -m unittest discover -s "$src/relay" >/dev/null 2>&1 \
    || { echo "relay tests fail; nothing installed" >&2; exit 1; }
install -d "$lib" "$units" "$(dirname "$env")"
install -m 0644 "$src/relay/mdns_relay.py" "$lib/mdns_relay.py"
install -m 0644 "$src/deploy/dev1/dcontroller-mdns-relay.service" "$src/deploy/dev1/dcontroller-mdns-relay.timer" "$units/"
if [ ! -f "$env" ]; then
    : "${DC_URL:?set DC_URL for the first install}"
    printf 'DC_URL=%s\n' "$DC_URL" > "$env"
    chmod 0600 "$env"
fi
systemctl --user daemon-reload
systemctl --user enable -q --now dcontroller-mdns-relay.timer
systemctl --user start dcontroller-mdns-relay.service
systemctl --user is-active -q dcontroller-mdns-relay.timer
echo "mDNS relay active: $(sed -n 's/^DC_URL=//p' "$env") -> dstack, every 30 s"
