import { test, expect, type ConsoleMessage } from "@playwright/test";

// The overview is the start page: health tiles from the device list and the
// receivers' recorded latency.
const DEVICE = "dc-e2e-device";

test("the overview is the start page with health tiles and the receivers' last hour", async ({ page }) => {
  const msgs: string[] = [];
  page.on("console", (m: ConsoleMessage) => {
    if (m.type() === "error" || m.type() === "warning") msgs.push(`${m.type()}: ${m.text()}`);
  });
  page.on("pageerror", (e) => msgs.push(`pageerror: ${e.message}`));

  await page.goto("/");
  await expect(page.getByTestId("tab-overview")).toHaveClass(/\bon\b/);
  await expect(page.locator(".page-head h1")).toHaveText("Overview");

  const devices = await page.evaluate(() => fetch("/api/devices").then((r) => r.json()));
  await expect(page.getByTestId("tile-devices").locator(".tile-value")).toHaveText(String(devices.devices.length), {
    timeout: 30_000,
  });
  await expect(page.getByTestId("tile-subs")).toContainText("Subscriptions");
  await expect(page.getByTestId("tile-clock")).toContainText("Clock");
  await expect(page.getByTestId("tile-network")).toContainText("links");

  // The device's own flow is recorded: it appears with a sparkline and its latency.
  const row = page.locator(`[data-testid="overview-receiver"][data-name="${DEVICE}"]`);
  await expect(row).toBeVisible({ timeout: 40_000 });
  await expect(row).toContainText("10.00 ms");
  await expect(row.locator("svg.spark path.line")).toHaveAttribute("d", /^M/);
  const o = await page.evaluate(() => fetch("/api/overview").then((r) => r.json()));
  const r = o.receivers.find((x: { device: string }) => x.device === DEVICE);
  expect(r.latency_us).toBe(10000);
  expect(r.flows).toBe(1);
  await expect(page.getByTestId("tile-latency").locator(".tile-value")).toHaveText(
    String(o.receivers.reduce((a: number, x: { late_1h: number }) => a + x.late_1h, 0)),
  );

  // A receiver's name opens its latency history.
  await row.locator("a").click();
  await expect(page).toHaveURL(new RegExp(`#latency/${DEVICE}$`));
  await expect(page.getByTestId("tab-latency")).toHaveClass(/\bon\b/);
  await expect(page.getByTestId("lat-flow").first()).toBeVisible({ timeout: 20_000 });
  expect(msgs).toEqual([]);
});
