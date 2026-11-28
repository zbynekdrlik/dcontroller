---
paths:
  - "e2e/**"
---

# E2E gotchas

- Playwright `hasText` matches a substring, case aside: `th` "Software" also hits "Dante software". Match a table row by its exact header: `getByRole("rowheader", { name, exact: true })`.
- After a commit, rebuild both the web (`trunk build`) and the server before a local run: the dashboard test compares their versions, and a stale build of either fails it.
- dstack's own device (`e2e/fixtures/device.toml`) answers settings, product and board info like hardware. Assert what it is configured with; take versions from `/api/devices` and check that the page shows the same.
- The start page is the Map (#122): a spec opens its page by hash (`/#routing`, `/#map/devices`, `/#map/device/<name>`), never relying on `/`. Overview and Devices are gone (#130): a device's card is `map-device-body` on `#map/device/<name>`, always open.
- uPlot draws axis labels on the canvas: assert chart text through `.u-legend`, not `.u-axis`.
- The recorder needs a few seconds of heartbeats before the latency page or the overview has rows: wait on the row (timeout 30–40 s), not on a sleep.
- Run the suite only through `e2e/isolated.sh npx playwright test …` (#95): a network namespace with a dummy `e2e0`. `playwright.config.ts` refuses any other run, because the suite's packets (a test device, fake dstack peers, mDNS floods, 96 s browsers) would reach the real Dante network the host sits on. It needs unprivileged user namespaces; CI turns off AppArmor's restriction with sudo.
- A size check (a phone layout's height) waits for every part it measures, e.g. the mDNS tile that comes on its own poll; CI's font is wider than a dev box's, so text wraps more there.
- A Map line or Links row comes from the subscriptions at once; its figures (`/api/links`) come once the recorder has seconds of the hour, and the page reads them on its own 10 s poll: poll the API for the figure, then give the page's cell a 30 s timeout.
- The suite's server can start before global-setup's stand-ins (dsync nodes, the video node list): a reader's first read fails and is stored as such. Poll the API until the stand-in's answer is there; never assert on the first stored point.
- The menu is behind a burger (#122): a spec changes page with `nav(page, "clock")` from `e2e/env.ts`, never a bare click on `tab-…` (it is off the screen until the burger opens it).
- On the Map `.hit` is also every line's wide click path (`path.hit`): a device looked for is `.node.hit`.
- The Map's SVG is replaced whenever its data changes: a `boundingBox()` of a box can come back `null` for a moment; retry it. Video nodes appear only after the server's first good read of the stand-in list (up to a minute after start), and only on the Map's Video view (#124, the default is Audio): a spec that needs one more audio box (a second place, "no place set") subscribes the test device to a source that is not on the network and unsubscribes in `finally`.
- A device a spec switches on and off (#122, `map.spec.ts` "switched off") is `dstack-dev` on the namespace's third address 10.199.0.3 (`isolated.sh`) with its own `--control` socket (without it dstack-dev exits, looking for /run/dstack). The spec forgets it at the end (`POST /api/forget`), so later specs see no extra box.
- The stand-in cambox reports its own `out` connections (camera-box 1.8.19 shape), the same flows as the OBS stand-in's `in`: a spec picks a video connection by `id` or `direction === "in"`, never by `source` alone or by `connections[0]`. Only `in` connections are Map lines.
