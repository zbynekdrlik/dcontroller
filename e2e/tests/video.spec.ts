import { test, expect, type ConsoleMessage } from "@playwright/test";
import { VIDEO } from "../env";

// The video network on the Map (#124 phase 1): camera-box's node list and
// nodes, stood in by global-setup.ts after its draft contract
// (camera-box#1540). The OBS node's input from the cambox loses a frame per
// read after its third: its line turns red and its chart shows the losses.
test("a video connection is a line of its own on the Map, red with frames lost, its chart a click away", async ({ page }) => {
  test.setTimeout(150_000);
  const msgs: string[] = [];
  page.on("console", (m: ConsoleMessage) => {
    if (m.type() === "error" || m.type() === "warning") msgs.push(`${m.type()}: ${m.text()}`);
  });
  page.on("pageerror", (e) => msgs.push(`pageerror: ${e.message}`));

  // The server reads the list and the nodes and keeps what each read added.
  const video = async () => (await page.request.get("/api/video")).json();
  await expect.poll(async () => (await video()).connections[0]?.lost_1h ?? 0, { timeout: 100_000, intervals: [2_000] }).toBeGreaterThan(0);
  const v = await video();
  expect(v.discovery_error).toBeNull();
  expect(v.nodes.map((n: any) => [n.name, n.kind, n.error])).toEqual([
    [VIDEO.cam, "cambox", null],
    [VIDEO.obs, "obs", null],
  ]);
  const c = v.connections[0];
  expect(c).toMatchObject({ id: VIDEO.conn, node: VIDEO.obs, source: VIDEO.cam, sink: VIDEO.obs, kind: "jxs", direction: "in" });
  expect(c.latency_ms).toBeGreaterThanOrEqual(40);
  expect(c.spark).toHaveLength(60);
  const h = await (await page.request.get(`/api/video/${encodeURIComponent(VIDEO.conn)}`)).json();
  expect(h.points.reduce((a: number, p: any) => a + p.lost, 0)).toBe(c.lost_1h);

  // On the Map: its two boxes and a dashed line, red.
  await page.goto("/#map");
  const map = page.getByTestId("map");
  await expect(map.locator(`[data-testid="map-node"][data-node="${VIDEO.cam}"]`)).toHaveClass(/\bvideo\b/, { timeout: 30_000 });
  const line = map.locator(`[data-testid="map-edge"][data-edge="#map/video/${encodeURIComponent(VIDEO.conn).replace(/[!'()*]/g, (x) => "%" + x.charCodeAt(0).toString(16).toUpperCase())}"]`);
  await expect(line).toHaveClass(/\bvideo\b/);
  await expect(line).toHaveAttribute("data-state", "bad");
  await expect(line.locator(".lbl text")).toContainText(" lost");

  // A click: its detail and its chart.
  await line.locator(".lbl").click({ force: true });
  await expect(page.getByTestId("map-title")).toHaveText(`${VIDEO.cam} → ${VIDEO.obs}`);
  await expect(page.getByTestId("map-state")).toHaveText("frames lost in the last hour");
  await expect(page.getByTestId("map-what")).toContainText(`video (jxs) into JXS cam-e2e src · reported by ${VIDEO.obs}`);
  await expect(page.getByTestId("map-video-lost").locator(".v")).toHaveText(/^[1-9][\d ]*$/, { timeout: 20_000 });
  await expect(page.getByTestId("map-video-latency").locator(".v")).toHaveText(/^4[0-2]\.0 ms$/);
  const chart = page.getByTestId("map-chart");
  await expect(chart.locator("canvas")).toHaveCount(1, { timeout: 20_000 });
  await expect(chart.locator(".u-legend")).toContainText(`${VIDEO.cam} → ${VIDEO.obs}`);
  await expect(chart).toHaveAttribute("data-late-marks", /^[1-9]\d*$/);
  // The ranges read it again.
  await page.locator('[data-testid="map-range"][data-range="300"]').click();
  await expect(chart.locator("canvas")).toBeVisible();
  // The choice survives a reload.
  await page.reload();
  await expect(page.getByTestId("map-title")).toHaveText(`${VIDEO.cam} → ${VIDEO.obs}`, { timeout: 30_000 });

  // In the Links table with the Dante connections, worst first.
  await page.getByTestId("map-view-links").click();
  const row = page.locator(`[data-testid="map-links"] [data-edge^="#map/video/"]`);
  await expect(row).toHaveCount(1);
  await expect(row).toContainText(`${VIDEO.cam} → ${VIDEO.obs}`);
  await expect(row).toContainText("frames lost in the last hour");
  expect(msgs).toEqual([]);
});
