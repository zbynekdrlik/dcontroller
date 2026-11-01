import { test, expect, type ConsoleMessage } from "@playwright/test";

// The device global-setup.ts runs receives its own TX (Stage L <- Mix L),
// so it has one RX flow, and its heartbeat carries that flow's latency.
const DEVICE = "dc-e2e-device";

test.describe("latency", () => {
  let console_msgs: string[];

  test.beforeEach(async ({ page }) => {
    console_msgs = [];
    page.on("console", (m: ConsoleMessage) => {
      if (m.type() === "error" || m.type() === "warning") console_msgs.push(`${m.type()}: ${m.text()}`);
    });
    page.on("pageerror", (e) => console_msgs.push(`pageerror: ${e.message}`));
  });

  test("shows a receiver's flow with its latency, histogram and heartbeats", async ({ page }) => {
    await page.goto("/#latency");
    await expect(page.getByTestId("tab-latency")).toHaveClass(/\bon\b/);
    const pick = page.locator(`[data-testid="lat-receiver"][data-name="${DEVICE}"]`);
    await expect(pick).toBeVisible({ timeout: 30_000 });
    await pick.click();
    await expect(page).toHaveURL(new RegExp(`#latency/${DEVICE}$`));

    const flow = page.getByTestId("lat-flow").first();
    await expect(flow).toBeVisible({ timeout: 20_000 });
    await expect(flow).toContainText(`from ${DEVICE}`);
    await expect(flow).toContainText("Stage L");
    await expect(flow.getByTestId("lat-set")).toHaveText("10.00 ms");
    await expect(flow.getByTestId("lat-hist").locator(".bin")).toHaveCount(11);
    // Heartbeats come once a second: the count grows while the page watches.
    await expect(page.getByTestId("lat-head")).toContainText(/ [1-9]\d* heartbeats/, { timeout: 20_000 });

    // Once peaks are in, their bins draw bars and an empty bin draws none (0 px).
    const heights = () =>
      flow.locator(".bin .hbar").evaluateAll((bars) => bars.map((b) => b.getBoundingClientRect().height));
    await expect.poll(async () => (await heights()).some((h) => h > 0), { timeout: 20_000 }).toBe(true);
    const hs = await heights();
    expect(hs.filter((h) => h === 0).length).toBeGreaterThan(0);

    // The page shows what the server heard from the device.
    const r = await page.evaluate((d) => fetch(`/api/latency/${d}`).then((x) => x.json()), DEVICE);
    expect(r.beats).toBeGreaterThan(0);
    expect(r.flows.length).toBe(1);
    expect(r.flows[0].latency_us).toBe(10000);
    expect(r.flows[0].channels).toEqual(["Stage L"]);
    expect(r.flows[0].peaks_us.length).toBeGreaterThan(0);

    // The choice survives a reload (kept in the URL).
    await page.reload();
    await expect(page.getByTestId("lat-flow").first()).toBeVisible({ timeout: 20_000 });
    expect(console_msgs).toEqual([]);
  });

  test("an unknown receiver says why", async ({ page }) => {
    const r = await page.request.get("/api/latency/no-such-device");
    expect(r.ok()).toBe(true);
    const body = await r.json();
    expect(body.error).toContain("not listed");
    expect(body.flows).toEqual([]);
  });
});
