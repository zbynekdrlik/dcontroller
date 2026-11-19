import { test, expect, type ConsoleMessage } from "@playwright/test";

// The Map (#122): the schema of what is connected to what, and the chart
// of the connection the user clicks. The test device receives its own TX
// (one connection, a loop on its box); the bridge test device has its two
// delays as two connections to its ASIO side.
const DEVICE = "dc-e2e-device";
const BRIDGE = "dc-e2e-bridge";

test("the map is the start page: every device a box, every connection a line, a click shows that connection's chart", async ({ page }) => {
  const msgs: string[] = [];
  page.on("console", (m: ConsoleMessage) => {
    if (m.type() === "error" || m.type() === "warning") msgs.push(`${m.type()}: ${m.text()}`);
  });
  page.on("pageerror", (e) => msgs.push(`pageerror: ${e.message}`));

  await page.goto("/");
  await expect(page.getByTestId("tab-map")).toHaveClass(/\bon\b/);
  await expect(page.locator(".page-head h1")).toHaveText("Map");

  // The server sums the last hour per connection and names the bridge.
  await expect
    .poll(async () => (await page.request.get("/api/links").then((r) => r.json())).links.length, { timeout: 40_000 })
    .toBeGreaterThan(0);
  await expect
    .poll(async () => (await page.request.get("/api/links").then((r) => r.json())).bridges.length, { timeout: 40_000 })
    .toBe(1);
  const links = await page.request.get("/api/links").then((r) => r.json());
  const own = links.links.find((l: any) => l.to === DEVICE);
  expect(own.from).toBe(DEVICE);
  expect(own.latency_us).toBe(10_000);
  expect(own.spark).toHaveLength(60);
  expect(links.bridges[0].device).toBe(BRIDGE);

  // Every listed device is a box; the bridge's ASIO side is one more.
  const map = page.getByTestId("map");
  const devices = await page.request.get("/api/devices").then((r) => r.json());
  for (const d of devices.devices) {
    await expect(map.locator(`[data-testid="map-node"][data-node="${d.name}"]`)).toHaveCount(1, { timeout: 30_000 });
  }
  await expect(map.locator('[data-testid="map-node"].asio')).toHaveCount(1, { timeout: 30_000 });
  await expect(map.locator('[data-testid="map-node"].asio')).toHaveAttribute("data-node", "SoundGrid");

  // The device's own connection: measured, its latency on the line.
  const line = map.locator(`[data-testid="map-edge"][data-from="${DEVICE}"][data-to="${DEVICE}"]`);
  await expect(line).toHaveCount(1);
  await expect(line).not.toHaveAttribute("data-state", "unknown", { timeout: 40_000 });
  await expect(line.locator(".lbl text")).toContainText("10.00 ms");

  // Nothing chosen yet: the worst connection shows, and says so.
  const detail = page.getByTestId("map-detail");
  await expect(detail).toBeVisible();
  await expect(page.getByTestId("map-auto")).toBeVisible();

  // A click on the line: its chart under the schema, the choice in the URL.
  await line.locator(".lbl").click({ force: true });
  await expect(page).toHaveURL(new RegExp(`#map/link/${DEVICE}/${DEVICE}$`));
  await expect(line).toHaveClass(/\bsel\b/);
  await expect(page.getByTestId("map-auto")).toHaveCount(0);
  await expect(page.getByTestId("map-title")).toHaveText(`${DEVICE} → ${DEVICE}`);
  await expect(page.getByTestId("map-what")).toContainText("channel");
  await expect(page.getByTestId("map-latency").locator(".v")).toHaveText("10.00 ms", { timeout: 30_000 });
  // A count (the test device may lose a few packets as it starts).
  await expect(page.getByTestId("map-late").locator(".v")).toHaveText(/^\d[\d ]*$/);
  await expect(page.getByTestId("map-late").locator(".s")).toContainText("in the last hour");
  const chart = page.getByTestId("map-chart");
  await expect(chart.locator("canvas")).toBeVisible();
  await expect(chart.locator(".u-legend")).toContainText("latency 10.00 ms: a later packet is not played");
  expect(await chart.evaluate((el) => (el as any)._dcData.sources.length)).toBe(1);
  await expect(page.getByTestId("map-flows").getByTestId("lat-source")).toHaveCount(1);
  // The ranges read the same connection again.
  await page.locator('[data-testid="map-range"][data-range="300"]').click();
  await expect(page.locator('[data-testid="map-range"][data-range="300"]')).toHaveClass(/\bon\b/);
  await expect(chart.locator("canvas")).toBeVisible();

  // The bridge's way to Dante is a connection too: its hold time, its chart.
  const up = map.locator('[data-testid="map-edge"][data-from="SoundGrid"]');
  await expect(up.locator(".lbl text")).toContainText("7.30 ms");
  await up.locator(".lbl").click({ force: true });
  await expect(page).toHaveURL(new RegExp(`#map/bridge/${BRIDGE}/to-dante$`));
  await expect(page.getByTestId("map-title")).toHaveText("SoundGrid → Dante");
  await expect(page.getByTestId("map-held").locator(".v")).toHaveText("7.30 ms", { timeout: 30_000 });
  await expect(page.getByTestId("map-underruns").locator(".v")).toHaveText("0");
  await expect(chart.locator(".u-legend")).toContainText("SoundGrid → Dante");
  expect(await chart.evaluate((el) => (el as any)._dcData.sources.length)).toBe(1);
  const down = map.locator('[data-testid="map-edge"][data-to="SoundGrid"]');
  await expect(down.locator(".lbl text")).toContainText("7.50 ms");
  await page.screenshot({ path: "test-results/map.png", fullPage: true });

  // The choice survives a reload.
  await page.reload();
  await expect(page.getByTestId("map-title")).toHaveText("SoundGrid → Dante", { timeout: 30_000 });
  await expect(page.getByTestId("map-held").locator(".v")).toHaveText("7.30 ms", { timeout: 30_000 });

  // A click on a device's box opens the device (its detail comes to the map in step 2).
  await map.locator(`[data-testid="map-node"][data-node="${DEVICE}"]`).click();
  await expect(page).toHaveURL(new RegExp(`#devices/${DEVICE}$`));
  await expect(page.getByTestId("tab-devices")).toHaveClass(/\bon\b/);
  await expect(page.getByTestId("lat-title")).toHaveText(`Latency · ${DEVICE}`);
  expect(msgs).toEqual([]);
});
