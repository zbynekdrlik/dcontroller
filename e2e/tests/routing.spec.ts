import { test, expect, type ConsoleMessage } from "@playwright/test";

// The device global-setup.ts runs: RX "Stage L" takes its own TX "Mix L".
const DEVICE = "dc-e2e-device";

test.describe("routing matrix", () => {
  let console_msgs: string[];

  test.beforeEach(async ({ page }) => {
    console_msgs = [];
    page.on("console", (m: ConsoleMessage) => {
      if (m.type() === "error" || m.type() === "warning") console_msgs.push(`${m.type()}: ${m.text()}`);
    });
    page.on("pageerror", (e) => console_msgs.push(`pageerror: ${e.message}`));
  });

  test("shows the subscription at device and channel level", async ({ page }) => {
    await page.goto("/#routing");
    await expect(page.getByTestId("tab-routing")).toHaveClass(/\bon\b/);
    const matrix = page.getByTestId("matrix");
    const devCell = matrix.locator(`[data-testid="cell"][data-rx="${DEVICE}"][data-tx="${DEVICE}"]`);
    // Discovery plus the first ARC read.
    await expect(devCell).toHaveText("1", { timeout: 30_000 });
    await expect(devCell).toHaveClass(/\bcell\b/);
    await expect(devCell).toHaveClass(/\b(ok|warn)\b/);

    // Open the receiver: one row per RX channel; the collapsed TX device names the source channel.
    await matrix.locator(`[data-testid="rx-device"][data-name="${DEVICE}"]`).click();
    const stageL = matrix.locator(`[data-testid="cell"][data-rx="${DEVICE}/Stage L"][data-tx="${DEVICE}"]`);
    await expect(stageL).toHaveText("Mix L");
    await expect(matrix.locator(`[data-testid="cell"][data-rx="${DEVICE}/Stage R"][data-tx="${DEVICE}"]`)).toHaveText("");

    // Open the transmitter: one column per TX channel; the dot sits at Stage L × Mix L only.
    await matrix.locator(`[data-testid="tx-device"][data-name="${DEVICE}"]`).click();
    const hit = matrix.locator(`[data-testid="cell"][data-rx="${DEVICE}/Stage L"][data-tx="${DEVICE}/Mix L"]`);
    await expect(hit).toHaveText("●");
    await expect(matrix.locator(`[data-testid="cell"][data-rx="${DEVICE}/Stage L"][data-tx="${DEVICE}/Mix R"]`)).toHaveText("");
    const dots = matrix.locator('[data-testid="cell"]', { hasText: "●" });
    await expect(dots).toHaveCount(1);

    // Opened devices stay open across two refreshes (3 s each).
    await page.waitForTimeout(7_000);
    await expect(hit).toHaveText("●");

    // Closing the receiver hides its channel rows again.
    await matrix.locator(`[data-testid="rx-device"][data-name="${DEVICE}"]`).click();
    await expect(hit).toHaveCount(0);
    expect(console_msgs).toEqual([]);
  });

  test("the hovered cross-point lights its row and column up to both names", async ({ page }) => {
    await page.goto("/#routing");
    const matrix = page.getByTestId("matrix");
    await expect(matrix.locator(`[data-testid="cell"][data-rx="${DEVICE}"][data-tx="${DEVICE}"]`)).toHaveText("1", {
      timeout: 30_000,
    });
    await matrix.locator(`[data-testid="rx-device"][data-name="${DEVICE}"]`).click();
    await matrix.locator(`[data-testid="tx-device"][data-name="${DEVICE}"]`).click();
    const lit = (loc: ReturnType<typeof matrix.locator>) => loc.evaluate((e) => getComputedStyle(e).backgroundImage !== "none");
    const rxHead = matrix.locator(`[data-testid="rx-channel-head"][data-name="${DEVICE}/Stage R"]`);
    const txHead = matrix.locator(`[data-testid="tx-channel-head"][data-name="${DEVICE}/Talkback"]`);
    const otherRx = matrix.locator(`[data-testid="rx-channel-head"][data-name="${DEVICE}/Stage L"]`);
    const rowCell = matrix.locator(`[data-testid="cell"][data-rx="${DEVICE}/Stage R"][data-tx="${DEVICE}/Mix L"]`);
    const colCell = matrix.locator(`[data-testid="cell"][data-rx="${DEVICE}/Spare 3"][data-tx="${DEVICE}/Talkback"]`);
    const away = matrix.locator(`[data-testid="cell"][data-rx="${DEVICE}/Spare 3"][data-tx="${DEVICE}/Mix L"]`);
    expect(await lit(rxHead)).toBe(false);

    await matrix.locator(`[data-testid="cell"][data-rx="${DEVICE}/Stage R"][data-tx="${DEVICE}/Talkback"]`).hover();
    await expect.poll(() => lit(rxHead)).toBe(true);
    expect(await lit(txHead)).toBe(true);
    expect(await lit(rowCell)).toBe(true);
    expect(await lit(colCell)).toBe(true);
    expect(await lit(otherRx)).toBe(false);
    expect(await lit(away)).toBe(false);
    // The device headers of that row and column light too.
    expect(await lit(matrix.locator(`[data-testid="rx-device"][data-name="${DEVICE}"]`))).toBe(true);
    expect(await lit(matrix.locator(`[data-testid="tx-device"][data-name="${DEVICE}"]`))).toBe(true);

    // It stays on the hovered cell across a refresh, and goes when the mouse leaves.
    await page.waitForResponse((r) => r.url().endsWith("/api/devices"));
    expect(await lit(rxHead)).toBe(true);
    await page.getByTestId("status").hover();
    await expect.poll(() => lit(rxHead)).toBe(false);
    expect(console_msgs).toEqual([]);
  });

  test("headers carry live levels: the loudest channel folded, each channel open", async ({ page }) => {
    // dstack's device answers a meter subscription with silence (-126 dBFS).
    await page.goto("/#routing");
    const matrix = page.getByTestId("matrix");
    const txDev = matrix.locator(`[data-testid="tx-device"][data-name="${DEVICE}"]`);
    const rxDev = matrix.locator(`[data-testid="rx-device"][data-name="${DEVICE}"]`);
    const txBar = txDev.getByTestId("lvl");
    const rxBar = rxDev.getByTestId("lvl");
    // Folded: one bar each, naming the loudest channel and its level.
    await expect(txBar).toHaveAttribute("title", /^(Mix L|Mix R|Talkback|Click) < -60 dB$/, { timeout: 30_000 });
    await expect(rxBar).toHaveAttribute("title", /^(Stage L|Stage R|Spare 3|Spare 4) < -60 dB$/);
    await expect(txBar).toHaveAttribute("data-dir", "tx");
    await expect(rxBar).toHaveAttribute("data-dir", "rx");
    // Silence fills nothing.
    await expect(txBar.locator("span")).toHaveAttribute("style", "width: 0.0%");

    // The page reads what the server has for the device.
    const lv = await page.evaluate((d) => fetch(`/api/levels?devices=${d}`).then((r) => r.json()), DEVICE);
    expect(lv.map((m: { device: string }) => m.device)).toEqual([DEVICE]);
    expect(lv[0].tx.map((c: { label: string }) => c.label)).toEqual(["Mix L", "Mix R", "Talkback", "Click"]);

    // Unfolded: the device's bar moves to a bar per channel.
    await txDev.click();
    await rxDev.click();
    await expect(txBar).toHaveCount(0);
    await expect(rxBar).toHaveCount(0);
    const mixL = matrix.locator(`[data-testid="tx-channel-head"][data-name="${DEVICE}/Mix L"]`).getByTestId("lvl");
    await expect(mixL).toHaveAttribute("title", "Mix L < -60 dB");
    await expect(mixL).toHaveClass(/\bv\b/);
    await expect(mixL.locator("span")).toHaveAttribute("style", "height: 0.0%");
    await expect(matrix.locator('[data-testid="tx-channel-head"] [data-testid="lvl"]')).toHaveCount(4);
    const stageR = matrix.locator(`[data-testid="rx-channel-head"][data-name="${DEVICE}/Stage R"]`).getByTestId("lvl");
    await expect(stageR).toHaveAttribute("title", "Stage R < -60 dB");
    await expect(matrix.locator('[data-testid="rx-channel-head"] [data-testid="lvl"]')).toHaveCount(4);
    expect(console_msgs).toEqual([]);
  });

  test("an unfolded matrix keeps its scroll position across refreshes", async ({ page }) => {
    // A small window, so the unfolded matrix scrolls inside its box.
    await page.setViewportSize({ width: 480, height: 330 });
    await page.goto("/#routing");
    const matrix = page.getByTestId("matrix");
    await expect(matrix.locator(`[data-testid="cell"][data-rx="${DEVICE}"][data-tx="${DEVICE}"]`)).toHaveText("1", {
      timeout: 30_000,
    });
    await matrix.locator(`[data-testid="rx-device"][data-name="${DEVICE}"]`).click();
    await matrix.locator(`[data-testid="tx-device"][data-name="${DEVICE}"]`).click();
    const wrap = page.locator(".matrix-wrap");
    await expect(matrix.locator('[data-testid="cell"]', { hasText: "●" })).toHaveCount(1);
    const room = await wrap.evaluate((e) => ({ y: e.scrollHeight - e.clientHeight, x: e.scrollWidth - e.clientWidth }));
    expect(room.y).toBeGreaterThan(20);
    await wrap.evaluate((e) => {
      e.scrollTop = 20;
      e.scrollLeft = Math.min(30, e.scrollWidth - e.clientWidth);
    });
    const at = await wrap.evaluate((e) => ({ y: e.scrollTop, x: e.scrollLeft }));
    // Two device refreshes (3 s each) later, the box shows the same place.
    for (let i = 0; i < 2; i++) await page.waitForResponse((r) => r.url().endsWith("/api/devices"));
    await page.waitForTimeout(300);
    expect(await page.locator(".matrix-wrap").evaluate((e) => ({ y: e.scrollTop, x: e.scrollLeft }))).toEqual(at);
    expect(console_msgs).toEqual([]);
  });

  test("tabs switch between routing and devices and keep the view in the URL", async ({ page }) => {
    await page.goto("/#routing");
    await page.getByTestId("tab-devices").click();
    await expect(page).toHaveURL(/#devices$/);
    await expect(page.locator(`[data-testid="device"][data-name="${DEVICE}"]`)).toBeVisible({ timeout: 30_000 });
    await page.reload();
    await expect(page.getByTestId("tab-devices")).toHaveClass(/\bon\b/);
    await page.getByTestId("tab-routing").click();
    await expect(page.getByTestId("matrix")).toBeVisible();
    expect(console_msgs).toEqual([]);
  });
});
