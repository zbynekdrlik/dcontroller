import { test, expect, type ConsoleMessage } from "@playwright/test";

// The Overview and Devices pages live on the Map (#130): the menu has
// neither, their links land on the Map, and the Map's Links table carries
// the Overview's late seconds.
const DEVICE = "dc-e2e-device";

test("the old pages' links land on the Map, which holds what they showed", async ({ page }) => {
  const msgs: string[] = [];
  page.on("console", (m: ConsoleMessage) => {
    if (m.type() === "error" || m.type() === "warning") msgs.push(`${m.type()}: ${m.text()}`);
  });
  page.on("pageerror", (e) => msgs.push(`pageerror: ${e.message}`));

  await page.goto("/#overview");
  await expect(page).toHaveURL(/#map$/);
  await expect(page.getByTestId("tab-map")).toHaveClass(/\bon\b/);
  await expect(page.locator(".page-head h1")).toHaveText("Map");
  await expect(page.getByTestId("tab-overview")).toHaveCount(0);
  await expect(page.getByTestId("tab-devices")).toHaveCount(0);
  await expect(page.locator(".nav-item")).toHaveText(["Map", "Routing", "Meters", "Clock", "Network", "Changes"]);

  // The Links table: the device's own connection with its late seconds,
  // the figure the server counts for it.
  await page.getByTestId("map-view-links").click();
  const row = page.locator(`[data-testid="map-links"] [data-edge="#map/link/${DEVICE}/${DEVICE}"]`);
  await expect(row).toBeVisible({ timeout: 40_000 });
  await expect(page.getByTestId("map-links").locator("thead")).toContainText("Late seconds 1 h");
  const links = await page.request.get("/api/links").then((r) => r.json());
  const own = links.links.find((l: any) => l.from === DEVICE && l.to === DEVICE);
  expect(typeof own.late_s_1h).toBe("number");
  await expect(row.getByTestId("late-seconds")).toHaveText(/^\d[\d ]*$/);

  // A Devices link lands on the Map's device table; one device's link (and
  // an old latency link) on that device's detail, with its chart.
  await page.goto("/#devices");
  await expect(page).toHaveURL(/#map\/devices$/);
  await expect(page.getByTestId("map-devices")).toBeVisible({ timeout: 30_000 });
  await expect(page.getByTestId("map-view-devices")).toHaveClass(/\bon\b/);

  await page.goto(`/#devices/${DEVICE}`);
  await expect(page).toHaveURL(new RegExp(`#map/device/${DEVICE}$`));
  await expect(page.getByTestId("map-title")).toHaveText(DEVICE, { timeout: 30_000 });
  await expect(page.getByTestId("map-chart")).toBeVisible({ timeout: 40_000 });
  await expect(page.getByTestId("map-chart").locator("canvas").first()).toBeVisible();
  await expect(page.getByTestId("map-device-body").getByTestId("rename-device")).toBeVisible();

  await page.goto(`/#latency/${DEVICE}`);
  await expect(page).toHaveURL(new RegExp(`#map/device/${DEVICE}$`));
  await expect(page.getByTestId("map-title")).toHaveText(DEVICE);

  // The Devices tile opens the device table.
  await page.goto("/#map");
  await page.getByTestId("map-view-schema").click();
  await page.getByTestId("map-tiles-top").getByTestId("tile-devices").click();
  await expect(page).toHaveURL(/#map\/devices$/);
  await expect(page.getByTestId("map-devices")).toBeVisible();
  expect(msgs).toEqual([]);
});
