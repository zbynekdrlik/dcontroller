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

  test("is the start view and shows the subscription at device and channel level", async ({ page }) => {
    await page.goto("/");
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

  test("tabs switch between routing and devices and keep the view in the URL", async ({ page }) => {
    await page.goto("/");
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
