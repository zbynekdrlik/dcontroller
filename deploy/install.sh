#!/usr/bin/env bash
# Install or update dcontroller on the server box. Run as root from an
# unpacked release directory holding: dcontroller (binary), web/ (trunk dist),
# dcontroller.service. Leaves /etc/dcontroller/config.toml alone if present;
# otherwise writes it from DC_BIND and DC_IFACE.
#
# On any failure after the swap starts (including the new version not
# answering /api/version), the previous binary and web are restored and
# restarted, and the script exits non-zero.
set -euo pipefail

src=$(cd "$(dirname "$0")" && pwd)
opt=/opt/dcontroller
etc=/etc/dcontroller
cfg=$etc/config.toml
test -x "$src/dcontroller" || { echo "missing $src/dcontroller" >&2; exit 1; }
test -f "$src/web/index.html" || { echo "missing $src/web/index.html" >&2; exit 1; }

id dcontroller >/dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin dcontroller

install -d -m 0755 "$opt"
install -d -m 0750 -o root -g dcontroller "$etc"

if [ ! -f "$cfg" ]; then
    : "${DC_BIND:?set DC_BIND for the first install}" "${DC_IFACE:?set DC_IFACE for the first install}"
    re='^(\[[0-9a-fA-F:]+\]|[0-9.]+):[0-9]+$'
    [[ "$DC_BIND" =~ $re ]] || { echo "bad DC_BIND: $DC_BIND" >&2; exit 1; }
    [[ "$DC_IFACE" =~ ^[A-Za-z0-9._-]+$ ]] && [ -e "/sys/class/net/$DC_IFACE" ] \
        || { echo "no such interface: $DC_IFACE" >&2; exit 1; }
    tmp=$(mktemp "$etc/.config.XXXXXX")
    printf 'bind = "%s"\nweb_dir = "%s/web"\ndante_interface = "%s"\n' "$DC_BIND" "$opt" "$DC_IFACE" > "$tmp"
    chown root:dcontroller "$tmp" && chmod 0640 "$tmp"
    mv "$tmp" "$cfg"
fi
chown root:dcontroller "$cfg" && chmod 0640 "$cfg"
bind=$(sed -n 's/^bind = "\(.*\)"$/\1/p' "$cfg")
bind=${bind:-127.0.0.1:8080}

healthy() {   # the service answers /api/version within ~15 s
    local i
    for i in $(seq 1 30); do
        if systemctl is-active -q dcontroller.service \
            && curl -fsS -m 2 "http://$bind/api/version" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.5
    done
    return 1
}

# Stage the new release next to the live one.
install -m 0755 "$src/dcontroller" "$opt/dcontroller.new"
rm -rf "$opt/web.new" && cp -r "$src/web" "$opt/web.new" && chmod -R a+rX "$opt/web.new"
install -m 0644 "$src/dcontroller.service" /etc/systemd/system/dcontroller.service.new

had_old=0
[ -x "$opt/dcontroller" ] && had_old=1

rollback() {
    echo "install failed; rolling back" >&2
    trap - ERR
    if [ "$had_old" = 1 ]; then
        [ -e "$opt/dcontroller.old" ] && mv -f "$opt/dcontroller.old" "$opt/dcontroller"
        if [ -d "$opt/web.old" ]; then rm -rf "$opt/web"; mv "$opt/web.old" "$opt/web"; fi
        [ -e /etc/systemd/system/dcontroller.service.old ] \
            && mv -f /etc/systemd/system/dcontroller.service.old /etc/systemd/system/dcontroller.service
        systemctl daemon-reload
        systemctl reset-failed dcontroller.service || true
        systemctl restart dcontroller.service || true
        healthy && echo "previous version restored and answering" >&2 || echo "previous version NOT answering" >&2
    else
        systemctl stop dcontroller.service || true
    fi
    exit 1
}
trap rollback ERR

# Swap: keep the previous release as *.old until the new one answers.
rm -rf "$opt/web.old" "$opt/dcontroller.old" /etc/systemd/system/dcontroller.service.old
if [ "$had_old" = 1 ]; then
    cp -p "$opt/dcontroller" "$opt/dcontroller.old"
    [ -f /etc/systemd/system/dcontroller.service ] \
        && cp -p /etc/systemd/system/dcontroller.service /etc/systemd/system/dcontroller.service.old
fi
[ -d "$opt/web" ] && mv "$opt/web" "$opt/web.old"
mv "$opt/web.new" "$opt/web"
mv -f "$opt/dcontroller.new" "$opt/dcontroller"
mv -f /etc/systemd/system/dcontroller.service.new /etc/systemd/system/dcontroller.service
systemctl daemon-reload
systemctl enable -q dcontroller.service
systemctl reset-failed dcontroller.service 2>/dev/null || true
systemctl restart dcontroller.service
healthy || false
trap - ERR
echo "dcontroller $(curl -fsS -m 2 "http://$bind/api/version") active"
