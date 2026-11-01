---
paths:
  - "e2e/**"
---

# E2E gotchas

- Playwright `hasText` matches a substring, case aside: `th` "Software" also hits "Dante software". Match a table row by its exact header: `getByRole("rowheader", { name, exact: true })`.
- After a commit, rebuild both the web (`trunk build`) and the server before a local run: the dashboard test compares their versions, and a stale build of either fails it.
- dstack's own device (`e2e/fixtures/device.toml`) answers settings, product and board info like hardware. Assert what it is configured with; take versions from `/api/devices` and check that the page shows the same.
