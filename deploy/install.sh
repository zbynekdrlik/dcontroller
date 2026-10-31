#!/usr/bin/env bash
# Install or update dcontroller on the server box. Run as root from an
# unpacked release directory holding: dcontroller (binary), web/ (trunk dist),
# dcontroller.service. Keeps /etc/dcontroller/config.toml if present (it must
# pass `dcontroller --check-config`); otherwise writes it from DC_BIND and
# DC_IFACE.
#
# Once the swap starts, any failure or signal (including the new version not
# answering /api/version) restores the previous binary, web and unit and
# restarts them; a failed FIRST install removes everything it created. The
# script then exits non-zero.
set -euo pipefail

src=$(cd "$(dirname "$0")" && pwd)
opt=/opt/dcontroller
etc=/etc/dcontroller
cfg=$etc/config.toml
unit=/etc/systemd/system/dcontroller.service
test -x "$src/dcontroller" || { echo "missing $src/dcontroller" >&2; exit 1; }
test -f "$src/web/index.html" || { echo "missing $src/web/index.html" >&2; exit 1; }

id dcontroller >/dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin dcontroller
install -d -m 0755 "$opt"
install -d -m 0750 -o root -g dcontroller "$etc"

created_cfg=0
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
    created_cfg=1
fi
chown root:dcontroller "$cfg" && chmod 0640 "$cfg"
# The new binary must accept the config; it also tells us where to probe.
if ! bind=$("$src/dcontroller" --check-config "$cfg"); then
    echo "config $cfg refused by the new version" >&2
    [ "$created_cfg" = 1 ] && rm -f "$cfg"
    exit 1
fi

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
install -m 0644 "$src/dcontroller.service" "$unit.new"

had_old=0
[ -x "$opt/dcontroller" ] && had_old=1

step() { "$@" || echo "rollback step failed: $*" >&2; }

rolled=0
rollback() {
    [ "$rolled" = 1 ] && exit 1
    rolled=1
    trap - ERR INT TERM HUP
    set +e
    echo "install failed; rolling back" >&2
    if [ "$had_old" = 1 ]; then
        [ -e "$opt/dcontroller.old" ] && step mv -f "$opt/dcontroller.old" "$opt/dcontroller"
        if [ -d "$opt/web.old" ]; then step rm -rf "$opt/web"; step mv "$opt/web.old" "$opt/web"; fi
        [ -e "$unit.old" ] && step mv -f "$unit.old" "$unit"
        step systemctl daemon-reload
        systemctl reset-failed dcontroller.service 2>/dev/null
        step systemctl restart dcontroller.service
        if healthy; then echo "previous version restored and answering" >&2
        else echo "previous version NOT answering" >&2; fi
    else
        systemctl disable --now -q dcontroller.service 2>/dev/null
        step rm -rf "$opt/dcontroller" "$opt/web" "$opt/web.old" "$unit"
        [ "$created_cfg" = 1 ] && step rm -f "$cfg"
        step systemctl daemon-reload
        systemctl reset-failed dcontroller.service 2>/dev/null
        echo "first install removed" >&2
    fi
    rm -rf "$opt/dcontroller.new" "$opt/web.new" "$unit.new"
    exit 1
}
trap rollback ERR INT TERM HUP

# Swap: keep the previous release as *.old until the new one answers.
rm -rf "$opt/web.old" "$opt/dcontroller.old" "$unit.old"
if [ "$had_old" = 1 ]; then
    cp -p "$opt/dcontroller" "$opt/dcontroller.old"
    if [ -f "$unit" ]; then cp -p "$unit" "$unit.old"; fi
fi
if [ -d "$opt/web" ]; then mv "$opt/web" "$opt/web.old"; fi
mv "$opt/web.new" "$opt/web"
mv -f "$opt/dcontroller.new" "$opt/dcontroller"
mv -f "$unit.new" "$unit"
systemctl daemon-reload
systemctl enable -q dcontroller.service
systemctl reset-failed dcontroller.service 2>/dev/null || true
systemctl restart dcontroller.service
healthy || false
served=$(curl -fsS -m 2 "http://$bind/api/version")
trap - ERR INT TERM HUP
echo "dcontroller $served active"
