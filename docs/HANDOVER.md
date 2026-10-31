# dcontroller handover

Request: zbynekdrlik/airuleset#1273, owner 30.10.2026 and 31.10.2026.

## Status

Foundation in progress (#1): Cargo workspace with `crates/server` (axum: PWA, `/api/version`, `/healthz`), `crates/web` (Leptos CSR, built by trunk, PWA manifest + service worker), `crates/buildver` (one version string from git for both). Cloudflare tunnel with Access waits for the box on the Dante network.

Dante protocol APIs requested in priority order: zbynekdrlik/dstack#16.

## Machines

Claude works in the `dcontroller` account on dev1. dev1 has no access to the Dante network, so the server runs on a box on that network, chosen by the owner 31.10.2026. Which box, how to reach it and its interface are in the private `~/.config/dcontroller/SITE.md`.

Deploy: `deploy/package.sh` builds `target/bundle/dcontroller-<ver>.tar.gz` (CI job `package` builds the same bundle, installs it as a systemd service on the runner and uploads it as an artifact). On the box: unpack, `sudo DC_BIND=127.0.0.1:<port> DC_IFACE=<dante nic> ./install.sh` the first time, `sudo ./install.sh` after. The service runs as the `dcontroller` system user on loopback only. Public access: Cloudflare Tunnel + Access (set up by airuleset, airuleset#1278); the box runs the connector as its own unit `deploy/cloudflared-dcontroller.service` (never `cloudflared service install`, which would replace another `cloudflared.service`), token in `/etc/cloudflared-dcontroller/token` (root 0600, not in git; the dev-side copy is a key file in the account's secrets dir, used only via `airuleset.py secret exec`).

## What else acts on the machines

Dante Controller on Windows PCs and every Dante consumer (iemmixer, dasio, SongPlayer) act on the same devices. Do not change what they own. dantesync is the clock and belongs to dstack.

## How to run a test

- Rust: `cargo test --workspace`; lint `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and the same clippy with `-p dcontroller-web --target wasm32-unknown-unknown`.
- E2E: `cd crates/web && trunk build`, `cargo build -p dcontroller`, `cd e2e && npm ci && npx playwright test` (starts the real server on 127.0.0.1:18080). trunk is system-wide on dev1 (airuleset#1275); CI installs its own. dev1's shared Playwright browsers lag 1.64.1 (airuleset#1277).
- Server config: `~/.config/dcontroller/config.toml` with `web_dir`, optional `bind` and `dante_interface`.

Dante tests: not defined yet. A read-only test against real devices comes first: mDNS discovery and ARC reads through dstack. A test that changes a device needs the owner's approval, unless the device is ours.

## Site facts

Not in this public repo. dstack holds them in its account at `~/.config/dstack/SITE.md`. Ask dstack over the peer channel, then keep what this project needs in `~/.config/dcontroller/`.

## Runners

None yet. GitHub-hosted runners cover build and tests. A test on the Dante network needs a runner or a box the owner approves.

## Backlog

- Foundation as in Status.
- Device list, channels and subscriptions.
- Renames, latency, sample rate and encoding, per-flow telemetry.

dstack provides these or builds them, tracked on zbynekdrlik/dstack#1: mDNS discovery, ARC reads, subscription requests, renames (ARC 0x2013/0x3001/0x1001), device latency (0x1101), sample rate and encoding (UDP 8700, messages 0x0081/0x0083), per-flow telemetry.

## Knowledge left in other projects

dstack: protocol notes and captures. dasio and iemmixer: real consumers of the stack. The peer channel reaches dstack.
