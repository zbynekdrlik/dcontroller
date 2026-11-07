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

  test.describe("seen from another time zone", () => {
    test.use({ timezoneId: "America/New_York" });

    test("the chart reads Slovak time in Slovak format", async ({ page }) => {
      await page.goto("/#latency");
      // 15 January 2026, 12:00 UTC: 13:00 in Slovakia.
      const t0 = 1768478400;
      const got = await page.evaluate((t0) => {
        const el = document.createElement("div");
        el.id = "tz-test";
        el.style.width = "800px";
        document.body.appendChild(el);
        (window as any).dcChart.latency(el, JSON.stringify({
          t: [t0, t0 + 3600],
          unit: "%",
          floor: 105,
          height: 280,
          thresholds: [],
          sources: [{ label: "src", color: "#60a5fa", values: [20, 30], late: [0, 0] }],
        }));
        const u = (el as any)._dc;
        u.setCursor({ left: u.valToPos(t0, "x"), top: 10 });
        const axis = u.axes[0].values;
        return {
          legend: el.querySelector(".u-legend .u-series .u-value")!.textContent,
          hours: axis(u, [t0], 0, 0, 3600),
          seconds: axis(u, [t0 + 5], 0, 0, 5),
          days: axis(u, [t0], 0, 0, 86400),
        };
      }, t0);
      expect(got.legend).toBe("15. 1. 2026 13:00:00");
      expect(got.hours).toEqual(["13:00"]);
      expect(got.seconds).toEqual(["13:00:05"]);
      expect(got.days).toEqual(["15. 1."]);
      expect(console_msgs).toEqual([]);
    });
  });

  test("late packets are marks on the one chart, per the sources showing", async ({ page }) => {
    await page.goto("/#latency");
    // The recorded device has none to rely on: the chart gets a known history.
    const box = page.locator("#late-test");
    await page.evaluate(() => {
      const el = document.createElement("div");
      el.id = "late-test";
      el.style.width = "800px";
      document.body.appendChild(el);
      const t = [1000, 1060, 1120, 1180];
      (window as any).dcChart.latency(el, JSON.stringify({
        t,
        unit: "%",
        floor: 105,
        height: 280,
        thresholds: [{ label: "latency", color: "#f87171", value: 100 }],
        sources: [
          { label: "src-a", color: "#60a5fa", values: [20, 30, 20, 25], late: [0, 3, 0, 2] },
          { label: "src-b", color: "#34d399", values: [10, 12, 11, 10], late: [0, 1, 0, 0] },
        ],
      }));
    });
    await expect(box.locator("canvas")).toHaveCount(1);
    // Two buckets have late packets, whichever source they came from.
    await expect(box).toHaveAttribute("data-late-marks", "2");
    // The legend names a source's late packets under the cursor.
    await page.evaluate(() => {
      const u = (document.getElementById("late-test") as any)._dc;
      u.setCursor({ left: u.valToPos(1060, "x"), top: 10 });
    });
    await expect(box.locator(".u-legend")).toContainText("3 late");
    await expect(box.locator(".u-legend")).toContainText("1 late");
    // A source switched off in the legend takes its marks along: the second
    // bucket stays for src-b, the fourth was src-a's alone.
    await box.locator(".u-legend .u-label", { hasText: "src-a" }).click();
    await expect(box).toHaveAttribute("data-late-marks", "1");
    expect(console_msgs).toEqual([]);
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
    // No channel lists: a device may carry hundreds.
    await expect(src).not.toContainText("Stage L");
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

    // One chart: uPlot draws it and names the source device and its latency in the legend.
    const margin = page.getByTestId("lat-chart");
    await expect(margin.locator("canvas")).toHaveCount(1);
    // The source's colour comes from the server's recorded order, the same in the chart and the table.
    const color = await margin.evaluate((el) => (el as any)._dcData.sources[0].color);
    expect(color).toMatch(/^hsl\(/);
    await expect(src.locator(".dot")).toHaveAttribute("data-color", color);
    await expect(margin.locator("canvas")).toBeVisible();
    await expect(margin.locator(".u-legend")).toContainText(DEVICE);
    await expect(margin.locator(".u-legend")).not.toContainText("Flow 1");
    // No late packets, no late marks.
    if (late === 0) await expect(margin).toHaveAttribute("data-late-marks", "0");
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
    await expect(page.getByTestId("lat-chart").locator("canvas")).toBeVisible();
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
