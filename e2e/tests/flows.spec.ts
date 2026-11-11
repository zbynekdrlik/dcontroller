import { test, expect, type ConsoleMessage, type Page } from "@playwright/test";

// TX flows on dstack's own device (global-setup.ts), never a site device.
// It lists, creates and deletes TX flows over ARC 2.8.9 like an UltimoX2
// (dstack v0.6.91).
const DEVICE = "dc-e2e-device";

// Status entries (a subscription stopped or receives again) arrive with device reads at any time: leave them out.
const changes = (page: Page) =>
  page.evaluate(() =>
    fetch("/api/changes")
      .then((r) => r.json())
      .then((l: { kind: string }[]) => l.filter((e) => e.kind !== "status")),
  );
const subscribe = (page: Page, source: { channel: string; device: string } | null) =>
  page.evaluate(
    (body) =>
      fetch("/api/subscription", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      }).then((r) => r.json()),
    { rx_device: DEVICE, rx_channel: 2, source },
  );

test.describe("TX flows", () => {
  let msgs: string[];
  test.beforeEach(async ({ page }) => {
    msgs = [];
    page.on("console", (m: ConsoleMessage) => {
      if (m.type() === "error" || m.type() === "warning") msgs.push(`${m.type()}: ${m.text()}`);
    });
    page.on("pageerror", (e) => msgs.push(`pageerror: ${e.message}`));
  });

  test("a transmitter's flows are listed with their receiver; a multicast flow is created, listed and deleted, each confirmed and logged", async ({
    page,
  }) => {
    test.setTimeout(90_000);
    await page.goto("/#devices");
    const card = page.locator(`[data-testid="device"][data-name="${DEVICE}"]`);
    await expect(card.getByTestId("device-counts")).toHaveText("4 TX · 4 RX", { timeout: 30_000 });
    // Its fixture's Stage L <- Mix L is one unicast TX flow; a subscription
    // of Stage R to its own Mix R adds a second once it receives.
    expect((await subscribe(page, { channel: "Mix R", device: DEVICE })).ok).toBe(true);
    // The device starts the flow a few seconds after the subscription.
    await expect
      .poll(
        async () =>
          (await page.evaluate((d) => fetch(`/api/flows/${d}`).then((r) => r.json()), DEVICE)).flows.length,
        { timeout: 30_000 },
      )
      .toBe(2);
    await card.getByTestId("device-name").click();

    await card.getByTestId("tx-flows").click();
    // dstack's device states 32 flow slots.
    await expect(card.getByTestId("flows-count")).toHaveText("2 of 32 TX flows", { timeout: 15_000 });
    const rows = card.locator('[data-testid="flow-row"][data-kind="unicast"]');
    await expect(rows.getByTestId("flow-channels")).toHaveText(["Mix L (1)", "Mix R (2)"]);
    await expect(rows.getByTestId("flow-to")).toHaveText([DEVICE, DEVICE]);
    // A unicast flow is the receiver's: no Delete on it, and the server
    // refuses one before sending anything.
    await expect(card.getByTestId("flow-delete")).toHaveCount(0);
    const logged = await changes(page);
    const refused = await page.evaluate(
      (body) =>
        fetch("/api/multicast/delete", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(body),
        }).then((r) => r.json()),
      { device: DEVICE, slot: 1 },
    );
    expect(refused).toMatchObject({ ok: false, message: "slot 1 is a unicast flow: remove the receiver's subscription instead" });
    expect(await changes(page)).toEqual(logged);
    // It stays open across the device poll's rebuilds.
    await page.waitForResponse((r) => r.url().endsWith("/api/devices"));
    await page.waitForTimeout(300);
    await expect(card.getByTestId("flows-table")).toBeVisible();

    // Cancel sends nothing.
    const before = await changes(page);
    const dialog = page.getByTestId("multicast-dialog");
    await card.getByTestId("create-multicast").click();
    await expect(dialog.getByTestId("multicast-create")).toBeDisabled();
    await dialog.getByTestId("multicast-cancel").click();
    await expect(dialog).toHaveCount(0);
    expect(await changes(page)).toEqual(before);

    // Two channels, a question naming them, sent on Create.
    await card.getByTestId("create-multicast").click();
    await dialog.locator('[data-testid="multicast-channel"][data-channel="1"] input').check();
    await dialog.locator('[data-testid="multicast-channel"][data-channel="2"] input').check();
    await expect(dialog.getByTestId("multicast-question")).toContainText(`Create a multicast flow on ${DEVICE} with `);
    await dialog.getByTestId("multicast-create").click();
    await expect(page.getByTestId("change-result")).toHaveText(/^Done: done, read back from the device: slot 3 multicast 1,2 -> 239\.255\.\d+\.\d+:4321$/, {
      timeout: 20_000,
    });
    await expect(dialog).toHaveCount(0);
    const after = await changes(page);
    expect(after.length).toBe(before.length + 1);
    expect(after[0]).toMatchObject({ kind: "multicast", rx_device: DEVICE, name_asked: "1,2", ok: true });

    // Listed back as a multicast flow to its group, with a Delete button.
    const mc = card.locator('[data-testid="flow-row"][data-kind="multicast"]');
    await expect(mc).toHaveCount(1, { timeout: 15_000 });
    await expect(mc.getByTestId("flow-channels")).toHaveText("Mix L (1), Mix R (2)");
    await expect(mc.getByTestId("flow-to")).toHaveText(/^239\.255\.\d+\.\d+:4321$/);
    await expect(card.getByTestId("flows-count")).toHaveText("3 of 32 TX flows");

    // Delete: cancel sends nothing; confirmed, it is gone from the device's list.
    const del = page.getByTestId("flow-delete-dialog");
    await mc.getByTestId("flow-delete").click();
    await expect(del.getByTestId("flow-delete-question")).toHaveText(
      /^Delete multicast flow slot 3 on dc-e2e-device \(Mix L \(1\), Mix R \(2\) to 239\.255\.\d+\.\d+:4321\)\? Receivers subscribed to it stop receiving\.$/,
    );
    const beforeDelete = await changes(page);
    await del.getByTestId("flow-delete-cancel").click();
    await expect(del).toHaveCount(0);
    expect(await changes(page)).toEqual(beforeDelete);
    await mc.getByTestId("flow-delete").click();
    await del.getByTestId("flow-delete-confirm").click();
    await expect(page.getByTestId("change-result")).toHaveText(/^Done: done, gone from the device's list: slot 3 multicast 1,2 -> /, {
      timeout: 20_000,
    });
    await expect(mc).toHaveCount(0, { timeout: 15_000 });
    await expect(card.getByTestId("flows-count")).toHaveText("2 of 32 TX flows");
    const deleted = await changes(page);
    expect(deleted.length).toBe(beforeDelete.length + 1);
    expect(deleted[0]).toMatchObject({ kind: "multicast", what: "delete", rx_device: DEVICE, rx_channel: 3, name_after: "gone", ok: true });

    // The Changes tab lists both.
    await page.getByTestId("tab-changes").click();
    const logRows = page.locator('[data-testid="change-row"]:not([data-kind="status"])');
    await expect(logRows.first()).toContainText(`delete multicast flow slot 3 @ ${DEVICE}`);
    await expect(logRows.nth(1)).toContainText(`new multicast flow @ ${DEVICE}`);

    expect((await subscribe(page, null)).ok).toBe(true);
    expect(msgs).toEqual([]);
  });
});
