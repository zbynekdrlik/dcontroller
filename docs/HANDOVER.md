# dcontroller handover

Request: zbynekdrlik/airuleset#1273, owner 30.10.2026 and 31.10.2026.

## Status

New project, no code yet. The first job is the usual Rust web app foundation: version on the dashboard, Playwright E2E, PWA, Cloudflare tunnel with Access and e-mail authorization.

## Machines

Claude works in the `dcontroller` account on dev1. dev1 has no access to the Dante network, so the server must run on a box on that network. The owner has not chosen the box yet.

## What else acts on the machines

Dante Controller on Windows PCs and every Dante consumer (iemmixer, dante-asio, SongPlayer) act on the same devices. Do not change what they own. dantesync is the clock and belongs to dante-stack.

## How to run a test

Not defined yet. A read-only test against real devices comes first: mDNS discovery and ARC reads through dante-stack. A test that changes a device needs the owner's approval, unless the device is ours.

## Site facts

Not in this public repo. dante-stack holds them in its account at `~/.config/dante-stack/SITE.md`. Ask dante-stack over the peer channel, then keep what this project needs in `~/.config/dcontroller/`.

## Runners

None yet. GitHub-hosted runners cover build and tests. A test on the Dante network needs a runner or a box the owner approves.

## Backlog

- Foundation as in Status.
- Device list, channels and subscriptions.
- Renames, latency, sample rate and encoding, per-flow telemetry.

dante-stack provides these or builds them, tracked on zbynekdrlik/dante-stack#1: mDNS discovery, ARC reads, subscription requests, renames (ARC 0x2013/0x3001/0x1001), device latency (0x1101), sample rate and encoding (UDP 8700, messages 0x0081/0x0083), per-flow telemetry.

## Knowledge left in other projects

dante-stack: protocol notes and captures. dante-asio and iemmixer: real consumers of the stack. The peer channel reaches dante-stack.
