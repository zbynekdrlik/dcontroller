import { test, expect, type ConsoleMessage } from "@playwright/test";

// The device global-setup.ts runs: dstack's own Dante device binary. It
// answers a meter subscription with a frame a second (silence: -126 dBFS).
const DEVICE = "dc-e2e-device";

test("the meters tab subscribes to the chosen device and shows its channel levels", async ({ page }) => {
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

  // Frames come: every TX and RX channel with its label and level.
  await expect(page.getByTestId("mtr-head")).toHaveText(new RegExp(`^${DEVICE} · \\d+ frames · last \\d+ ms ago$`), {
    timeout: 15_000,
  });
  const tx = page.getByTestId("mtr-tx").getByTestId("mtr-channel");
  const rx = page.getByTestId("mtr-rx").getByTestId("mtr-channel");
  await expect(tx).toHaveCount(4);
  await expect(rx).toHaveCount(4);
  await expect(tx.locator(".mlabel")).toHaveText(["Mix L", "Mix R", "Talkback", "Click"]);
  await expect(rx.locator(".mlabel")).toHaveText(["Stage L", "Stage R", "Spare 3", "Spare 4"]);
  // Silence reads below the bar's floor, the bar empty.
  await expect(tx.first().getByTestId("mtr-value")).toHaveText("< -60 dB");
  await expect(page.getByTestId("mtr-error")).toHaveCount(0);

  // The page shows what the server has, and frames keep coming.
  const a = await page.evaluate((d) => fetch(`/api/meters/${d}`).then((r) => r.json()), DEVICE);
  expect(a.tx.map((c: { label: string }) => c.label)).toEqual(["Mix L", "Mix R", "Talkback", "Click"]);
  expect(a.tx[0].dbfs).toBe(-126);
  await page.waitForTimeout(2500);
  const b = await page.evaluate((d) => fetch(`/api/meters/${d}`).then((r) => r.json()), DEVICE);
  expect(b.frames).toBeGreaterThan(a.frames);

  // The view stays in the URL with its device: a reload opens it again.
  await page.reload();
  await expect(page.locator(`[data-testid="mtr-device"][data-name="${DEVICE}"]`)).toHaveClass(/on/, { timeout: 30_000 });
  await expect(page.getByTestId("mtr-tx").getByTestId("mtr-channel")).toHaveCount(4, { timeout: 15_000 });
  expect(msgs).toEqual([]);
});
