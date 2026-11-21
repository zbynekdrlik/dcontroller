import { test, expect, type ConsoleMessage } from "@playwright/test";
import { DSYNC, nav } from "../env";

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
  await expect(line.locator(".lbl text")).toContainText("10 ms");
  // #124: a dstack device's box carries its own dsync clock next to the
  // Dante PTP word, as the device says it (/api/clocks).
  await expect(map.locator(`[data-testid="map-node"][data-node="${DEVICE}"] [data-testid="map-node-dsync"]`)).toHaveText(
    /^dsync (locked|limping|no time)$/,
    { timeout: 30_000 },
  );

  // #142: the colours speak of a window, 1 h by default; the switch moves
  // them to 3 h or 24 h, and the line's state follows /api/links for it.
  await expect(page.getByTestId("map-span-1h")).toHaveClass(/\bon\b/);
  const sum = (await page.request.get("/api/links").then((r) => r.json())).links.find(
    (l: any) => l.from === DEVICE && l.to === DEVICE,
  );
  const want = (late: number, peak: number, lat: number) =>
    late > 0 ? "bad" : lat > 0 && peak * 10 >= lat * 9 ? "warn" : "ok";
  await page.getByTestId("map-span-24h").click();
  await expect(page.getByTestId("map-span-24h")).toHaveClass(/\bon\b/);
  await expect(line).toHaveAttribute(
    "data-state",
    new RegExp(`^(${want(sum.late_24h, sum.peak_24h_us, sum.latency_24h_us)}|bad)$`),
  );
  await expect(page.locator(".legend")).toContainText("in the last 24 hours");
  await page.getByTestId("map-span-1h").click();
  await expect(page.locator(".legend")).toContainText("in the last hour");

  // The network's hour at a glance: connections, the bridge, and the
  // tiles the overview has.
  const top = page.getByTestId("map-tiles-top");
  // The Dante device's own connection and the stand-in video one (#124).
  await expect(top.getByTestId("tile-connections").locator(".tile-value")).toHaveText(/^\d+ \/ 2$/, { timeout: 40_000 });
  await expect(top.getByTestId("tile-bridges")).toContainText("1 bridge");
  await expect(top.getByTestId("tile-devices").locator(".tile-value")).toHaveText(String(devices.devices.length));
  await expect(top.getByTestId("tile-devices")).toContainText("all there since");
  await expect(top.getByTestId("tile-subs")).toContainText("Subscriptions");
  await expect(top.getByTestId("tile-clock")).toContainText("Clock");
  // The clock tile counts the dsync clocks too (#124): the suite's second
  // dsync node has nothing behind it, so the rig's clock is in trouble.
  await expect(top.getByTestId("tile-clock")).toContainText(`dsync not locked: ${DSYNC.down}`, { timeout: 30_000 });
  await expect(top.getByTestId("tile-clock")).toHaveClass(/\bbad\b/);
  await expect(top.getByTestId("tile-network")).toContainText("links");
  // As in the mockup (#142): the bridge's day, and the mDNS meter's word.
  await expect(top.getByTestId("tile-bridges")).toContainText(/\d+ in 24 h/);
  const md = top.getByTestId("tile-mdns");
  await expect(md.locator(".tile-value")).toHaveText(/^(counting|quiet)$/, { timeout: 30_000 });
  await expect(md).toContainText("mDNS");
  await expect(md).toHaveAttribute("href", "#network");
  // As in the mockup (#142): on a desktop every tile sits in one row, the
  // bridge's tile included (a seventh one wrapped to a row of its own).
  const tops = await top.locator(".tile").evaluateAll((l) => l.map((e) => Math.round(e.getBoundingClientRect().top)));
  expect(tops.length).toBe(7);
  expect(new Set(tops).size).toBe(1);

  // Links: every connection in one table, the worst first; a row is a click
  // on its line.
  await page.getByTestId("map-view-links").click();
  await expect(page.getByTestId("map-panel")).toBeHidden();
  const rows = page.getByTestId("map-links").getByTestId("map-dev-row");
  await expect(rows).toHaveCount(4);
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

  // #142, as mockup v2: "Do something" under the chart, and the receiver's
  // other connections (never the one shown), each a row with its hour.
  const acts = page.getByTestId("map-actions");
  await expect(acts.getByRole("link", { name: `Change the latency of ${DEVICE}` })).toHaveAttribute(
    "href",
    `#map/device/${DEVICE}`,
  );
  await expect(acts.getByRole("link", { name: "Open in Routing" })).toHaveAttribute("href", "#routing");
  await expect(page.getByTestId("map-changes")).toHaveAttribute("href", `#changes/${DEVICE}`);
  const others = page.getByTestId("map-others");
  if ((await others.count()) > 0) {
    await expect(others.locator(`[data-edge="#map/link/${DEVICE}/${DEVICE}"]`)).toHaveCount(0);
  }

  // The bridge's way to Dante is a connection too: its hold time, its chart.
  const up = map.locator('[data-testid="map-edge"][data-from="SoundGrid"]');
  await expect(up.locator(".lbl text")).toContainText("7.3 ms");
  await up.locator(".lbl").click({ force: true });
  await expect(page).toHaveURL(new RegExp(`#map/bridge/${BRIDGE}/to-dante$`));
  await expect(page.getByTestId("map-title")).toHaveText("SoundGrid → Dante");
  await expect(page.getByTestId("map-held").locator(".v")).toHaveText("7.30 ms", { timeout: 30_000 });
  await expect(page.getByTestId("map-underruns").locator(".v")).toHaveText("0");
  await expect(chart.locator(".u-legend")).toContainText("SoundGrid → Dante");
  expect(await chart.evaluate((el) => (el as any)._dcData.sources.length)).toBe(1);
  const down = map.locator('[data-testid="map-edge"][data-to="SoundGrid"]');
  await expect(down.locator(".lbl text")).toContainText("7.5 ms");
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
  // Its own chart (#130): everything it receives, as the Devices page had it.
  await expect(page.getByTestId("map-chart").locator("canvas")).toBeVisible({ timeout: 30_000 });
  await expect(page.locator('[data-testid="map-range"]').first()).toBeVisible();
  // Its connections, each with the last hour's figures.
  const incoming = page.getByTestId("map-dev-in").getByTestId("map-dev-row");
  await expect(incoming).toHaveCount(1);
  await expect(incoming).toContainText(`from ${DEVICE}`);
  await expect(incoming).toContainText("10 ms");
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
  await expect(inside.first()).toContainText("7.3 ms");
  // The device's link from a connection's panel leads here too.
  await inside.first().click();
  await expect(page.getByTestId("map-title")).toHaveText("SoundGrid → Dante");
  await page.getByTestId("map-actions").getByRole("link", { name: `Device ${BRIDGE}` }).click();
  await expect(page).toHaveURL(new RegExp(`#map/device/${BRIDGE}$`));
  expect(msgs).toEqual([]);
});

// #122, the Map at scale: the page has the whole screen (the menu behind a
// burger, the tiles one thin strip), and the schema answers a click on a
// device, "Problems only" and a name looked for.
test("the map has the screen: a burger menu, a status strip, a device's click dims the rest, problems only, find a device", async ({ page }) => {
  const msgs: string[] = [];
  page.on("console", (m: ConsoleMessage) => {
    if (m.type() === "error" || m.type() === "warning") msgs.push(`${m.type()}: ${m.text()}`);
  });
  page.on("pageerror", (e) => msgs.push(`pageerror: ${e.message}`));
  await page.setViewportSize({ width: 1600, height: 900 });
  await page.goto("/");
  const map = page.getByTestId("map");
  const node = (name: string) => map.locator(`[data-testid="map-node"][data-node="${name}"]`);
  await expect(node(DEVICE)).toBeVisible({ timeout: 40_000 });
  await expect(node(BRIDGE)).toBeVisible({ timeout: 40_000 });

  // The menu is closed and the page starts at the screen's edge.
  const menu = page.getByTestId("menu");
  await expect(menu).toBeHidden();
  expect(await page.locator(".main").evaluate((e) => e.getBoundingClientRect().left)).toBe(0);
  await expect(page.getByTestId("version")).toBeVisible();
  await page.getByTestId("menu-button").click();
  await expect(menu).toBeVisible();
  await expect(page.getByTestId("iface")).toBeVisible();
  await page.getByTestId("tab-clock").click();
  await expect(page.locator(".page-head h1")).toHaveText("Clock");
  await expect(menu).toBeHidden();
  // A click beside the open menu closes it.
  await page.getByTestId("menu-button").click();
  await expect(menu).toBeVisible();
  await page.getByTestId("menu-scrim").click({ position: { x: 900, y: 400 } });
  await expect(menu).toBeHidden();
  await nav(page, "map");
  await expect(node(DEVICE)).toBeVisible();

  // The status strip: one thin row, level with the title; the small text
  // of an item shows when it is pointed at.
  const strip = (await page.getByTestId("map-tiles-top").boundingBox())!;
  const title = (await page.locator(".page-head h1").boundingBox())!;
  expect(strip.height).toBeLessThanOrEqual(36);
  expect(Math.abs(strip.y + strip.height / 2 - (title.y + title.height / 2))).toBeLessThan(6);
  const conn = page.getByTestId("tile-connections");
  await expect(conn.locator(".tile-sub")).toBeHidden();
  await conn.hover();
  await expect(conn.locator(".tile-sub")).toBeVisible();
  // The schema starts right under it and ends on the screen.
  const drawn = (await map.locator("svg.map").boundingBox())!;
  expect(drawn.y).toBeLessThan(130);
  expect(drawn.y + drawn.height).toBeLessThanOrEqual(900);

  // A click on a device: the others step back, the page stays where it is.
  await node(DEVICE).click();
  await expect(page).toHaveURL(new RegExp(`#map/device/${DEVICE}$`));
  await expect(map.locator("svg.map")).toHaveClass(/\bhas-device\b/);
  await expect(node(BRIDGE)).toHaveCSS("opacity", "0.3");
  await expect(node(DEVICE)).toHaveCSS("opacity", "1");
  await expect(page.getByTestId("map-title")).toHaveText(DEVICE);
  await page.waitForTimeout(300);
  expect(await page.evaluate(() => window.scrollY)).toBe(0);
  // A second click lets the whole map back.
  await node(DEVICE).click();
  await expect(page).toHaveURL(/#map$/);
  await expect(node(BRIDGE)).toHaveCSS("opacity", "1");

  // Problems only: every good or unmeasured line steps back, the others stay.
  const edges = map.locator('[data-testid="map-edge"]');
  const states = await edges.evaluateAll((es) => es.map((e) => e.getAttribute("data-state")));
  expect(states.length).toBeGreaterThanOrEqual(3);
  await expect(map.locator(".quiet")).toHaveCount(0);
  await page.getByTestId("map-problems").click();
  await expect(page.getByTestId("map-problems")).toHaveAttribute("aria-pressed", "true");
  await expect(map.locator("svg.map")).toHaveClass(/\bproblems\b/);
  const good = states.filter((s) => s === "ok" || s === "unknown").length;
  await expect(map.locator('[data-testid="map-edge"].quiet')).toHaveCount(good);
  if (good > 0) await expect(map.locator('[data-testid="map-edge"].quiet').first()).toHaveCSS("opacity", "0.1");
  await page.getByTestId("map-problems").click();
  await expect(map.locator(".quiet")).toHaveCount(0);

  // A name looked for marks its device and no other.
  await page.getByTestId("map-find").fill("E2E-BRIDGE");
  await expect(node(BRIDGE)).toHaveClass(/\bhit\b/);
  await expect(node(DEVICE)).not.toHaveClass(/\bhit\b/);
  await page.getByTestId("map-find").fill("");
  await expect(map.locator(".node.hit")).toHaveCount(0);
  await page.screenshot({ path: "test-results/map-scale.png", fullPage: true });
  expect(msgs).toEqual([]);
});

// #122, places: the owner names where a device stands; the devices of one
// place stand in one area, and a site with no place set keeps its columns.
test("a device gets a place: the map draws the places as areas, the change is logged, and without places the columns are back", async ({ page }) => {
  const msgs: string[] = [];
  page.on("console", (m: ConsoleMessage) => {
    if (m.type() === "error" || m.type() === "warning") msgs.push(`${m.type()}: ${m.text()}`);
  });
  page.on("pageerror", (e) => msgs.push(`pageerror: ${e.message}`));
  const clear = async () => {
    for (const device of [DEVICE, BRIDGE, "cam-e2e"]) {
      expect((await page.request.post("/api/place", { data: { device, place: "" } })).status()).toBe(200);
    }
  };
  await page.setViewportSize({ width: 1600, height: 900 });
  await page.goto(`/#map/device/${DEVICE}`);
  const map = page.getByTestId("map");
  const node = (name: string) => map.locator(`[data-testid="map-node"][data-node="${name}"]`);
  await expect(node(DEVICE)).toBeVisible({ timeout: 40_000 });
  // The video nodes come with the server's first good read of their list.
  await expect(node("cam-e2e")).toBeVisible({ timeout: 90_000 });
  try {
    // No place yet: the columns, no area.
    await expect(map.locator('[data-testid="map-area"]')).toHaveCount(0);
    await expect(map.locator(".col-title").first()).toContainText(/sends|receives/);
    await expect(page.getByTestId("place-input")).toHaveValue("");

    // The server refuses what is not a place's name.
    const long = await page.request.post("/api/place", { data: { device: DEVICE, place: "x".repeat(41) } });
    expect(long.status()).toBe(400);
    expect(await long.text()).toContain("at most 40 characters");

    await page.getByTestId("place-input").fill("Stage");
    await page.getByTestId("place-save").click();
    await expect(page.getByTestId("place-note")).toHaveText(`${DEVICE}: in Stage`);
    const places = await page.request.get("/api/places").then((r) => r.json());
    expect(places.places).toEqual([{ device: DEVICE, place: "Stage" }]);
    expect(places.names).toContain("Stage");
    // The areas: Stage, and one for what has no place.
    await expect(map.locator('[data-testid="map-area"]')).toHaveCount(2);
    await expect(map.locator(".col-title")).toHaveText(["Stage", "no place set"]);
    // The device stands inside its area, the bridge in the other.
    const inside = async (name: string, area: number) => {
      const n = (await node(name).locator("rect").boundingBox())!;
      const a = (await map.locator('[data-testid="map-area"]').nth(area).boundingBox())!;
      return n.x >= a.x && n.x + n.width <= a.x + a.width && n.y >= a.y && n.y + n.height <= a.y + a.height;
    };
    expect(await inside(DEVICE, 0)).toBe(true);
    expect(await inside(BRIDGE, 1)).toBe(true);
    expect(await inside(BRIDGE, 0)).toBe(false);

    // A second place, offered from the ones known; the bridge's ASIO side follows it.
    await page.goto(`/#map/device/${BRIDGE}`);
    await expect(page.getByTestId("map-title")).toHaveText(BRIDGE);
    await expect(page.locator('[data-testid="place-names"] option')).toHaveCount(places.names.length);
    await page.getByTestId("place-input").fill("FOH");
    await page.getByTestId("place-save").click();
    await expect(page.getByTestId("place-note")).toHaveText(`${BRIDGE}: in FOH`);
    await expect(map.locator(".col-title")).toHaveText(["Stage", "FOH", "no place set"]);
    expect(await inside(BRIDGE, 1)).toBe(true);
    expect(await inside("SoundGrid", 1)).toBe(true);
    await page.screenshot({ path: "test-results/map-places.png", fullPage: true });

    // It stays after a reload, and the change is in the log.
    await page.reload();
    await expect(map.locator('[data-testid="map-area"]')).toHaveCount(3, { timeout: 40_000 });
    await expect(page.getByTestId("place-input")).toHaveValue("FOH");
    const log = await page.request.get("/api/changes").then((r) => r.json());
    const made = log.find((e: any) => e.kind === "place" && e.rx_device === BRIDGE);
    expect(made).toMatchObject({ ok: true, name_before: null, name_asked: "FOH", name_after: "FOH" });
    await nav(page, "changes");
    await expect(page.getByText(`place on the Map @ ${BRIDGE}`).first()).toBeVisible();

    // A video node has no detail of its own: a connection it ends holds
    // the field for each of its two ends.
    const video = await page.request.get("/api/video").then((r) => r.json());
    const conn = video.connections.find((c: any) => c.source === "cam-e2e");
    await page.goto(`/#map/video/${encodeURIComponent(conn.id)}`);
    const cam = page.locator('[data-testid="place-edit"][data-device="cam-e2e"]');
    await expect(cam).toContainText("Place of cam-e2e on the Map", { timeout: 40_000 });
    await expect(page.locator(`[data-testid="place-edit"][data-device="${conn.sink}"]`)).toBeVisible();
    await cam.getByTestId("place-input").fill("Stage");
    await cam.getByTestId("place-save").click();
    await expect(cam.getByTestId("place-note")).toHaveText("cam-e2e: in Stage");
    await expect.poll(() => inside("cam-e2e", 0)).toBe(true);
    expect((await page.request.post("/api/place", { data: { device: "cam-e2e", place: "" } })).status()).toBe(200);
    // A place no device stands in is no longer offered.
    await page.request.post("/api/place", { data: { device: "cam-e2e", place: "Typo" } });
    expect((await page.request.get("/api/places").then((r) => r.json())).names).toContain("Typo");
    await page.request.post("/api/place", { data: { device: "cam-e2e", place: "" } });
    expect((await page.request.get("/api/places").then((r) => r.json())).names).toEqual(["Stage", "FOH"]);

    // Emptied: no place, and with none left the columns are back.
    await page.goto(`/#map/device/${BRIDGE}`);
    await expect(page.getByTestId("place-input")).toHaveValue("FOH", { timeout: 40_000 });
    await page.getByTestId("place-input").fill("");
    await page.getByTestId("place-save").click();
    await expect(page.getByTestId("place-note")).toHaveText(`${BRIDGE}: no place`);
    await expect(map.locator('[data-testid="map-area"]')).toHaveCount(2);
    await clear();
    await page.reload();
    await expect(node(DEVICE)).toBeVisible({ timeout: 40_000 });
    await expect(map.locator('[data-testid="map-area"]')).toHaveCount(0);
    await expect(map.locator(".col-title").first()).toContainText(/sends|receives/);
  } finally {
    await clear();
  }
  expect(msgs).toEqual([]);
});

// #122, dragging: the owner moves a box with the mouse instead of typing a
// place for each device, and orders the boxes of a place himself.
test("a device is dragged on the map: to a new place, into another place, up and down in it, and out of every place", async ({ page }) => {
  const msgs: string[] = [];
  page.on("console", (m: ConsoleMessage) => {
    if (m.type() === "error" || m.type() === "warning") msgs.push(`${m.type()}: ${m.text()}`);
  });
  page.on("pageerror", (e) => msgs.push(`pageerror: ${e.message}`));
  const clear = async () => {
    for (const device of [DEVICE, BRIDGE]) {
      expect((await page.request.post("/api/place", { data: { device, place: "" } })).status()).toBe(200);
    }
  };
  const stored = async () =>
    (await page.request.get("/api/places").then((r) => r.json())).places.map((p: any) => `${p.device}@${p.place}`);
  await page.setViewportSize({ width: 1600, height: 900 });
  await clear();
  await page.goto("/#map");
  const map = page.getByTestId("map");
  const node = (name: string) => map.locator(`[data-testid="map-node"][data-node="${name}"]`);
  const area = (place: string) => map.locator(`[data-testid="map-area"][data-place="${place}"]`);
  // A box's place on the screen; the schema is drawn again on a change,
  // so a box may be gone for a moment.
  const box = async (l: ReturnType<typeof node>) => {
    for (let i = 0; i < 50; i++) {
      const b = await l.locator("rect").first().boundingBox().catch(() => null);
      if (b) return b;
      await page.waitForTimeout(100);
    }
    throw new Error("the box is not on the map");
  };
  await expect(node(DEVICE)).toBeVisible({ timeout: 40_000 });
  await expect(node(BRIDGE)).toBeVisible({ timeout: 40_000 });
  await expect(node("cam-e2e")).toBeVisible({ timeout: 90_000 });
  // Takes the box of `name` and lets it go at the point `to` gives once the
  // drag has begun (the "new place" target is there only while dragging).
  const drag = async (name: string, to: () => Promise<{ x: number; y: number }>) => {
    const b = await box(node(name));
    await page.mouse.move(b.x + b.width / 2, b.y + b.height / 2);
    await page.mouse.down();
    await page.mouse.move(b.x + b.width / 2 + 30, b.y + b.height / 2 + 10, { steps: 4 });
    await expect(page.getByTestId("map-new-place")).toBeVisible();
    await expect(page.locator(".map-drag-ghost")).toHaveText(name);
    const p = await to();
    await page.mouse.move(p.x, p.y, { steps: 8 });
    await page.mouse.up();
    await expect(page.getByTestId("map-new-place")).toHaveCount(0);
  };
  const middle = (r: { x: number; y: number; width: number; height: number }) => ({ x: r.x + r.width / 2, y: r.y + r.height / 2 });
  try {
    // No place yet: the box goes to "new place", which asks its name once.
    page.once("dialog", (d) => d.accept("  Stage "));
    await drag(DEVICE, async () => middle((await page.getByTestId("map-new-place").boundingBox())!));
    await expect.poll(stored).toEqual([`${DEVICE}@Stage`]);
    await expect(map.locator(".col-title")).toHaveText(["Stage", "no place set"]);
    // A drag is not a click: no device was chosen.
    await expect(page).toHaveURL(/#map$/);

    // Into that place, under the device there.
    await drag(BRIDGE, async () => {
      const a = (await area("Stage").boundingBox())!;
      const d = await box(node(DEVICE));
      return { x: a.x + a.width / 2, y: d.y + d.height + 30 };
    });
    await expect.poll(stored).toEqual([`${DEVICE}@Stage`, `${BRIDGE}@Stage`]);
    await expect(map.locator('[data-testid="map-area"]')).toHaveCount(2);
    await expect.poll(async () => (await box(node(DEVICE))).y < (await box(node(BRIDGE))).y).toBe(true);
    // Its ASIO side came along, under it.
    expect((await box(node("SoundGrid"))).y).toBeGreaterThan((await box(node(BRIDGE))).y);

    // Up: above the first one, and the map stands so.
    await drag(BRIDGE, async () => {
      const a = (await area("Stage").boundingBox())!;
      const d = await box(node(DEVICE));
      return { x: a.x + a.width / 2, y: d.y + 6 };
    });
    await expect.poll(stored).toEqual([`${BRIDGE}@Stage`, `${DEVICE}@Stage`]);
    await expect.poll(async () => (await box(node(BRIDGE))).y < (await box(node(DEVICE))).y).toBe(true);
    await page.screenshot({ path: "test-results/map-drag.png" });
    // It holds after a reload.
    await page.reload();
    await expect(node(DEVICE)).toBeVisible({ timeout: 40_000 });
    await expect.poll(async () => (await box(node(BRIDGE))).y < (await box(node(DEVICE))).y).toBe(true);

    // Let go beside every area: nothing changes.
    await drag(DEVICE, async () => ({ x: 20, y: 880 }));
    await page.waitForTimeout(500);
    expect(await stored()).toEqual([`${BRIDGE}@Stage`, `${DEVICE}@Stage`]);

    // Out: onto "no place set".
    await drag(DEVICE, async () => middle((await area("").boundingBox())!));
    await expect.poll(stored).toEqual([`${BRIDGE}@Stage`]);
    // Each drop is in the changes log.
    const log = await page.request.get("/api/changes").then((r) => r.json());
    expect(log.filter((e: any) => e.kind === "place" && e.rx_device === DEVICE && e.ok).length).toBeGreaterThanOrEqual(2);
  } finally {
    await clear();
  }
  expect(msgs).toEqual([]);
});

// #122: a device that is switched off has a box too ("not on the network"),
// and the owner must be able to put it in its place. Its box is drawn with
// no fill, and the mouse fell through it onto the area behind.
test("the box of a device that is not on the network is dragged to a place like any other", async ({ page }) => {
  const msgs: string[] = [];
  page.on("console", (m: ConsoleMessage) => {
    if (m.type() === "error" || m.type() === "warning") msgs.push(`${m.type()}: ${m.text()}`);
  });
  page.on("pageerror", (e) => msgs.push(`pageerror: ${e.message}`));
  const OFF = "dc-e2e-switched-off";
  const place = (device: string, name: string) => page.request.post("/api/place", { data: { device, place: name } });
  const subscribe = (source: { channel: string; device: string } | null) =>
    page.request.post("/api/subscription", { data: { rx_device: DEVICE, rx_channel: 2, source } });
  await page.setViewportSize({ width: 1600, height: 900 });
  await page.goto("/#map");
  const map = page.getByTestId("map");
  const node = (name: string) => map.locator(`[data-testid="map-node"][data-node="${name}"]`);
  await expect(node(DEVICE)).toBeVisible({ timeout: 40_000 });
  try {
    expect((await subscribe({ channel: "Mic", device: OFF })).status()).toBe(200);
    expect((await place(DEVICE, "Stage")).status()).toBe(200);
    await expect(node(OFF)).toHaveClass(/\bmissing\b/, { timeout: 40_000 });
    await expect(map.locator('[data-testid="map-area"][data-place="Stage"]')).toHaveCount(1, { timeout: 30_000 });
    // The pointer over the middle of its box is over the box.
    const b = (await node(OFF).locator("rect").first().boundingBox())!;
    const [x, y] = [b.x + b.width / 2, b.y + b.height / 2];
    expect(await page.evaluate(([px, py]) => document.elementFromPoint(px, py)?.closest("g.node")?.getAttribute("data-node") ?? null, [x, y])).toBe(OFF);
    await page.mouse.move(x, y);
    await page.mouse.down();
    await page.mouse.move(x + 30, y + 10, { steps: 4 });
    await expect(page.locator(".map-drag-ghost")).toHaveText(OFF);
    const a = (await map.locator('[data-testid="map-area"][data-place="Stage"]').boundingBox())!;
    await page.mouse.move(a.x + a.width / 2, a.y + a.height - 30, { steps: 8 });
    await page.mouse.up();
    await expect
      .poll(async () => (await page.request.get("/api/places").then((r) => r.json())).places.map((p: any) => `${p.device}@${p.place}`))
      .toEqual([`${DEVICE}@Stage`, `${OFF}@Stage`]);
    await expect(node(OFF)).toHaveAttribute("data-place", "Stage");
  } finally {
    await page.mouse.up();
    await subscribe(null);
    await place(OFF, "");
    await place(DEVICE, "");
  }
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
  // Every tile in, the mDNS one last (it waits on its own poll).
  await expect(tiles.getByTestId("tile-mdns")).toBeVisible({ timeout: 30_000 });
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
