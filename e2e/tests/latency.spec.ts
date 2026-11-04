import { test, expect, type ConsoleMessage } from "@playwright/test";

// The device global-setup.ts runs receives its own TX (Stage L <- Mix L),
// so it has one RX flow, and its heartbeat carries that flow's lateness.
// The server records it all the time; the page reads the history.
const DEVICE = "dc-e2e-device";

test.describe("latency history", () => {
  let console_msgs: string[];

  test.beforeEach(async ({ page }) => {
    console_msgs = [];
    page.on("console", (m: ConsoleMessage) => {
      if (m.type() === "error" || m.type() === "warning") console_msgs.push(`${m.type()}: ${m.text()}`);
    });
    page.on("pageerror", (e) => console_msgs.push(`pageerror: ${e.message}`));
  });

  test("charts a receiver device to device: late packets as the verdict, the margin per source, ranges and zoom", async ({ page }) => {
    await page.goto("/#latency");
    await expect(page.getByTestId("tab-latency")).toHaveClass(/\bon\b/);
    const pick = page.locator(`[data-testid="lat-receiver"][data-name="${DEVICE}"]`);
    await expect(pick).toBeVisible({ timeout: 30_000 });
    await pick.click();
    await expect(page).toHaveURL(new RegExp(`#latency/${DEVICE}$`));
    await expect(pick).toHaveClass(/\bon\b/);

    // The recorder's first samples (5 s listens) show up as one source device:
    // the device receives its own TX, so it is device to device with itself.
    const src = page.getByTestId("lat-source").first();
    await expect(src).toBeVisible({ timeout: 30_000 });
    await expect(page.getByTestId("lat-source")).toHaveCount(1);
    await expect(src).toHaveAttribute("data-name", DEVICE);
    await expect(src).toContainText("1 flow");
    await expect(src).toContainText("Stage L");
    await expect(src.getByTestId("lat-source-latency")).toHaveText("10.00 ms");
    await expect(src.locator(".hist .hb")).toHaveCount(11);
    await expect(page.getByTestId("lat-live")).toBeVisible();

    // The page shows what the server recorded: late packets are the verdict.
    const h = await page.evaluate((d) => fetch(`/api/history/${d}`).then((x) => x.json()), DEVICE);
    expect(h.flows.length).toBe(1);
    expect(h.flows[0].latency_us).toBe(10000);
    expect(h.flows[0].channels).toEqual(["Stage L"]);
    expect(h.flows[0].max_us.some((v: number | null) => v !== null)).toBe(true);
    await expect(src.getByTestId("lat-source-peak")).toHaveText(/^\d+\.\d\d ms \(\d+ %\)$/);
    const late = h.flows[0].late_total;
    await expect(src.getByTestId("lat-source-late")).toHaveText(String(late));
    await expect(page.getByTestId("lat-verdict")).toContainText(late === 0 ? "No late packets" : `${late} late packets`);

    // The chart: uPlot draws the margin chart and names the source device and its latency in the legend.
    const margin = page.getByTestId("lat-chart-margin");
    await expect(margin.locator("canvas")).toBeVisible();
    await expect(margin.locator(".u-legend")).toContainText(DEVICE);
    await expect(margin.locator(".u-legend")).not.toContainText("Flow 1");
    // The late chart shows only when there are late packets.
    await expect(page.getByTestId("lat-chart-late")).toHaveCount(late === 0 ? 0 : 1);
    // The default scale is each flow's share of its own latency: one 100 % line.
    await expect(page.getByTestId("lat-scale-share")).toHaveClass(/\bon\b/);
    await expect(margin.locator(".u-legend")).toContainText("latency");
    await expect(margin.locator(".u-legend")).not.toContainText("latency 10.00 ms");
    // In ms, the latency line is named with its value.
    await page.getByTestId("lat-scale-ms").click();
    await expect(page.getByTestId("lat-scale-ms")).toHaveClass(/\bon\b/);
    await expect(margin.locator(".u-legend")).toContainText("latency 10.00 ms");
    await page.getByTestId("lat-scale-share").click();

    // Ranges: 5 min reads seconds, 6 h reads minutes.
    await page.locator('[data-testid="lat-range"][data-range="300"]').click();
    await expect(page.locator('[data-testid="lat-range"][data-range="300"]')).toHaveClass(/\bon\b/);
    await expect(page.getByTestId("lat-head")).toContainText("one point per 1 s");
    await page.locator('[data-testid="lat-range"][data-range="21600"]').click();
    await expect(page.getByTestId("lat-head")).toContainText("one point per 1 min");
    await page.locator('[data-testid="lat-range"][data-range="300"]').click();
    await expect(page.getByTestId("lat-head")).toContainText("one point per 1 s");

    // Zoom: a drag across the chart zooms in and stops following live; the chip zooms out.
    const over = margin.locator(".u-over");
    const box = (await over.boundingBox())!;
    await page.mouse.move(box.x + box.width * 0.5, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width * 0.95, box.y + box.height / 2, { steps: 8 });
    await page.mouse.up();
    await expect(page.getByTestId("lat-unzoom")).toBeVisible();
    await expect(page.getByTestId("lat-live")).toHaveCount(0);
    await page.getByTestId("lat-unzoom").click();
    await expect(page.getByTestId("lat-unzoom")).toHaveCount(0);
    await expect(page.getByTestId("lat-live")).toBeVisible();

    // The choice survives a reload (kept in the URL), and the history is still there.
    await page.reload();
    await expect(page.getByTestId("lat-source").first()).toBeVisible({ timeout: 20_000 });
    await expect(page.getByTestId("lat-chart-margin").locator("canvas")).toBeVisible();
    expect(console_msgs).toEqual([]);
  });

  test("an unknown receiver has an empty history", async ({ page }) => {
    // The last five minutes (a range in the future is clamped to an hour ahead).
    const now = Math.floor(Date.now() / 1000);
    const r = await page.request.get(`/api/history/no-such-device?from=${now - 300}&to=${now}`);
    expect(r.ok()).toBe(true);
    const body = await r.json();
    expect(body.flows).toEqual([]);
    expect(body.t.length).toBe(301);
    expect(body.step_s).toBe(1);
  });
});
