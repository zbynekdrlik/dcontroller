import { test, expect, type ConsoleMessage } from "@playwright/test";
import { DSYNC } from "../env";

// The Map (#122): the schema of what is connected to what, and the chart
// of the connection the user clicks. The test device receives its own TX
// (one connection, a loop on its box); the bridge test device has its two
// delays as two connections to its ASIO side.
const DEVICE = "dc-e2e-device";
const BRIDGE = "dc-e2e-bridge";

test("the map is the start page: every device a box, every connection a line, a click shows that connection's chart or that device's detail", async ({ page }) => {
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

  // The network's hour at a glance: connections, the bridge, and the
  // tiles the overview has.
  const top = page.getByTestId("map-tiles-top");
  await expect(top.getByTestId("tile-connections").locator(".tile-value")).toHaveText(/^\d+ \/ 1$/, { timeout: 40_000 });
  await expect(top.getByTestId("tile-bridges")).toContainText("1 bridge");
  await expect(top.getByTestId("tile-devices").locator(".tile-value")).toHaveText(String(devices.devices.length));
  await expect(top.getByTestId("tile-subs")).toContainText("Subscriptions");
  await expect(top.getByTestId("tile-clock")).toContainText("Clock");
  // The clock tile counts the dsync clocks too (#124): the suite's second
  // dsync node has nothing behind it, so the rig's clock is in trouble.
  await expect(top.getByTestId("tile-clock")).toContainText(`dsync not locked: ${DSYNC.down}`, { timeout: 30_000 });
  await expect(top.getByTestId("tile-clock")).toHaveClass(/\bbad\b/);
  await expect(top.getByTestId("tile-network")).toContainText("links");

  // Links: every connection in one table, the worst first; a row is a click
  // on its line.
  await page.getByTestId("map-view-links").click();
  await expect(page.getByTestId("map-panel")).toBeHidden();
  const rows = page.getByTestId("map-links").getByTestId("map-dev-row");
  await expect(rows).toHaveCount(3);
  await expect(page.getByTestId("map-links")).toContainText(`${DEVICE} → ${DEVICE}`);
  await expect(page.getByTestId("map-links")).toContainText("SoundGrid → Dante");
  await rows.filter({ hasText: "Dante → SoundGrid" }).click();
  await expect(page).toHaveURL(new RegExp(`#map/bridge/${BRIDGE}/to-asio$`));
  await expect(page.getByTestId("map-title")).toHaveText("Dante → SoundGrid");
  await expect(rows.filter({ hasText: "Dante → SoundGrid" })).toHaveClass(/\bsel\b/);
  // Devices: every device in one table (step 4); a row chooses the device.
  await page.getByTestId("map-view-devices").click();
  await expect(page.getByTestId("map-links-panel")).toBeHidden();
  const drows = page.getByTestId("map-devices").getByTestId("map-device-row");
  await expect(drows).toHaveCount(devices.devices.length);
  const drow = page.locator(`[data-testid="map-device-row"][data-name="${DEVICE}"]`);
  await expect(drow).toContainText("4 TX · 4 RX");
  await expect(drow).toContainText("latency 10.00 ms");
  await expect(drow).toContainText("there at start");
  await page.getByTestId("map-sort-newest").click();
  await expect(page.getByTestId("map-sort-newest")).toHaveClass(/\bon\b/);
  await expect(drows).toHaveCount(devices.devices.length);
  await page.getByTestId("map-sort-name").click();
  await drow.click();
  await expect(page).toHaveURL(new RegExp(`#map/device/${DEVICE}$`));
  await expect(page.getByTestId("map-title")).toHaveText(DEVICE);
  await expect(drow).toHaveClass(/\bsel\b/);
  await expect(page.getByTestId("map-device-body").getByTestId("rename-device")).toBeVisible();
  await page.getByTestId("map-view-schema").click();
  await expect(page.getByTestId("map-panel")).toBeVisible();
  await expect(page.getByTestId("map-links-panel")).toBeHidden();
  await expect(page.getByTestId("map-devices-panel")).toBeHidden();
  await page.goto("/#map");

  // Nothing chosen: the worst connection shows, and says so.
  const detail = page.getByTestId("map-detail");
  await expect(detail).toBeVisible();
  await expect(page.getByTestId("map-auto")).toBeVisible();

  // A click on the line: its chart under the schema, the choice in the URL.
  await line.locator(".lbl").click({ force: true });
  await expect(page).toHaveURL(new RegExp(`#map/link/${DEVICE}/${DEVICE}$`));
  await expect(line).toHaveClass(/\bsel\b/);
  await expect(page.getByTestId("map-auto")).toHaveCount(0);
  await expect(page.getByTestId("map-title")).toHaveText(`${DEVICE} → ${DEVICE}`);
  // #142: the detail and its chart come into view; under the schema they
  // were a screen below and the owner had to look for them.
  await expect(page.getByTestId("map-title")).toBeInViewport();
  await expect
    .poll(() => page.getByTestId("map-detail").evaluate((e) => Math.round(e.getBoundingClientRect().top)), { timeout: 5_000 })
    .toBeLessThan(40);
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

  // A click on a device's box: its detail under the schema (step 2), its
  // box and its connections standing out.
  await map.locator(`[data-testid="map-node"][data-node="${DEVICE}"]`).click();
  await expect(page).toHaveURL(new RegExp(`#map/device/${DEVICE}$`));
  await expect(page.getByTestId("tab-map")).toHaveClass(/\bon\b/);
  await expect(page.getByTestId("map-title")).toHaveText(DEVICE);
  await expect(map.locator(`[data-testid="map-node"][data-node="${DEVICE}"]`)).toHaveClass(/\bsel\b/);
  await expect(line).toHaveClass(/\bof\b/);
  await expect(page.getByTestId("map-what")).toContainText("4 TX · 4 RX");
  await expect(page.getByTestId("map-chart")).toBeHidden();
  await expect(page.locator('[data-testid="map-range"]').first()).toBeHidden();
  // Its connections, each with the last hour's figures.
  const incoming = page.getByTestId("map-dev-in").getByTestId("map-dev-row");
  await expect(incoming).toHaveCount(1);
  await expect(incoming).toContainText(`from ${DEVICE}`);
  await expect(incoming).toContainText("10.00 ms");
  await expect(incoming.locator("svg.spark path.line")).toHaveAttribute("d", /^M/);
  await expect(page.getByTestId("map-dev-out").getByTestId("map-dev-row")).toHaveCount(1);
  // Everything today's device card holds, by the same code.
  const body = page.getByTestId("map-device-body");
  await expect(body.getByTestId("settings-table")).toContainText("Sample rate");
  await expect(body.getByTestId("rename-device")).toBeVisible();
  await expect(body.getByTestId("change-latency")).toBeVisible();
  await expect(body.getByTestId("rx-table").getByTestId("rx-channel")).toHaveCount(4);
  await expect(body.getByTestId("tx-list").getByTestId("tx-channel")).toHaveCount(4);
  // A change asked from here opens the same confirm dialog as on the card.
  await body.getByTestId("change-latency").click();
  await expect(page.getByTestId("setting-dialog")).toBeVisible();
  await page.getByTestId("setting-cancel").click();
  await expect(page.getByTestId("setting-dialog")).toBeHidden();
  await page.screenshot({ path: "test-results/map-device.png", fullPage: true });
  // A row leads to that connection's chart.
  await incoming.click();
  await expect(page).toHaveURL(new RegExp(`#map/link/${DEVICE}/${DEVICE}$`));
  await expect(page.getByTestId("map-chart").locator("canvas")).toBeVisible();
  // The bridge: its two ways inside it.
  await map.locator(`[data-testid="map-node"][data-node="${BRIDGE}"]`).click();
  await expect(page.getByTestId("map-title")).toHaveText(BRIDGE);
  const inside = page.getByTestId("map-dev-bridge").getByTestId("map-dev-row");
  await expect(inside).toHaveCount(2);
  await expect(inside.first()).toContainText("SoundGrid → Dante");
  await expect(inside.first()).toContainText("7.30 ms");
  // The device's link from a connection's panel leads here too.
  await inside.first().click();
  await expect(page.getByTestId("map-title")).toHaveText("SoundGrid → Dante");
  await page.getByTestId("map-actions").getByRole("link", { name: `Device ${BRIDGE}` }).click();
  await expect(page).toHaveURL(new RegExp(`#map/device/${BRIDGE}$`));
  expect(msgs).toEqual([]);
});

test("on a phone the map opens on the connections' table and no page scrolls sideways", async ({ page }) => {
  const msgs: string[] = [];
  page.on("console", (m: ConsoleMessage) => {
    if (m.type() === "error" || m.type() === "warning") msgs.push(`${m.type()}: ${m.text()}`);
  });
  page.on("pageerror", (e) => msgs.push(`pageerror: ${e.message}`));
  // A small phone: narrower than the strip of ranges, so the rows must
  // let it scroll in itself (a wider font on another machine does the
  // same to a 390 px phone).
  await page.setViewportSize({ width: 330, height: 700 });
  // What reaches past the window's right edge (nothing, when the page
  // does not scroll sideways): named, so a failure says what to fix.
  const wide = () =>
    page.evaluate(() => {
      const w = window.innerWidth;
      if (document.documentElement.scrollWidth <= w) return [];
      // Not what a scrolling ancestor clips (the nav's own strip, a table
      // in its box): only what pushes the page itself wider.
      const clipped = (e: Element) => {
        for (let p = e.parentElement; p && p !== document.body; p = p.parentElement) {
          const o = getComputedStyle(p).overflowX;
          if (o !== "visible" && p.getBoundingClientRect().right <= w + 1) return true;
        }
        return false;
      };
      return Array.from(document.querySelectorAll("body *"))
        .filter((e) => e.getBoundingClientRect().right > w + 1 && !clipped(e))
        .slice(0, 6)
        .map((e) => {
          const h = e as HTMLElement;
          return `${h.tagName.toLowerCase()}.${String(h.className).replace(/\s+/g, ".")}[${h.dataset.testid ?? ""}] right=${Math.round(h.getBoundingClientRect().right)}`;
        });
    });

  // One column of the schema fits a phone: the Links table shows first, the
  // schema one tap away (#123).
  await page.goto("/#map");
  await expect(page.getByTestId("map-view-links")).toHaveClass(/\bon\b/);
  await expect(page.getByTestId("map-links").getByTestId("map-dev-row").first()).toBeVisible({ timeout: 40_000 });
  await expect(page.getByTestId("map-panel")).toBeHidden();
  // The health tiles sit two to a row: all of them within the first screen.
  const tiles = page.getByTestId("map-tiles-top");
  expect((await tiles.boundingBox())!.height).toBeLessThan(420);
  expect(await wide()).toEqual([]);
  await page.getByTestId("map-view-schema").click();
  await expect(page.getByTestId("map").locator("svg.map")).toBeVisible();
  // The schema scrolls inside its box, not the page.
  expect(await wide()).toEqual([]);

  // The wide tables scroll inside their boxes too.
  for (const view of ["changes", "routing", "clock", "network"]) {
    await page.goto(`/#${view}`);
    await expect(page.locator(".page-head h1")).not.toBeEmpty();
    await page.waitForTimeout(500);
    expect(await wide(), `${view} fits the phone's width`).toEqual([]);
  }
  expect(msgs).toEqual([]);
});
