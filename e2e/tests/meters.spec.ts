import { test, expect, type ConsoleMessage } from "@playwright/test";

// The device global-setup.ts runs: dstack's own Dante device binary. It
// sends no meter frames yet (dstack#16), so the view says it waits for
// the first frame and the server renews the subscription.
const DEVICE = "dc-e2e-device";

test("the meters tab subscribes to the chosen device and shows what the server has", async ({ page }) => {
  const msgs: string[] = [];
  page.on("console", (m: ConsoleMessage) => {
    if (m.type() === "error" || m.type() === "warning") msgs.push(`${m.type()}: ${m.text()}`);
  });
  page.on("pageerror", (e) => msgs.push(`pageerror: ${e.message}`));

  await page.goto("/");
  await page.getByTestId("tab-meters").click();
  const pick = page.locator(`[data-testid="mtr-device"][data-name="${DEVICE}"]`);
  await expect(pick).toBeVisible({ timeout: 30_000 });
  await pick.click();
  await expect(page).toHaveURL(new RegExp(`#meters/${DEVICE}$`));
  await expect(pick).toHaveClass(/on/);
  await expect(page.getByTestId("mtr-head")).toHaveText(`${DEVICE} · waiting for the first meter frame`);

  // No frames within 5 s: the server says so and renews the subscription.
  await expect(page.getByTestId("mtr-error")).toHaveText(
    `no meter frames from ${DEVICE} for 5 s: subscription renewed`,
    { timeout: 15_000 },
  );
  const api = await page.evaluate((d) => fetch(`/api/meters/${d}`).then((r) => r.json()), DEVICE);
  expect(api).toMatchObject({ device: DEVICE, age_ms: null, frames: 0, tx: [], rx: [] });
  await expect(page.getByTestId("mtr-channel")).toHaveCount(0);

  // The view stays in the URL with its device: a reload opens it again.
  await page.reload();
  await expect(page.locator(`[data-testid="mtr-device"][data-name="${DEVICE}"]`)).toHaveClass(/on/, { timeout: 30_000 });
  await expect(page.getByTestId("mtr-head")).toContainText(DEVICE);
  expect(msgs).toEqual([]);
});
