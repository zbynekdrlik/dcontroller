# dcontroller handover

Request: zbynekdrlik/airuleset#1273, owner 30.10.2026 and 31.10.2026.

## Status

Cargo workspace: `crates/server` (axum: PWA, `/api/version`, `/api/devices`, `/healthz`), `crates/web` (Leptos CSR, built by trunk, PWA manifest + service worker), `crates/api` (the JSON types both share), `crates/buildver` (one version string from git for both). Live behind Cloudflare Access (#1).

Reads: `crates/server/src/dante.rs` runs dstack discovery on the IPv4 of `dante_interface` and reads each device when listed, when its records change and every 30 s. Every read covers the channels, subscriptions and settings (ARC), plus the clock status and network info (UDP 8700). The product info, board info, encoding and sample rates are read once per listing. The page polls `/api/devices` every 3 s. Pages: Overview (start), Routing, Devices, Latency, Meters, Clock, Network, Changes; the web is a sidebar shell with one stylesheet (`crates/web/public/style.css`, tokens on `:root`).

Latency history: `crates/server/src/latency.rs` records every receiver all the time from one heartbeat listener (dstack `latency::Listener`) into SQLite (`history_db`, default `/var/lib/dcontroller/history.db`; `crates/server/src/history.rs`): seconds for 24 h, minutes for 30 days, hours for a year, rolled up every minute; schema steps in `PRAGMA user_version`, never edited once shipped. `/api/history/{device}?from&to` buckets any range to ≤ 1500 points; `/api/overview` sums the last hour and day per receiver. The Latency page reads device to device (`hist::by_source`: one series per source device, the worst peak against each flow's own latency, late packets added). Late packets, the receiver's own count of packets past their flow's latency, are the verdict: health is bad only with late packets, a peak is only the margin. A flow's latency from ARC 0x3200 is already the larger of receiver and transmitter latency. Charts are uPlot 1.6.30 (MIT, vendored in `crates/web/public/vendor/`) driven by `crates/web/public/charts.js`.

Writes: only in `crates/server/src/change.rs`, one at a time, each confirmed in a page dialog, read back and logged to `changes.jsonl` with the Access user. They cover subscriptions (`/api/subscription`), renames (`/api/rename`) and settings (`/api/setting`: latency, sample rate, encoding). Tests write only to dstack's own device in E2E.

Dante protocol APIs requested in priority order: zbynekdrlik/dstack#16.

## Machines

Claude works in the `dcontroller` account on dev1. dev1 has no access to the Dante network, so the server runs on a box on that network, chosen by the owner 31.10.2026. Which box, how to reach it and its interface are in the private `~/.config/dcontroller/SITE.md`.

Deploy: `deploy/package.sh` builds `target/bundle/dcontroller-<ver>.tar.gz` (CI job `package` builds the same bundle, installs it as a systemd service on the runner and uploads it as an artifact). On the box: unpack, `sudo DC_BIND=127.0.0.1:<port> DC_IFACE=<dante nic> ./install.sh` the first time, `sudo ./install.sh` after. The service runs as the `dcontroller` system user on loopback only. Public access: Cloudflare Tunnel + Access (set up by airuleset, airuleset#1278); the box runs the connector as its own unit `deploy/cloudflared-dcontroller.service` (never `cloudflared service install`, which would replace another `cloudflared.service`), token in `/etc/cloudflared-dcontroller/token` (root 0600, not in git; the dev-side copy is a key file in the account's secrets dir, used only via `airuleset.py secret exec`).

Dante NIC: the box has a Dante leg (a VLAN on its one NIC, dstack `docs/two-networks.md`, the standard for every PC with a Dante leg). Only Dante traffic may use it (owner 1.11.2026). After install, `sudo DC_IFACE=<dante interface> ./dante-guard.sh` from the bundle: firewall unit `dante-guard.service` (out only to the Dante subnet and Dante's multicast groups, and only from the `dcontroller` user, IGMP, PTP and, on a DHCP leg, DHCP; inbound only replies, DHCP on a DHCP leg, PTP, mDNS including unicast answers from source port 5353, UDP multicast, meter ports), ARP for its own address only, and in NetworkManager no default route, DNS or IPv6 on it, with its subnet in routing table 100 (metric 900) used only by sockets bound to the leg's address (rule `from <address>`). The Dante side of dcontroller must therefore bind to the leg's address. Another Dante device on the same PC (a dlink app) needs its user and ports added to the guard. Blocked sends are logged as `dante-guard out drop:` in the kernel log.

## What else acts on the machines

Dante Controller on Windows PCs and every Dante consumer (iemmixer, dasio, SongPlayer) act on the same devices. Do not change what they own. dantesync is the clock and belongs to dstack.

## How to run a test

- Rust: `cargo test --workspace`; lint `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and the same clippy with `-p dcontroller-web --target wasm32-unknown-unknown`.
- E2E: `cd crates/web && trunk build`, `cargo build -p dcontroller -p dstack --bin dcontroller --bin dstack-dev`, `cd e2e && npm ci && npx playwright test`. It starts dstack's own Dante device (`dstack-dev --clock system`, `e2e/fixtures/`) on the default-route address and the real server on 127.0.0.1:18080 with that interface as `dante_interface`; the test reads the device through the page. trunk is system-wide on dev1 (airuleset#1275); CI installs its own. dev1's shared Playwright browsers lag 1.64.1 (airuleset#1277).
- Server config: `~/.config/dcontroller/config.toml` with `web_dir`, optional `bind` and `dante_interface`.

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
