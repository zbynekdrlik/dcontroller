import { test, expect, type ConsoleMessage } from "@playwright/test";

// The device global-setup.ts runs: dstack's own Dante device binary.
const DEVICE = "dc-e2e-device";

test.describe("devices", () => {
  let console_msgs: string[];

  test.beforeEach(async ({ page }) => {
    console_msgs = [];
    page.on("console", (m: ConsoleMessage) => {
      if (m.type() === "error" || m.type() === "warning") console_msgs.push(`${m.type()}: ${m.text()}`);
    });
    page.on("pageerror", (e) => console_msgs.push(`pageerror: ${e.message}`));
  });

  test("lists the discovered Dante device with its channels and subscription", async ({ page }) => {
    await page.goto("/#devices");
    const card = page.locator(`[data-testid="device"][data-name="${DEVICE}"]`);
    // Discovery plus the first ARC read: a few seconds on a real network.
    await expect(card.getByTestId("device-counts")).toHaveText("4 TX · 4 RX", { timeout: 30_000 });
    await expect(page.getByTestId("status")).toContainText(/Dante devices? on /);
    await expect(card).toContainText("dcontroller-e2e test-box");
    await expect(card).toContainText("1 subscribed");

    // Channels open on a click on the device.
    await expect(card.getByTestId("rx-table")).toBeHidden();
    await card.getByTestId("device-name").click();
    const rx = card.getByTestId("rx-channel");
    await expect(rx).toHaveCount(4);
    await expect(rx.nth(0)).toContainText("Stage L");
    await expect(rx.nth(0).getByTestId("rx-source")).toHaveText(`Mix L @ ${DEVICE}`);
    await expect(rx.nth(0).getByTestId("rx-status")).not.toBeEmpty();
    await expect(rx.nth(1).getByTestId("rx-source")).toHaveText("");
    await expect(card.getByTestId("tx-channel")).toHaveText(["Mix L", "Mix R", "Talkback", "Click"]);

    // The page shows what the server read, not something of its own.
    const api = await page.evaluate(() => fetch("/api/devices").then((r) => r.json()));
    expect(api.enabled).toBe(true);
    const dev = api.devices.find((d: { name: string }) => d.name === DEVICE);
    expect(dev.channels.rx[0].source).toEqual({ channel: "Mix L", device: DEVICE });
    // dstack's own device answers the settings read with its config
    // (fixtures/device.toml): 48 kHz, no pull-up, 10 ms latency.
    expect(dev.settings).toMatchObject({
      sample_rate: 48000,
      pullup: 0,
      latency_configured_us: 10000,
      latency_active_us: 10000,
    });
    // Its product info (UDP 8700) and encoding (TX channels) come from the same config.
    expect(dev.info).toMatchObject({
      manufacturer: "dcontroller-e2e",
      model: "test-box",
      encoding_bits: 24,
    });
    await expect(card.getByTestId("device-settings")).toHaveText("48 kHz · 24 bit · latency 10.00 ms");
    const table = card.getByTestId("settings-table");
    await expect(table).toBeVisible();
    const row = (name: string) => table.locator("tr", { has: page.getByRole("rowheader", { name, exact: true }) }).locator("td");
    await expect(row("Sample rate")).toHaveText("48000 Hz");
    await expect(row("Encoding")).toHaveText("PCM 24 bit");
    await expect(row("Latency, active")).toHaveText("10.00 ms");
    await expect(row("Manufacturer")).toHaveText("dcontroller-e2e");
    await expect(row("Model")).toHaveText("test-box");
    // Each version the device reports is shown as the server read it.
    for (const [key, name] of [
      ["product_version", "Product version"],
      ["software_version", "Software"],
      ["firmware_version", "Firmware"],
      ["platform", "Dante platform"],
    ]) {
      if (dev.info[key]) await expect(row(name)).toHaveText(dev.info[key]);
      else await expect(row(name)).toHaveCount(0);
    }
    expect(console_msgs).toEqual([]);
  });

  test("an open device stays open while the list refreshes", async ({ page }) => {
    await page.goto("/#devices");
    const card = page.locator(`[data-testid="device"][data-name="${DEVICE}"]`);
    await expect(card.getByTestId("device-counts")).toHaveText("4 TX · 4 RX", { timeout: 30_000 });
    await card.getByTestId("device-name").click();
    await expect(card.getByTestId("rx-table")).toBeVisible();
    // Two poll intervals (3 s each) later the channels are still shown.
    await page.waitForTimeout(7_000);
    await expect(card.getByTestId("rx-table")).toBeVisible();
    expect(console_msgs).toEqual([]);
  });
});
