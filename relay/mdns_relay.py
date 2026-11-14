#!/usr/bin/env python3
"""mDNS alarms to dstack's Claude session (dcontroller #88, dstack #286).

dcontroller's server logs a `status` change with `what` = `mdns` when an mDNS
sender on the Dante leg goes over the cap and when it is back under (#86).
The owner wants those alarms to reach dstack, which finds and fixes the
cause. The peer channel (airuleset `peer_channel.py`) is unix sockets on
dev1, checked by account, and the server runs on another box, so this relay
runs on dev1 as the `dcontroller` account, once every 30 s (a systemd user
timer): it reads the server's `/api/changes`, takes the mDNS entries newer
than its cursor and sends each to `dstack` through `peer_channel.py serve`
(MCP over stdio, `tools/call peer_send`, the path a Claude session uses).

One entry is one message: a start and an end, never a repeat while an
alarm lasts. The cursor (`state` file) moves only past a delivered entry,
so an alarm while dstack has no live session goes out when one is live,
in order. The first run starts at now: no history is replayed.

Settings, from the environment (the unit's env file, not the repo):
  DC_URL     the server, e.g. http://<tailnet address>:<port>
  DC_STATE   the cursor file (default ~/.local/state/dcontroller/mdns-relay.json)
  DC_PEER    peer_channel.py (default ~/devel/airuleset/peer_channel.py)
"""
import json
import logging
import os
import subprocess
import sys
import time
import urllib.request

TARGET = "dstack"
PROJECT = "dcontroller"
FETCH_TIMEOUT_S = 10
SEND_TIMEOUT_S = 30
log = logging.getLogger("mdns-relay")


def key(e):
    """An entry's identity within its second."""
    return "%s|%s|%s" % (e["at"], e.get("rx_label", ""), bool(e.get("ok")))


def pick(entries, cursor):
    """The mDNS alarm entries after `cursor`, oldest first."""
    out = [
        e for e in entries
        if e.get("kind") == "status" and e.get("what") == "mdns"
        and (e["at"] > cursor["at"] or (e["at"] == cursor["at"] and key(e) not in cursor["sent"]))
    ]
    out.sort(key=lambda e: (e["at"], key(e)))
    return out


def advance(cursor, e):
    """The cursor past `e`."""
    if e["at"] == cursor["at"]:
        return {"at": e["at"], "sent": sorted(set(cursor["sent"]) | {key(e)})}
    return {"at": e["at"], "sent": [key(e)]}


def stamp(at):
    return time.strftime("%d.%m.%Y %H:%M:%S %Z", time.localtime(at))


def message(e):
    """The peer message for one entry."""
    source = e.get("rx_label", "?")
    devices = e.get("rx_device") or source
    devices = "none listed" if devices == source else devices
    if e.get("ok"):
        return (
            "mDNS alarm ended (dcontroller #86/#88, dstack#286): %s is back under the cap "
            "of 50/s at %s: now %s, during the alarm %s. Dante devices at that address: %s."
            % (source, stamp(e["at"]), e.get("name_after"), e.get("name_before"), devices)
        )
    return (
        "mDNS alarm (dcontroller #86/#88, dstack#286): %s is over the cap of 50/s since %s: "
        "%s averaged over a whole minute. Dante devices at that address: %s (the sender may be "
        "another program on that PC). Counted by dcontroller's mDNS meter on its server's "
        "Dante address; one more message when it ends, no repeats while it lasts."
        % (source, stamp(e["at"]), e.get("name_after"), devices)
    )


def requests_for(text):
    """The MCP lines that send `text` to dstack, the session's own way."""
    lines = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize",
         "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                    "clientInfo": {"name": "dcontroller-mdns-relay", "version": "1"}}},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/call",
         "params": {"name": "peer_send", "arguments": {"to": TARGET, "text": text}}},
    ]
    return "".join(json.dumps(m) + "\n" for m in lines)


def answer(out):
    """(delivered, text) from the MCP server's output."""
    for line in out.splitlines():
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        if msg.get("id") != 2:
            continue
        if "error" in msg:
            return False, str(msg["error"])
        res = msg.get("result", {})
        text = " ".join(c.get("text", "") for c in res.get("content", []))
        return not res.get("isError", False), text
    return False, "no answer to peer_send"


def send(text, cmd):
    """Sends `text` to dstack through the peer channel run as `cmd`."""
    try:
        p = subprocess.run(cmd, input=requests_for(text), capture_output=True, text=True,
                           timeout=SEND_TIMEOUT_S)
    except (OSError, subprocess.TimeoutExpired) as e:
        return False, "peer channel did not run: %s" % e
    ok, said = answer(p.stdout)
    if not ok and p.returncode != 0:
        said += " (exit %d: %s)" % (p.returncode, p.stderr.strip()[-300:])
    return ok, said


def fetch(url):
    with urllib.request.urlopen(url + "/api/changes", timeout=FETCH_TIMEOUT_S) as r:
        return json.load(r)


def load(path):
    try:
        with open(path) as f:
            c = json.load(f)
        return {"at": int(c["at"]), "sent": list(c.get("sent", []))}
    except FileNotFoundError:
        return None


def save(path, cursor):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        json.dump(cursor, f)
    os.replace(tmp, path)


def run(url, state, cmd, now, get=fetch, sender=send):
    """One pass. Exit code: 0 done or waiting for dstack, 1 an error."""
    cursor = load(state)
    if cursor is None:
        cursor = {"at": int(now), "sent": []}
        save(state, cursor)
        log.info("first run: alarms after %s are sent, none before", stamp(cursor["at"]))
    try:
        entries = get(url)
    except (OSError, ValueError) as e:
        log.error("changes not read from %s: %s", url, e)
        return 1
    for e in pick(entries, cursor):
        ok, said = sender(message(e), cmd)
        if not ok:
            log.warning("alarm not delivered to %s, tried again next run: %s (source=%s at=%s)",
                        TARGET, said, e.get("rx_label"), e["at"])
            return 0
        cursor = advance(cursor, e)
        save(state, cursor)
        log.info("alarm %s sent to %s: source=%s rate=%s devices=%s (%s)",
                 "end" if e.get("ok") else "start", TARGET, e.get("rx_label"),
                 e.get("name_after"), e.get("rx_device"), said)
    return 0


def main():
    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(message)s")
    url = os.environ.get("DC_URL", "").rstrip("/")
    if not url:
        log.error("DC_URL is not set")
        return 2
    home = os.path.expanduser("~")
    state = os.environ.get("DC_STATE", os.path.join(home, ".local/state/dcontroller/mdns-relay.json"))
    peer = os.environ.get("DC_PEER", os.path.join(home, "devel/airuleset/peer_channel.py"))
    cmd = ["python3", "-I", peer, "serve", "--project", PROJECT]
    return run(url, state, cmd, time.time())


if __name__ == "__main__":
    sys.exit(main())
