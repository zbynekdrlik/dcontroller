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
  const obsIn = (v: any) => v.connections.find((c: any) => c.id === VIDEO.conn);
  await expect.poll(async () => obsIn(await video())?.lost_1h ?? 0, { timeout: 100_000, intervals: [2_000] }).toBeGreaterThan(0);
  const v = await video();
  expect(v.discovery_error).toBeNull();
  expect(v.nodes.map((n: any) => [n.name, n.kind, n.error])).toEqual([
    [VIDEO.cam, "cambox", null],
    [VIDEO.obs, "obs", null],
  ]);
  const c = obsIn(v);
  // The cambox's outputs are read too (one with a null sink).
  expect(v.connections.filter((x: any) => x.node === VIDEO.cam).map((x: any) => [x.direction, x.sink])).toEqual([
    ["out", "10.199.0.9:7002"],
    ["out", ""],
  ]);
  expect(c).toMatchObject({ id: VIDEO.conn, node: VIDEO.obs, source: VIDEO.cam, sink: VIDEO.obs, kind: "jxs", direction: "in" });
  expect(c.latency_ms).toBeGreaterThanOrEqual(40);
  expect(c.spark).toHaveLength(60);
  const h = await (await page.request.get(`/api/video/${encodeURIComponent(VIDEO.conn)}`)).json();
  expect(h.points.reduce((a: number, p: any) => a + p.lost, 0)).toBe(c.lost_1h);

  // The Map opens on the audio network: no video box, no video line.
  await page.goto("/#map");
  const map = page.getByTestId("map");
  await expect(page.getByTestId("map-layer-audio")).toHaveAttribute("aria-pressed", "true");
  await expect(map.locator('[data-testid="map-node"]').first()).toBeVisible({ timeout: 40_000 });
  await expect(map.locator(`[data-testid="map-node"][data-node="${VIDEO.cam}"]`)).toHaveCount(0);
  await expect(map.locator('[data-testid="map-edge"][data-edge^="#map/video/"]')).toHaveCount(0);
  // The health tile still counts the video connection: it speaks of the whole rig.
  await expect(page.getByTestId("tile-connections").locator(".tile-value")).toHaveText(/\/ 2$/);

  // The Video switch: its two boxes and a dashed line, red; no Dante line.
  await page.getByTestId("map-layer-video").click();
  await expect(page.getByTestId("map-layer-video")).toHaveAttribute("aria-pressed", "true");
  await expect(map.locator(`[data-testid="map-node"][data-node="${VIDEO.cam}"]`)).toHaveClass(/\bvideo\b/, { timeout: 30_000 });
  await expect(map.locator('[data-testid="map-edge"][data-edge^="#map/link/"]')).toHaveCount(0);
  await expect(page.locator(".legend")).toContainText("frames lost in the last");
  const line = map.locator(`[data-testid="map-edge"][data-edge="#map/video/${encodeURIComponent(VIDEO.conn).replace(/[!'()*]/g, (x) => "%" + x.charCodeAt(0).toString(16).toUpperCase())}"]`);
  await expect(line).toHaveClass(/\bvideo\b/);
  await expect(line).toHaveAttribute("data-state", "bad");
  await expect(line.locator(".lbl text")).toContainText(" lost");
  // A line is the receiver's view: the cambox's outputs draw no line and no
  // box of their own (none named after an address or after nothing).
  await expect(map.locator('[data-testid="map-edge"][data-edge^="#map/video/"]')).toHaveCount(1);
  await expect(map.locator('[data-testid="map-node"][data-node="10.199.0.9:7002"]')).toHaveCount(0);
  await expect(map.locator('[data-testid="map-node"][data-node=""]')).toHaveCount(0);

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
  // The choice survives a reload, and its link opens the video network.
  await page.reload();
  await expect(page.getByTestId("map-title")).toHaveText(`${VIDEO.cam} → ${VIDEO.obs}`, { timeout: 30_000 });
  await expect(page.getByTestId("map-layer-video")).toHaveAttribute("aria-pressed", "true");

  // The Links table follows the switch: the video connections only.
  await page.getByTestId("map-view-links").click();
  const row = page.locator(`[data-testid="map-links"] [data-edge^="#map/video/"]`);
  await expect(row).toHaveCount(1);
  await expect(row).toContainText(`${VIDEO.cam} → ${VIDEO.obs}`);
  await expect(row).toContainText("frames lost in the last hour");
  await expect(page.locator('[data-testid="map-links"] [data-edge^="#map/link/"]')).toHaveCount(0);
  // Back to audio: the video connection shown lets go, the Dante ones are back.
  await page.getByTestId("map-layer-audio").click();
  await expect(page).toHaveURL(/#map$/);
  await expect(row).toHaveCount(0);
  await expect(page.locator('[data-testid="map-links"] [data-edge^="#map/link/"]').first()).toBeVisible();
  expect(msgs).toEqual([]);
});
