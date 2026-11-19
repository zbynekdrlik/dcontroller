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
  if (silent.length > 0) await expect(page.getByTestId("dsync-silent")).toContainText(silent[0].name);

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
  const svc = (await page.request.get("/api/clocks").then((r) => r.json())).services;
  expect(svc.map((s: { node: string }) => s.node)).toEqual([DSYNC.up, DSYNC.down]);
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
  await expect(page.getByTestId("dsync-service-summary")).toHaveText(
    `1 of 2 service nodes locked · in trouble: ${DSYNC.down}`,
  );

  // The view stays in the URL: a reload opens it again.
  await page.reload();
  await expect(page.getByTestId("clock-table")).toBeVisible();
  expect(msgs).toEqual([]);
});
