import { test, expect, type ConsoleMessage } from "@playwright/test";
import { DSYNC } from "../env";

// The device global-setup.ts runs: dstack's own Dante device binary. It has
// no PTP clock of its own and answers no clock status, so its row says so.
const DEVICE = "dc-e2e-device";

test("the clock tab lists every device's clock status as the server read it", async ({ page }) => {
  const msgs: string[] = [];
  page.on("console", (m: ConsoleMessage) => {
    if (m.type() === "error" || m.type() === "warning") msgs.push(`${m.type()}: ${m.text()}`);
  });
  page.on("pageerror", (e) => msgs.push(`pageerror: ${e.message}`));

  await page.goto("/");
  await page.getByTestId("tab-clock").click();
  await expect(page).toHaveURL(/#clock$/);
  const row = page.locator(`[data-testid="clock-row"][data-name="${DEVICE}"]`);
  // Discovery, the first read and a clock ask that waits out its 1 s.
  await expect(row.getByTestId("clock-none")).toHaveText(/^no clock status: .*no reply/, { timeout: 30_000 });
  await expect(page.getByTestId("clock-summary")).not.toBeEmpty();

  // Each row shows what /api/devices says, whatever else is on the network.
  const api = await page.evaluate(() => fetch("/api/devices").then((r) => r.json()));
  const dev = api.devices.find((d: { name: string }) => d.name === DEVICE);
  expect(dev.clock).toBeNull();
  expect(dev.clock_error).toMatch(/no reply/);
  await expect(page.getByTestId("clock-row")).toHaveCount(api.devices.length);
  for (const d of api.devices.filter((d: { clock: unknown }) => d.clock)) {
    const r = page.locator(`[data-testid="clock-row"][data-name="${d.name}"]`);
    const role = d.clock.leader ? (d.clock.grandmaster ? "Leader (grandmaster)" : "Leader") : "Follower";
    await expect(r.getByTestId("clock-role")).toHaveText(role);
    await expect(r.getByTestId("clock-sync")).toHaveText(d.clock.sync);
  }

  // dsync clocks (#124): each dstack device says what its own clock runs
  // on; the server asks with dstack's clock figures request. The page
  // shows what /api/clocks holds, every listed device either in the table
  // or named as giving no figures.
  await expect
    .poll(async () => (await page.request.get("/api/clocks").then((r) => r.json())).nodes.length, { timeout: 40_000 })
    .toBeGreaterThan(0);
  const clocks = await page.request.get("/api/clocks").then((r) => r.json());
  const node = clocks.nodes.find((n: { device: string }) => n.device === DEVICE);
  expect(node).toBeTruthy();
  expect(["none", "follower", "service", "system"]).toContain(node.source);
  expect(node.age_s).toBeLessThan(60);
  const drow = page.locator(`[data-testid="dsync-row"][data-name="${DEVICE}"]`);
  await expect(drow).toBeVisible({ timeout: 20_000 });
  await expect(drow).toHaveAttribute("data-state", /^(ok|warn|bad)$/);
  await expect(drow.getByTestId("dsync-state")).not.toBeEmpty();
  await expect(page.getByTestId("dsync-summary")).toContainText("dsync:");
  const answered = clocks.nodes.map((n: { device: string }) => n.device.toLowerCase());
  const silent = api.devices.filter((d: { name: string }) => !answered.includes(d.name.toLowerCase()));
  if (silent.length > 0) {
    // The devices that say nothing are a count; their names one click away.
    const quiet = page.getByTestId("dsync-silent");
    await expect(quiet.locator("summary")).toHaveText(
      new RegExp(`^${silent.length} devices? gives? no dsync figures \\(not a dstack device, or a dstack before v0\\.6\\.187\\)$`),
    );
    await expect(quiet.locator("p")).toBeHidden();
    await quiet.locator("summary").click();
    await expect(quiet.locator("p")).toBeVisible();
    await expect(quiet.locator("p")).toContainText(silent[0].name);
  }

  // dsync service nodes (#124): the PCs' clock services the box's config
  // names, asked `/status` over the LAN. One answers (a stand-in for
  // dsync, global-setup.ts), one has nothing behind it.
  await expect
    .poll(
      async () =>
        (await page.request.get("/api/clocks").then((r) => r.json())).services.find(
          (s: { node: string }) => s.node === DSYNC.up,
        )?.mode,
      { timeout: 30_000 },
    )
    .toBe("LOCK");
  await expect
    .poll(
      async () =>
        (await page.request.get("/api/clocks").then((r) => r.json())).services.filter(
          (s: { age_s: number | null }) => s.age_s !== null,
        ).length,
      { timeout: 30_000 },
    )
    .toBe(3);
  const svc = (await page.request.get("/api/clocks").then((r) => r.json())).services;
  expect(svc.map((s: { node: string }) => s.node)).toEqual([DSYNC.up, DSYNC.down, DSYNC.old, DSYNC.newer]);
  expect(svc[0]).toMatchObject({
    error: null,
    version: "0.9.4",
    ptp_phase_locked: true,
    ptp_phase_error_us: 3.25,
    gm_uuid: "00:1d:c1:aa:bb:cc",
    gm_source_ip: "10.199.0.200",
    alarm: null,
  });
  // What dsync says is no health is not passed on.
  expect(svc[0]).not.toHaveProperty("offset_ns");
  expect(svc[0]).not.toHaveProperty("settled");
  expect(svc[1].error).toBeTruthy();
  const up = page.locator(`[data-testid="dsync-service"][data-name="${DSYNC.up}"]`);
  const down = page.locator(`[data-testid="dsync-service"][data-name="${DSYNC.down}"]`);
  await expect(up).toHaveAttribute("data-state", "ok", { timeout: 20_000 });
  await expect(up.getByTestId("dsync-service-state")).toHaveText("locked · phase error 3.2 µs");
  await expect(up).toContainText("LOCK");
  await expect(up).toContainText("00:1d:c1:aa:bb:cc · 10.199.0.200");
  await expect(down).toHaveAttribute("data-state", "bad");
  await expect(down.getByTestId("dsync-service-state")).toContainText("no answer");
  // The one in trouble reads first.
  await expect(page.getByTestId("dsync-service").first()).toHaveAttribute("data-name", DSYNC.down);
  // The status contract (#124): an older dsync names none and is read as
  // before, named old; a newer one is not misread, it says to update.
  const old = page.locator(`[data-testid="dsync-service"][data-name="${DSYNC.old}"]`);
  const newer = page.locator(`[data-testid="dsync-service"][data-name="${DSYNC.newer}"]`);
  expect(svc[2]).toMatchObject({ schema: null, mode: "LOCK", ptp_phase_locked: true });
  expect(svc[3]).toMatchObject({ schema: 2, version: "0.9.4", mode: null, ptp_phase_locked: null });
  await expect(old).toHaveAttribute("data-state", "ok");
  await expect(old.getByTestId("dsync-service-version")).toHaveText("0.9.4 · old dsync");
  await expect(up.getByTestId("dsync-service-version")).toHaveText("0.9.4");
  // A node that never answered says nothing of its dsync, old or not.
  await expect(down.getByTestId("dsync-service-version")).toHaveText("–");
  await expect(newer).toHaveAttribute("data-state", "bad");
  await expect(newer.getByTestId("dsync-service-state")).toHaveText(
    "its status is schema 2, newer than this dcontroller reads (1): update dcontroller",
  );
  await expect(page.getByTestId("dsync-service-summary")).toHaveText(
    `2 of 4 service nodes locked · in trouble: ${DSYNC.down}, ${DSYNC.newer}`,
  );

  // #124 2c: a node's row opens its chart under the table, from the states
  // the server stores every 10 s; the node with nothing behind it had no
  // lock the whole time.
  await expect
    .poll(
      async () =>
        (await page.request.get(`/api/dsync/${encodeURIComponent(DSYNC.up)}`).then((r) => r.json())).points.at(-1)
          ?.answered,
      { timeout: 30_000 },
    )
    .toBe(true);
  const hist = await page.request.get(`/api/dsync/${encodeURIComponent(DSYNC.up)}`).then((r) => r.json());
  expect(hist.points.at(-1)).toMatchObject({ answered: true, locked: true, phase_error_us: 3.25, mode: "LOCK" });
  await up.getByTestId("dsync-service-open").click();
  await expect(page).toHaveURL(new RegExp(`#clock/dsync/${encodeURIComponent(DSYNC.up)}$`));
  await expect(page.getByTestId("dsync-chart-title")).toHaveText("127.0.0.1:18898 · phase error".replace("127.0.0.1:18898", await up.locator("td").first().innerText()));
  await expect(page.getByTestId("dsync-chart").locator("canvas")).toHaveCount(1, { timeout: 20_000 });
  await expect(page.getByTestId("dsync-chart").locator(".u-legend")).toContainText("dsync alarms at 50 µs");
  // The suite's server may store a state before the stand-in listens: the
  // words count what is stored, the chart has its red bars only there.
  await expect(page.getByTestId("dsync-chart-words")).toHaveText(
    /^(Locked the whole range \(\d+ states|No lock in \d+ of \d+ states)/,
  );
  await expect(up).toHaveClass(/\bsel\b/);
  await page.locator('[data-testid="dsync-range"][data-range="86400"]').click();
  await expect(page.locator('[data-testid="dsync-range"][data-range="86400"]')).toHaveClass(/\bon\b/);
  await expect(page.getByTestId("dsync-chart").locator("canvas")).toBeVisible();
  await down.getByTestId("dsync-service-open").click();
  await expect(page.getByTestId("dsync-chart-words")).toHaveText(/^No lock in (\d+) of \1 states/, { timeout: 20_000 });
  await expect(page.getByTestId("dsync-chart")).toHaveAttribute("data-late-marks", /^[1-9]\d*$/);
  await page.getByTestId("dsync-chart-close").click();
  await expect(page.getByTestId("dsync-chart-panel")).toBeHidden();

  // What the state chip says (the phase error) is not a column again, and
  // the PTP figures are one.
  await expect(up.getByTestId("dsync-service-ptp")).toHaveText(/^\d+\/s · last \d+\.\d s ago$/);
  await expect(page.getByTestId("dsync-services").locator("thead th")).toHaveCount(8);

  // The page's parts (#123): a chip per part on top; the part in trouble
  // (the service node with nothing behind it) comes first, above the PTP
  // table that is fine.
  const chips = page.getByTestId("part-chip");
  await expect(chips).toHaveCount(3);
  await expect(page.locator('[data-testid="part-chip"][data-part="dsync-services"]')).toHaveAttribute("data-state", "bad");
  await expect(page.locator('[data-testid="part-chip"][data-part="dsync-services"]')).toHaveText(
    "dsync service nodesin trouble",
  );
  const parts = page.getByTestId("part");
  const services = page.locator('[data-testid="part"][data-part="dsync-services"]');
  await expect(services.locator("h2")).toHaveText("dsync service nodes");
  // It is the page's third part by place; in trouble, it stands above the
  // devices' dsync part, which is not.
  await expect(page.locator('[data-testid="part"][data-part="dsync"]')).not.toHaveAttribute("data-state", "bad");
  const placed = await parts.evaluateAll((els) => els.map((e) => e.getAttribute("data-part")));
  expect(placed.indexOf("dsync-services")).toBeLessThan(placed.indexOf("dsync"));
  expect(placed.slice().sort()).toEqual(["dsync", "dsync-services", "ptp"]);
  const ranks = await parts.evaluateAll((els) =>
    els.map((e) => ({ bad: 0, warn: 1 })[e.getAttribute("data-state") ?? ""] ?? 2),
  );
  expect(ranks).toEqual([...ranks].sort((a, b) => a - b));
  // A chip leads to its part.
  await page.setViewportSize({ width: 1280, height: 420 });
  const ptp = page.locator('[data-testid="part"][data-part="ptp"]');
  await page.locator('[data-testid="part-chip"][data-part="ptp"]').click();
  await expect(ptp.locator("h2")).toBeInViewport();
  await page.locator('[data-testid="part-chip"][data-part="dsync"]').click();
  await expect(page.locator('[data-testid="part"][data-part="dsync"] h2')).toBeInViewport();

  // The view stays in the URL: a reload opens it again.
  await page.reload();
  await expect(page.getByTestId("clock-table")).toBeVisible();
  expect(msgs).toEqual([]);
});
