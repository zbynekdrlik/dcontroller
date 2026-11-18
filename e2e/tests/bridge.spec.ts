import { test, expect, type ConsoleMessage } from "@playwright/test";

// A bridge device's own figures (#108): the bridge test device (a real
// dstack device holding bridge figures, global-setup.ts) answers dstack's
// info request; dcontroller reads it once a second, stores a read every
// 10 s and shows it under the device's latency on the Devices page.
const BRIDGE = "dc-e2e-bridge";
const PLAIN = "dc-e2e-device";

test("a bridge device's figures show under its latency; a device without a bridge shows none", async ({ page }) => {
  const msgs: string[] = [];
  page.on("console", (m: ConsoleMessage) => {
    if (m.type() === "error" || m.type() === "warning") msgs.push(`${m.type()}: ${m.text()}`);
  });
  page.on("pageerror", (e) => msgs.push(`pageerror: ${e.message}`));

  // The server has read and stored it (the first store comes at once).
  await expect
    .poll(
      async () => (await page.request.get(`/api/bridge/${BRIDGE}`).then((r) => r.json())).points.length,
      { timeout: 40_000 },
    )
    .toBeGreaterThan(0);
  const b = await page.request.get(`/api/bridge/${BRIDGE}`).then((r) => r.json());
  const p = b.points[b.points.length - 1];
  expect(p.latency_us).toBe(1000);
  expect(p.to_asio_target_us).toBe(1000);
  expect(p.from_asio_target_us).toBe(1667);
  expect(p.to_asio_converter_delay_us).toBe(167);
  expect(p.to_asio_cause).toBe("driver");
  expect(p.from_asio_cause).toBe("none");
  expect(p.late_bins).toEqual([40, 12, 5, 2, 1, 0, 0, 1]);
  // Its own card-to-card time each way and its driver (#117).
  expect(p.card_asio_to_dante_us).toBe(7300);
  expect(p.card_dante_to_asio_us).toBe(7500);
  expect(p.driver).toBe("SoundGrid");
  // A device without a bridge has nothing stored.
  expect((await page.request.get(`/api/bridge/${PLAIN}`).then((r) => r.json())).points).toEqual([]);

  // On the page: the device's latency, then its bridge, in ms.
  await page.goto(`/#devices/${BRIDGE}`);
  await expect(page.getByTestId("lat-title")).toHaveText(`Latency · ${BRIDGE}`);
  const panel = page.getByTestId("bridge-panel");
  await expect(panel).toBeVisible({ timeout: 20_000 });
  // Two plain lines on top: how long the bridge holds a sample each way.
  const lineDante = panel.getByTestId("bridge-line-dante");
  await expect(lineDante.locator("th")).toHaveText("SoundGrid → Dante");
  await expect(lineDante.locator("td")).toHaveText("7.30 ms");
  const lineAsio = panel.getByTestId("bridge-line-asio");
  await expect(lineAsio.locator("th")).toHaveText("Dante → SoundGrid");
  await expect(lineAsio.locator("td")).toHaveText("7.50 ms");
  // Read as written, not in the table headers' capitals.
  for (const th of [lineDante.locator("th"), lineAsio.locator("th")]) {
    expect(await th.evaluate((el) => getComputedStyle(el).textTransform)).toBe("none");
  }
  await expect(panel.getByTestId("bridge-chart").locator(".u-legend")).toContainText("SoundGrid → Dante");
  expect(await panel.getByTestId("bridge-chart").evaluate((el) => (el as any)._dcData.axis)).toBe("card to card, ms");
  // The parts, underruns and lateness sit closed under Details.
  await expect(panel.getByTestId("bridge-table")).toBeHidden();
  await panel.getByTestId("bridge-details").locator("summary").click();
  await expect(panel.getByTestId("bridge-table")).toBeVisible();
  await expect(panel.getByTestId("bridge-to").locator("td").first()).toHaveText("2.17 ms");
  await expect(panel.getByTestId("bridge-to")).toContainText("latency 1.00 ms + target 1.00 ms + converter 0.17 ms");
  await expect(panel.getByTestId("bridge-from").locator("td").first()).toHaveText("1.83 ms");
  await expect(panel.getByTestId("bridge-to-under")).toContainText("latest cause: driver callback late");
  await expect(panel.getByTestId("bridge-late")).toContainText("0.25 ms: 40");
  await expect(panel.getByTestId("bridge-chart").locator("canvas")).toHaveCount(1);

  // Another device: no bridge part.
  await page.goto(`/#devices/${PLAIN}`);
  await expect(page.getByTestId("lat-title")).toHaveText(`Latency · ${PLAIN}`);
  await expect(page.getByTestId("bridge-panel")).toBeHidden();
  expect(msgs).toEqual([]);
});
