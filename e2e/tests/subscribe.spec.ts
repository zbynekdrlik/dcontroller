import { test, expect, type ConsoleMessage, type Page } from "@playwright/test";

// The device global-setup.ts runs (dstack's own Dante device): ours to
// change. RX "Stage R" starts unsubscribed; every test leaves it so.
const DEVICE = "dc-e2e-device";

async function openMatrix(page: Page) {
  await page.goto("/#routing");
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
    // Other specs log subscriptions too: count only this test's.
    const logged = (await page.evaluate(() => fetch("/api/changes").then((r) => r.json()))).length;

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

    const all = await page.evaluate(() => fetch("/api/changes").then((r) => r.json()));
    const log = all.slice(0, all.length - logged).filter((e: { kind: string }) => e.kind === "subscription");
    expect(log.length).toBe(2);
    expect(log[0].requested).toBeNull();
    expect(log[0].after).toBeNull();
    expect(log[0].before).toEqual({ channel: "Mix R", device: DEVICE });
    expect(log.every((e: { ok: boolean }) => e.ok)).toBe(true);
    expect(console_msgs).toEqual([]);
  });

  test("a subscription that does not receive is one line above the matrix, details on a click", async ({ page }) => {
    // A device is re-read every 30 s; the test waits for one re-read.
    test.setTimeout(90_000);
    await openMatrix(page);
    const box = page.getByTestId("sub-problems");
    const problem = page.locator(`[data-testid="sub-problem"][data-rx="Stage R @ ${DEVICE}"]`);
    await expect(box).toHaveCount(0);
    const ask = (source: { channel: string; device: string } | null) =>
      page.evaluate(
        (body) =>
          fetch("/api/subscription", {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify(body),
          }).then((r) => r.json()),
        { rx_device: DEVICE, rx_channel: 2, source },
      );

    // A source device that is not on the network is no error: no line at all.
    await ask({ channel: "Mic", device: "dc-e2e-switched-off" });
    await expect.poll(async () => (await stageR(page)).source?.device, { timeout: 15_000 }).toBe("dc-e2e-switched-off");
    expect((await stageR(page)).state).not.toBe("connected");
    await page.waitForResponse((r) => r.url().endsWith("/api/devices"));
    await page.waitForTimeout(500);
    await expect(box).toHaveCount(0);

    // The device is on the network, the channel is not there: one line, the row hidden.
    await ask({ channel: "Nowhere", device: DEVICE });
    const summary = page.getByTestId("sub-summary");
    await expect(summary).toBeVisible({ timeout: 30_000 });
    const st = await stageR(page);
    expect(st.state).not.toBe("connected");
    // Other specs leave subscriptions to devices that are gone: those only count as offline.
    await expect(page.getByTestId(st.state === "error" ? "sub-failing" : "sub-unresolved")).toHaveText(
      st.state === "error" ? "1 failing" : "1 not resolved",
    );
    await expect(page.locator('[data-testid="sub-problem"]')).toHaveCount(1);
    // The receiver's row header counts it: red for an error, amber otherwise.
    const badge = page.locator(`[data-testid="rx-device"][data-name="${DEVICE}"] [data-testid="rx-trouble"]`);
    await expect(badge).toHaveText("1");
    await expect(badge).toHaveClass(st.state === "error" ? /\berr\b/ : /\bwarn\b/);
    await expect(problem).toBeHidden();
    // A click opens the details: which channel, from where, what the device says.
    await summary.click();
    await expect(problem).toBeVisible();
    await expect(problem.locator(".p-what")).toHaveText(`Stage R @ ${DEVICE} \u2190 Nowhere @ ${DEVICE}`);
    await expect(problem.locator(".p-status")).toHaveText(st.status);
    await expect(problem).toHaveClass(st.state === "error" ? /\berr\b/ : /\bwarn\b/);

    // Opened, it stays open while the matrix takes a new device answer.
    const readAt = async () =>
      (await page.evaluate(() => fetch("/api/devices").then((r) => r.json()))).devices.find(
        (x: { name: string }) => x.name === DEVICE,
      ).read_at;
    const read = await readAt();
    await expect.poll(readAt, { timeout: 45_000 }).not.toBe(read);
    for (let i = 0; i < 2; i++) await page.waitForResponse((r) => r.url().endsWith("/api/devices"));
    await page.waitForTimeout(300);
    await expect(problem).toBeVisible();

    // Removed, the line goes.
    await ask(null);
    await expect(box).toHaveCount(0, { timeout: 30_000 });
    expect((await stageR(page)).source).toBeNull();
    await expect(badge).toHaveCount(0);
    expect(console_msgs).toEqual([]);
  });
});
