import { test, expect, type ConsoleMessage } from "@playwright/test";

// The device global-setup.ts runs: dstack's own Dante device binary. It has
// no PTP clock of its own and answers no clock status, so its row says so.
const DEVICE = "dc-e2e-device";

test("the clock tab lists every device's clock status as the server read it", async ({ page }) => {
  const msgs: string[] = [];
  page.on("console", (m: ConsoleMessage) => {
    if (m.type() === "error" || m.type() === "warning") msgs.push(`${m.type()}: ${m.text()}`);
  });
  page.on("pageerror", (e) => msgs.push(`pageerror: ${e.message}`));

  await page.goto("/");
  await page.getByTestId("tab-clock").click();
  await expect(page).toHaveURL(/#clock$/);
  const row = page.locator(`[data-testid="clock-row"][data-name="${DEVICE}"]`);
  // Discovery, the first read and a clock ask that waits out its 1 s.
  await expect(row.getByTestId("clock-none")).toHaveText(/^no clock status: .*no reply/, { timeout: 30_000 });
  await expect(page.getByTestId("clock-summary")).not.toBeEmpty();

  // Each row shows what /api/devices says, whatever else is on the network.
  const api = await page.evaluate(() => fetch("/api/devices").then((r) => r.json()));
  const dev = api.devices.find((d: { name: string }) => d.name === DEVICE);
  expect(dev.clock).toBeNull();
  expect(dev.clock_error).toMatch(/no reply/);
  await expect(page.getByTestId("clock-row")).toHaveCount(api.devices.length);
  for (const d of api.devices.filter((d: { clock: unknown }) => d.clock)) {
    const r = page.locator(`[data-testid="clock-row"][data-name="${d.name}"]`);
    const role = d.clock.leader ? (d.clock.grandmaster ? "Leader (grandmaster)" : "Leader") : "Follower";
    await expect(r.getByTestId("clock-role")).toHaveText(role);
    await expect(r.getByTestId("clock-sync")).toHaveText(d.clock.sync);
  }

  // The view stays in the URL: a reload opens it again.
  await page.reload();
  await expect(page.getByTestId("clock-table")).toBeVisible();
  expect(msgs).toEqual([]);
});
