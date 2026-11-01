import { test, expect, type ConsoleMessage, type Page } from "@playwright/test";

// The device global-setup.ts runs (dstack's own Dante device): ours to
// change. Every test puts the names back.
const DEVICE = "dc-e2e-device";

async function openMatrix(page: Page, device = DEVICE) {
  await page.goto("/");
  const matrix = page.getByTestId("matrix");
  await expect(matrix.locator(`[data-testid="rx-device"][data-name="${device}"]`)).toBeVisible({ timeout: 30_000 });
  await expect(matrix.locator(`[data-testid="cell"][data-rx="${device}"][data-tx="${device}"]`)).not.toHaveText("", {
    timeout: 30_000,
  });
  await matrix.locator(`[data-testid="rx-device"][data-name="${device}"]`).click();
  await matrix.locator(`[data-testid="tx-device"][data-name="${device}"]`).click();
  return matrix;
}

async function labels(page: Page, device = DEVICE) {
  const api = await page.evaluate(() => fetch("/api/devices").then((r) => r.json()));
  const dev = api.devices.find((d: { name: string }) => d.name === device);
  return {
    tx: dev?.channels?.tx.map((t: { label: string }) => t.label),
    rx: dev?.channels?.rx.map((r: { label: string }) => r.label),
  };
}

async function rename(page: Page, name: string) {
  await page.getByTestId("rename-input").fill(name);
  await page.getByTestId("rename-confirm").click();
  await expect(page.getByTestId("change-result")).toHaveText(/^Done: done, read back from the device/);
  await expect(page.getByTestId("rename-dialog")).toHaveCount(0);
}

test.describe("renames", () => {
  let console_msgs: string[];

  test.beforeEach(async ({ page }) => {
    console_msgs = [];
    page.on("console", (m: ConsoleMessage) => {
      if (m.type() === "error" || m.type() === "warning") console_msgs.push(`${m.type()}: ${m.text()}`);
    });
    page.on("pageerror", (e) => console_msgs.push(`pageerror: ${e.message}`));
  });

  test("an RX channel is renamed from its row, read back, logged, and renamed back", async ({ page }) => {
    const matrix = await openMatrix(page);
    await matrix.locator(`[data-testid="rx-channel-head"][data-name="${DEVICE}/Spare 3"]`).click();
    await expect(page.getByTestId("rename-title")).toHaveText(`Rename RX channel 3 “Spare 3” of ${DEVICE}`);
    await expect(page.getByTestId("rename-input")).toHaveValue("Spare 3");
    await expect(page.getByTestId("rename-confirm")).toBeDisabled(); // same name: nothing to send
    await rename(page, "Guest Mic");
    await expect(matrix.locator(`[data-testid="rx-channel-head"][data-name="${DEVICE}/Guest Mic"]`)).toBeVisible();
    expect((await labels(page)).rx[2]).toBe("Guest Mic");

    await page.getByTestId("tab-changes").click();
    const row = page.getByTestId("change-row").first();
    await expect(row).toContainText(`rename RX 3 @ ${DEVICE}`);
    await expect(row).toContainText("Spare 3");
    await expect(row).toContainText("Guest Mic");

    await page.getByTestId("tab-routing").click();
    await matrix.locator(`[data-testid="rx-channel-head"][data-name="${DEVICE}/Guest Mic"]`).click();
    await rename(page, "Spare 3");
    expect((await labels(page)).rx[2]).toBe("Spare 3");
    expect(console_msgs).toEqual([]);
  });

  test("a TX channel rename warns about the subscriptions using it; cancel sends nothing", async ({ page }) => {
    const matrix = await openMatrix(page);
    await matrix.locator(`[data-testid="tx-channel-head"][data-name="${DEVICE}/Mix L"]`).click();
    await expect(page.getByTestId("rename-affected")).toContainText(`1 subscription(s) use this name`);
    await expect(page.getByTestId("rename-affected")).toContainText(`Stage L @ ${DEVICE}`);
    await page.getByTestId("rename-input").fill("Main L");
    await page.getByTestId("rename-cancel").click();
    await expect(page.getByTestId("rename-dialog")).toHaveCount(0);
    expect((await labels(page)).tx[0]).toBe("Mix L");

    // An unused TX channel: no warning; renamed and back.
    await matrix.locator(`[data-testid="tx-channel-head"][data-name="${DEVICE}/Talkback"]`).click();
    await expect(page.getByTestId("rename-affected")).toHaveCount(0);
    await rename(page, "TB");
    expect((await labels(page)).tx[2]).toBe("TB");
    await matrix.locator(`[data-testid="tx-channel-head"][data-name="${DEVICE}/TB"]`).click();
    await rename(page, "Talkback");
    expect((await labels(page)).tx[2]).toBe("Talkback");
    // A name Dante refuses is refused before anything is sent.
    await matrix.locator(`[data-testid="tx-channel-head"][data-name="${DEVICE}/Talkback"]`).click();
    await page.getByTestId("rename-input").fill("Talk.back");
    await page.getByTestId("rename-confirm").click();
    await expect(page.getByTestId("change-result")).toContainText("Not changed: not a Dante channel name");
    expect((await labels(page)).tx[2]).toBe("Talkback");
    expect(console_msgs).toEqual([]);
  });

  test("a device is renamed from the devices view and back", async ({ page }) => {
    await page.goto("/#devices");
    const card = page.locator(`[data-testid="device"][data-name="${DEVICE}"]`);
    await expect(card.getByTestId("device-counts")).toHaveText("4 TX · 4 RX", { timeout: 30_000 });
    await card.getByTestId("device-name").click();
    await card.getByTestId("rename-device").click();
    await expect(page.getByTestId("rename-title")).toHaveText(`Rename device ${DEVICE}`);
    await expect(page.getByTestId("rename-reset")).toHaveCount(0);
    await expect(page.getByTestId("rename-affected")).toContainText(`Stage L @ ${DEVICE}`);
    await rename(page, "dc-e2e-renamed");
    // Its records change name: discovery lists it under the new one.
    const renamed = page.locator(`[data-testid="device"][data-name="dc-e2e-renamed"]`);
    await expect(renamed.getByTestId("device-counts")).toHaveText("4 TX · 4 RX", { timeout: 30_000 });

    await renamed.getByTestId("device-name").click();
    await renamed.getByTestId("rename-device").click();
    await rename(page, DEVICE);
    await expect(card.getByTestId("device-counts")).toHaveText("4 TX · 4 RX", { timeout: 30_000 });
    await expect(renamed).toHaveCount(0, { timeout: 30_000 });
    expect(console_msgs).toEqual([]);
  });
});
