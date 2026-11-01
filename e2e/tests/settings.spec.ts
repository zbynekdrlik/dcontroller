import { test, expect, type ConsoleMessage } from "@playwright/test";

// Setting writes go only to dstack's own device (global-setup.ts), never to
// a site device. That device takes no setting write yet (dstack#16), so a
// confirmed write ends "not confirmed": the test covers the dialog, the
// confirm step, the result line and the log entry.
const DEVICE = "dc-e2e-device";

test("a setting change is offered from what the device reports, confirmed, sent and logged", async ({ page }) => {
  const msgs: string[] = [];
  page.on("console", (m: ConsoleMessage) => {
    if (m.type() === "error" || m.type() === "warning") msgs.push(`${m.type()}: ${m.text()}`);
  });
  page.on("pageerror", (e) => msgs.push(`pageerror: ${e.message}`));

  await page.goto("/#devices");
  const card = page.locator(`[data-testid="device"][data-name="${DEVICE}"]`);
  await expect(card.getByTestId("device-counts")).toHaveText("4 TX · 4 RX", { timeout: 30_000 });
  await card.getByTestId("device-name").click();
  const api = await page.evaluate(() => fetch("/api/devices").then((r) => r.json()));
  const dev = api.devices.find((d: { name: string }) => d.name === DEVICE);
  console.log(JSON.stringify({ settings: dev.settings, info: dev.info }));

  // The encoding: 24 bit in its config, so 16/24/32 are offered.
  await card.getByTestId("change-encoding").click();
  const dialog = page.getByTestId("setting-dialog");
  await expect(dialog.getByTestId("setting-title")).toHaveText(`Change the encoding of ${DEVICE} (now 24 bit)`);
  await expect(dialog.getByTestId("setting-warning")).toContainText("Receivers that do not take this encoding");
  // The current value is chosen: nothing to apply yet.
  await expect(dialog.getByTestId("setting-apply")).toBeDisabled();
  await dialog.getByTestId("setting-value").selectOption("32");
  await expect(dialog.getByTestId("setting-apply")).toHaveText("Set 32 bit");

  // Cancel sends nothing and logs nothing.
  const before = await page.evaluate(() => fetch("/api/changes").then((r) => r.json()));
  await dialog.getByTestId("setting-cancel").click();
  await expect(dialog).toHaveCount(0);
  const still = await page.evaluate(() => fetch("/api/changes").then((r) => r.json()));
  expect(still.length).toBe(before.length);

  // Apply sends it; the device does not take it, and the page says so.
  await card.getByTestId("change-encoding").click();
  await dialog.getByTestId("setting-value").selectOption("32");
  await dialog.getByTestId("setting-apply").click();
  await expect(page.getByTestId("change-result")).toHaveText(
    /^Not changed: not confirmed: the device's channels read 24 bit after 5 s$/,
    { timeout: 20_000 },
  );
  await expect(dialog).toHaveCount(0);

  // The log has it: who, what, before, asked, after, result.
  const after = await page.evaluate(() => fetch("/api/changes").then((r) => r.json()));
  expect(after.length).toBe(before.length + 1);
  expect(after[0]).toMatchObject({
    user: "local",
    kind: "setting",
    what: "encoding",
    rx_device: DEVICE,
    name_before: "24 bit",
    name_asked: "32 bit",
    name_after: "24 bit",
    ok: false,
  });
  await page.getByTestId("tab-changes").click();
  const row = page.getByTestId("change-row").first();
  await expect(row).toContainText(`encoding @ ${DEVICE}`);
  await expect(row).toContainText("32 bit");
  await expect(row).toHaveClass(/err/);

  // The device still shows 24 bit.
  await page.getByTestId("tab-devices").click();
  await expect(card.getByTestId("device-settings")).toContainText("24 bit");
  expect(msgs).toEqual([]);
});
