#!/usr/bin/env bash
# Install or update dcontroller on the server box. Run as root from an
# unpacked release directory holding: dcontroller (binary), web/ (trunk dist),
# dcontroller.service. Leaves /etc/dcontroller/config.toml alone if present;
# otherwise writes it from DC_BIND and DC_IFACE.
set -euo pipefail

src=$(cd "$(dirname "$0")" && pwd)
test -x "$src/dcontroller" || { echo "missing $src/dcontroller" >&2; exit 1; }
test -f "$src/web/index.html" || { echo "missing $src/web/index.html" >&2; exit 1; }

id dcontroller >/dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin dcontroller

install -d -m 0755 /opt/dcontroller /etc/dcontroller
install -m 0755 "$src/dcontroller" /opt/dcontroller/dcontroller.new
rm -rf /opt/dcontroller/web.new
cp -r "$src/web" /opt/dcontroller/web.new
chmod -R a+rX /opt/dcontroller/web.new

if [ ! -f /etc/dcontroller/config.toml ]; then
    : "${DC_BIND:?set DC_BIND for the first install}" "${DC_IFACE:?set DC_IFACE for the first install}"
    printf 'bind = "%s"\nweb_dir = "/opt/dcontroller/web"\ndante_interface = "%s"\n' \
        "$DC_BIND" "$DC_IFACE" > /etc/dcontroller/config.toml
    chmod 0644 /etc/dcontroller/config.toml
fi

install -m 0644 "$src/dcontroller.service" /etc/systemd/system/dcontroller.service
systemctl daemon-reload

# Swap in place, then restart: the old process keeps its open files until then.
rm -rf /opt/dcontroller/web.old
[ -d /opt/dcontroller/web ] && mv /opt/dcontroller/web /opt/dcontroller/web.old
mv /opt/dcontroller/web.new /opt/dcontroller/web
mv /opt/dcontroller/dcontroller.new /opt/dcontroller/dcontroller
systemctl enable dcontroller.service >/dev/null
systemctl restart dcontroller.service
sleep 1
systemctl is-active dcontroller.service
