# dcontroller handover

Request: zbynekdrlik/airuleset#1273, owner 30.10.2026 and 31.10.2026.

## Status

Cargo workspace: `crates/server` (axum: PWA, `/api/version`, `/api/devices`, `/healthz`), `crates/web` (Leptos CSR, built by trunk, PWA manifest + service worker), `crates/api` (the JSON types both share), `crates/buildver` (one version string from git for both). Live behind Cloudflare Access (#1).

Reads: `crates/server/src/dante.rs` runs dstack discovery on the IPv4 of `dante_interface` and reads each device when listed, when its records change and every 30 s. Every read covers the channels, subscriptions and settings (ARC), plus the clock status and network info (UDP 8700). The product info, board info, encoding and sample rates are read once per listing. The page polls `/api/devices` every 3 s. Pages: Overview (start), Routing, Devices, Latency, Meters, Clock, Network, Changes; the web is a sidebar shell with one stylesheet (`crates/web/public/style.css`, tokens on `:root`).

Latency history: `crates/server/src/latency.rs` records every receiver all the time from one heartbeat listener (dstack `latency::Listener`) into SQLite (`history_db`, default `/var/lib/dcontroller/history.db`; `crates/server/src/history.rs`): seconds for 24 h, minutes for 30 days, hours for a year, rolled up every minute; schema steps in `PRAGMA user_version`, never edited once shipped. `/api/history/{device}?from&to` buckets any range to ≤ 1500 points; `/api/overview` sums the last hour and day per receiver. The Latency page reads device to device (`hist::by_source`: one series per source device, the worst peak against each flow's own latency, late packets added). Late packets, the receiver's own count of packets past their flow's latency, are the verdict: health is bad only with late packets, a peak is only the margin. A flow's latency from ARC 0x3200 is already the larger of receiver and transmitter latency. Charts are uPlot 1.6.30 (MIT, vendored in `crates/web/public/vendor/`) driven by `crates/web/public/charts.js`.

mDNS senders (#86, dstack #289): `crates/server/src/mdns.rs` runs dstack's passive mDNS meter (`discover::senders::Meter`) on our Dante address; dstack's devices put the same numbers in their status, but that is each PC's local socket. `/api/mdns` holds the last closed minute (all senders a second, the 5 loudest, every source over 50/s, with the listed devices at each address; ours is `this server`). A source over the cap shows an alarm on every page and logs a `status` change (`what` = `mdns`) when it starts and when it ends. The Network page lists the senders. The E2E floods the group from the routed address and waits up to two meter minutes each way.

mDNS alarms to dstack (#88, dstack #286, the owner's wish: alarms go to dstack, not to him): `relay/mdns_relay.py` runs on the PC holding the peer channel as the `dcontroller` account (a systemd user timer, every 30 s; `deploy/dev1/install-relay.sh`, the server URL in `~/.config/dcontroller/mdns-relay.env`). It reads `/api/changes`, sends each new `mdns` status entry (start, end) to `dstack` through `peer_channel.py serve` (MCP `tools/call peer_send`), and keeps a cursor in `~/.local/state/dcontroller/mdns-relay.json` that moves only past a delivered entry. Its journal: `journalctl --user -u dcontroller-mdns-relay`.

96 s browsers and the network floor (#93, dstack v0.6.145, dstack #319/#321): the same meter lists the hosts that refresh Dante records every ~96 s (`Minute::browsers`) and the dstack peers below the network floor 0.6.58 heard within the hour (`Meter::below_floor`; dropped at once when the peer answers with a newer version). The server logs a `status` entry at each one's start and end (`what` = `mdns-browser` / `network-floor`), the page shows a banner and two lists on the Network page, and the relay sends both to dstack like the cap alarm. A local client below the floor (dtray, `kind: client`, #97, dstack #323) is announced by its device in its ARC record (`dstack_old_client`, from v0.6.146): `State::old_clients()` reads it from the listed devices, named `client of <device>` at the device's address. Since v0.6.146 the meter no longer names Dante hardware (Audinate firmware refreshes every 96 s) as a browser (#98).

Writes: only in `crates/server/src/change.rs`, one at a time, each confirmed in a page dialog, read back and logged to `changes.jsonl` with the Access user. They cover subscriptions (`/api/subscription`), renames (`/api/rename`) and settings (`/api/setting`: latency, sample rate, encoding). Tests write only to dstack's own device in E2E.

Dante protocol APIs requested in priority order: zbynekdrlik/dstack#16.

## Machines

Claude works in the `dcontroller` account on dev1. dev1 has no access to the Dante network, so the server runs on a box on that network, chosen by the owner 31.10.2026. Which box, how to reach it and its interface are in the private `~/.config/dcontroller/SITE.md`.

Deploy: `deploy/package.sh` builds `target/bundle/dcontroller-<ver>.tar.gz` (CI job `package` builds the same bundle, installs it as a systemd service on the runner and uploads it as an artifact). On the box: unpack, `sudo DC_BIND=127.0.0.1:<port> DC_IFACE=<dante nic> ./install.sh` the first time, `sudo ./install.sh` after. The service runs as the `dcontroller` system user on loopback only. Public access: Cloudflare Tunnel + Access (set up by airuleset, airuleset#1278); the box runs the connector as its own unit `deploy/cloudflared-dcontroller.service` (never `cloudflared service install`, which would replace another `cloudflared.service`), token in `/etc/cloudflared-dcontroller/token` (root 0600, not in git; the dev-side copy is a key file in the account's secrets dir, used only via `airuleset.py secret exec`).

Dante NIC: the box has a Dante leg (a VLAN on its one NIC, dstack `docs/two-networks.md`, the standard for every PC with a Dante leg). Only Dante traffic may use it (owner 1.11.2026). After install, `sudo DC_IFACE=<dante interface> ./dante-guard.sh` from the bundle: the firewall is dstack's generic Dante-leg filter (`deploy/dante-leg.nft`, unchanged from dstack `tools/linux/dante-leg.nft` at the pinned tag, which CI checks; dstack #264), installed at `/etc/dstack/dante-leg.nft` and loaded by `dante-guard.service` as table `inet dstack_dante_leg` with `nft -D dante_leg=<iface>`. It names no device and no user: in on the leg only Dante's ports, the dstack flow range 29800-31847 (every dstack device's fixed slot ports) and Dante's groups, mDNS answers from 5353, ssh and the MCP; everything else dropped. The script also sets avahi's `deny-interfaces`, ARP for the leg's own address only, and in NetworkManager no default route, DNS or IPv6 on it, with its subnet in routing table 100 (metric 900) used only by sockets bound to the leg's address (rule `from <address>`). The Dante side of dcontroller must therefore bind to the leg's address. A box whose network is netplan/systemd-networkd instead of NetworkManager (the second site's PC): the leg is a netplan file (`/etc/netplan/60-dante-vlan.yaml`: the VLAN with its address as a /24, no gateway, link-local off, the subnet route also in table 100 with rule `from <address> table 100`). The kernel's prefix route to the Dante subnet stays in the main table, which dstack accepts for networkd boxes (dstack#407): a /32 would show Dante Controller a subnet mismatch through dstack's network info. The guard runs with `DC_GUARD_SKIP_ROUTING=1` (its routing step is NetworkManager-only). Should a leg ever hold a /32, dcontroller still lists its devices (#115). Moving the box from the per-box table `inet dante_guard` (0.1.60 and older): install.sh removes `guard_users` from the config and the script deletes the old table once the generic one is loaded (#84). A dstack bump that changes `dante-leg.nft` needs the file copied again (CI fails until it is).

## What else acts on the machines

Dante Controller on Windows PCs and every Dante consumer (iemmixer, dasio, SongPlayer) act on the same devices. Do not change what they own. dantesync is the clock and belongs to dstack.

## How to run a test

- Rust: `cargo test --workspace`; lint `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and the same clippy with `-p dcontroller-web --target wasm32-unknown-unknown`.
- E2E: `cd crates/web && trunk build`, `cargo build -p dcontroller -p dstack --bin dcontroller --bin dstack-dev --example arc_rename`, `cd e2e && npm ci && ./isolated.sh npx playwright test`. `isolated.sh` (#95) runs it in a network namespace of its own with a dummy `e2e0` (10.199.0.1/24, the default route), so no test packet reaches a real network; `playwright.config.ts` refuses any other run. It needs unprivileged user namespaces, which AppArmor restricts on Ubuntu 24.04 (CI turns that off with sudo). It starts dstack's own Dante device (`dstack-dev --clock system`, `e2e/fixtures/`) on the default-route address and the real server on 127.0.0.1:18080 with that interface as `dante_interface`; the test reads the device through the page. trunk is system-wide on dev1 (airuleset#1275); CI installs its own. dev1's shared Playwright browsers lag 1.64.1 (airuleset#1277).
- Server config: `~/.config/dcontroller/config.toml` with `web_dir`, optional `bind` and `dante_interface`.

Clocks (#124): the Clock page shows Dante PTP (each device's clock status), the devices' own dsync figures (`clocks.rs`: dstack's clock figures request on the Dante leg, answered by dstack devices from v0.6.187) and the PCs' dsync clock services (`dsync.rs`: `GET /status` on port 8898 over the LAN). The service nodes are site facts: they are named in the box's config as `dsync_nodes = ["<host>", "<host>:<port>"]` in `/etc/dcontroller/config.toml` (none named: none asked, the table does not show), then `systemctl restart dcontroller`. A trailing `?` (`"<host>?"`) marks a travelling node, a laptop that is off or elsewhere most days: it is no alarm while it does not answer. A node's name is looked up once and kept (lookups on every ask timed out now and then on the LAN's DNS); a failed ask looks it up again, and a node turns "no answer" only after three failed asks in a row. Only the health fields dsync names are read; `offset_ns` and `settled` never.

Dante tests: the E2E uses dstack's device binary as a real Dante device. A read against the site's devices runs on the box after each deploy (the device list must show them). A test that changes a device needs the owner's approval, unless the device is ours. A browser check after a deploy opens a fresh page each time and closes it after: a tab left open keeps the old version's code, and its polls fail while nothing serves it.

## Site facts

Not in this public repo. dstack holds them in its account at `~/.config/dstack/SITE.md`. Ask dstack over the peer channel, then keep what this project needs in `~/.config/dcontroller/`.

## Runners

None yet. GitHub-hosted runners cover build and tests. A test on the Dante network needs a runner or a box the owner approves.

## Backlog

- Foundation as in Status.
- Open items are on #1's checklist (meters: dstack#16 step 6).

dstack provides these or builds them, tracked on zbynekdrlik/dstack#1: mDNS discovery, ARC reads, subscription requests, renames (ARC 0x2013/0x3001/0x1001), device latency (0x1101), sample rate and encoding (UDP 8700, messages 0x0081/0x0083), per-flow telemetry.

## Knowledge left in other projects

dstack: protocol notes and captures. dasio and iemmixer: real consumers of the stack. The peer channel reaches dstack.
