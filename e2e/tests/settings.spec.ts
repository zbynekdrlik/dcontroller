import { test, expect, type ConsoleMessage, type Page } from "@playwright/test";

// Setting writes go only to dstack's own device (global-setup.ts), never to
// a site device. It takes a latency write within its config's rules (as
// hardware does), runs one sample rate, and takes no encoding write.
const DEVICE = "dc-e2e-device";
const OPTIONS = ["1.00 ms", "2.00 ms", "4.00 ms", "5.00 ms", "6.00 ms", "10.00 ms", "20.00 ms", "40.00 ms"];

// Status entries (a subscription stopped or receives again) arrive with device reads at any time: leave them out.
const changes = (page: Page) =>
  page.evaluate(() =>
    fetch("/api/changes")
      .then((r) => r.json())
      .then((l: { kind: string }[]) => l.filter((e) => e.kind !== "status")),
  );

test.describe("settings", () => {
  let msgs: string[];
  test.beforeEach(async ({ page }) => {
    msgs = [];
    page.on("console", (m: ConsoleMessage) => {
      if (m.type() === "error" || m.type() === "warning") msgs.push(`${m.type()}: ${m.text()}`);
    });
    page.on("pageerror", (e) => msgs.push(`pageerror: ${e.message}`));
    await page.goto(`/#map/device/${DEVICE}`);
    await expect(page.getByTestId("map-what")).toContainText("4 TX · 4 RX", { timeout: 30_000 });
  });

  test("a latency change is confirmed, read back from the device, logged, and set back", async ({ page }) => {
    const card = page.getByTestId("map-device-body");
    const setting = (name: string) =>
      card.getByTestId("settings-table").locator("tr", { has: page.getByRole("rowheader", { name, exact: true }) }).locator("td");
    const dialog = page.getByTestId("setting-dialog");
    await card.getByTestId("change-latency").click();
    await expect(dialog.getByTestId("setting-title")).toHaveText(`Change the latency of ${DEVICE} (now 10.00 ms)`);
    // Dante Controller's choices within the device's range, plus its current one.
    // dstack v0.6.90 states its real minimum (its TX floor, 0.633 ms at
    // 96 kHz fpp 32), so 0.25 and 0.50 ms are no longer offered.
    // Software latencies too (#101): 4 to 40 ms, within its range.
    await expect(dialog.locator("option")).toHaveText(OPTIONS);
    await expect(dialog.getByTestId("setting-warning")).toContainText("may drop out briefly");
    await expect(dialog.getByTestId("setting-apply")).toBeDisabled();

    const before = await changes(page);
    await dialog.getByTestId("setting-value").selectOption("5000");
    await dialog.getByTestId("setting-apply").click();
    await expect(page.getByTestId("change-result")).toHaveText("Done: done, read back from the device: latency 5.00 ms", {
      timeout: 20_000,
    });
    await expect(setting("Latency, active")).toHaveText("5.00 ms");
    const after = await changes(page);
    expect(after.length).toBe(before.length + 1);
    expect(after[0]).toMatchObject({
      kind: "setting",
      what: "latency",
      rx_device: DEVICE,
      name_before: "10.00 ms",
      name_asked: "5.00 ms",
      name_after: "5.00 ms",
      ok: true,
    });

    // 10 ms is a choice of its own now (#101): still offered at 5 ms.
    await card.getByTestId("change-latency").click();
    await expect(dialog.locator("option")).toHaveText(OPTIONS);
    await dialog.getByTestId("setting-cancel").click();
    // Cleanup through the API: back to the config's latency, so the other
    // tests see the device as it was.
    const back = await page.evaluate(
      (d) =>
        fetch("/api/setting", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ device: d, setting: { latency_us: 10000 } }),
        }).then((r) => r.json()),
      DEVICE,
    );
    expect(back.ok).toBe(true);
    await expect(setting("Latency, active")).toHaveText("10.00 ms");
    expect(msgs).toEqual([]);
  });

  test("a device with one sample rate offers nothing else", async ({ page }) => {
    const card = page.getByTestId("map-device-body");
    const setting = (name: string) =>
      card.getByTestId("settings-table").locator("tr", { has: page.getByRole("rowheader", { name, exact: true }) }).locator("td");
    await card.getByTestId("change-sample-rate").click();
    const dialog = page.getByTestId("setting-dialog");
    await expect(dialog.getByTestId("setting-title")).toHaveText(`Change the sample rate of ${DEVICE} (now 48 kHz)`);
    await expect(dialog.locator("option")).toHaveText(["48 kHz"]);
    await expect(dialog.getByTestId("setting-apply")).toBeDisabled();
    await dialog.getByTestId("setting-cancel").click();
    await expect(dialog).toHaveCount(0);
    expect(msgs).toEqual([]);
  });

  test("an encoding the device does not take ends not confirmed, logged; cancel sends nothing", async ({ page }) => {
    const card = page.getByTestId("map-device-body");
    const setting = (name: string) =>
      card.getByTestId("settings-table").locator("tr", { has: page.getByRole("rowheader", { name, exact: true }) }).locator("td");
    const dialog = page.getByTestId("setting-dialog");
    await card.getByTestId("change-encoding").click();
    await expect(dialog.getByTestId("setting-title")).toHaveText(`Change the encoding of ${DEVICE} (now 24 bit)`);
    await expect(dialog.getByTestId("setting-warning")).toContainText("Receivers that do not take this encoding");
    await dialog.getByTestId("setting-value").selectOption("32");
    await expect(dialog.getByTestId("setting-apply")).toHaveText("Set 32 bit");

    const before = await changes(page);
    await dialog.getByTestId("setting-cancel").click();
    await expect(dialog).toHaveCount(0);
    expect((await changes(page)).length).toBe(before.length);

    await card.getByTestId("change-encoding").click();
    await dialog.getByTestId("setting-value").selectOption("32");
    await dialog.getByTestId("setting-apply").click();
    await expect(page.getByTestId("change-result")).toHaveText(
      "Not changed: not confirmed: the device's channels read 24 bit after 5 s",
      { timeout: 20_000 },
    );
    const after = await changes(page);
    expect(after.length).toBe(before.length + 1);
    expect(after[0]).toMatchObject({
      user: "local",
      kind: "setting",
      what: "encoding",
      name_before: "24 bit",
      name_asked: "32 bit",
      name_after: "24 bit",
      ok: false,
    });
    await page.getByTestId("tab-changes").click();
    const row = page.locator('[data-testid="change-row"]:not([data-kind="status"])').first();
    await expect(row).toContainText(`encoding @ ${DEVICE}`);
    await expect(row).toHaveClass(/err/);
    await page.goto(`/#map/device/${DEVICE}`);
    await expect(setting("Encoding")).toHaveText("PCM 24 bit", { timeout: 30_000 });
    expect(msgs).toEqual([]);
  });
});
