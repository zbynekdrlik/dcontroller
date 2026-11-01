import { test, expect, type ConsoleMessage, type Page } from "@playwright/test";

// The device global-setup.ts runs (dstack's own Dante device): ours to
// change. RX "Stage R" starts unsubscribed; every test leaves it so.
const DEVICE = "dc-e2e-device";

async function openMatrix(page: Page) {
  await page.goto("/");
  const matrix = page.getByTestId("matrix");
  await expect(matrix.locator(`[data-testid="cell"][data-rx="${DEVICE}"][data-tx="${DEVICE}"]`)).not.toHaveText("", {
    timeout: 30_000,
  });
  await matrix.locator(`[data-testid="rx-device"][data-name="${DEVICE}"]`).click();
  await matrix.locator(`[data-testid="tx-device"][data-name="${DEVICE}"]`).click();
  return matrix;
}

async function stageR(page: Page) {
  const api = await page.evaluate(() => fetch("/api/devices").then((r) => r.json()));
  const dev = api.devices.find((d: { name: string }) => d.name === DEVICE);
  return dev.channels.rx.find((r: { label: string }) => r.label === "Stage R");
}

test.describe("subscription changes", () => {
  let console_msgs: string[];

  test.beforeEach(async ({ page }) => {
    console_msgs = [];
    page.on("console", (m: ConsoleMessage) => {
      if (m.type() === "error" || m.type() === "warning") console_msgs.push(`${m.type()}: ${m.text()}`);
    });
    page.on("pageerror", (e) => console_msgs.push(`pageerror: ${e.message}`));
  });

  test("cancel sends nothing", async ({ page }) => {
    const matrix = await openMatrix(page);
    const logBefore = await page.evaluate(() => fetch("/api/changes").then((r) => r.json()));
    await matrix.locator(`[data-testid="cell"][data-rx="${DEVICE}/Stage R"][data-tx="${DEVICE}/Mix R"]`).click();
    await expect(page.getByTestId("confirm-text")).toHaveText(`Subscribe Stage R @ ${DEVICE} to Mix R @ ${DEVICE}?`);
    await page.getByTestId("cancel-change").click();
    await expect(page.getByTestId("confirm-dialog")).toHaveCount(0);
    expect((await stageR(page)).source).toBeNull();
    const log = await page.evaluate(() => fetch("/api/changes").then((r) => r.json()));
    expect(log).toEqual(logBefore);
    expect(console_msgs).toEqual([]);
  });

  test("subscribe, read back, log, then remove", async ({ page }) => {
    const matrix = await openMatrix(page);
    const cell = matrix.locator(`[data-testid="cell"][data-rx="${DEVICE}/Stage R"][data-tx="${DEVICE}/Mix R"]`);
    await expect(cell).toHaveText("");

    await cell.click();
    await page.getByTestId("confirm-change").click();
    await expect(page.getByTestId("change-result")).toHaveText(/^Done: done, read back from the device$/);
    await expect(page.getByTestId("confirm-dialog")).toHaveCount(0);
    await expect(cell).toHaveText("●");
    // The device itself reports it (the server read it back over ARC).
    expect((await stageR(page)).source).toEqual({ channel: "Mix R", device: DEVICE });

    // The changes tab lists it: who, which channel, before and after.
    await page.getByTestId("tab-changes").click();
    const row = page.getByTestId("change-row").first();
    await expect(row).toContainText("local");
    await expect(row).toContainText(`Stage R @ ${DEVICE}`);
    await expect(row).toContainText(`Mix R @ ${DEVICE}`);
    await expect(row).toContainText("nothing");

    // A click on the subscribed cross-point offers its removal.
    await page.getByTestId("tab-routing").click();
    await expect(cell).toHaveText("●");
    await cell.click();
    await expect(page.getByTestId("confirm-text")).toHaveText(
      `Remove the subscription of Stage R @ ${DEVICE} (now Mix R @ ${DEVICE})?`,
    );
    await page.getByTestId("confirm-change").click();
    await expect(page.getByTestId("change-result")).toHaveText(/^Done: /);
    await expect(cell).toHaveText("");
    expect((await stageR(page)).source).toBeNull();

    const log = (await page.evaluate(() => fetch("/api/changes").then((r) => r.json()))).filter(
      (e: { kind: string }) => e.kind === "subscription",
    );
    expect(log.length).toBe(2);
    expect(log[0].requested).toBeNull();
    expect(log[0].after).toBeNull();
    expect(log[0].before).toEqual({ channel: "Mix R", device: DEVICE });
    expect(log.every((e: { ok: boolean }) => e.ok)).toBe(true);
    expect(console_msgs).toEqual([]);
  });
});
