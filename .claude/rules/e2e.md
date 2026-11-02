---
paths:
  - "e2e/**"
---

# E2E gotchas

- Playwright `hasText` matches a substring, case aside: `th` "Software" also hits "Dante software". Match a table row by its exact header: `getByRole("rowheader", { name, exact: true })`.
- After a commit, rebuild both the web (`trunk build`) and the server before a local run: the dashboard test compares their versions, and a stale build of either fails it.
- dstack's own device (`e2e/fixtures/device.toml`) answers settings, product and board info like hardware. Assert what it is configured with; take versions from `/api/devices` and check that the page shows the same.
- The start page is Overview: a spec opens its page by hash (`/#routing`, `/#devices`), never relying on `/`.
- uPlot draws axis labels on the canvas: assert chart text through `.u-legend`, not `.u-axis`.
- The recorder needs a few seconds of heartbeats before the latency page or the overview has rows: wait on the row (timeout 30–40 s), not on a sleep.
