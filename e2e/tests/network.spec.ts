import { test, expect, type ConsoleMessage } from "@playwright/test";

// The device global-setup.ts runs: dstack's own Dante device binary. It
// answers the network info with the interface it runs on.
const DEVICE = "dc-e2e-device";

const link = (m: number) => (m === 0 ? "no link" : m % 1000 === 0 ? `${m / 1000} Gbit/s` : `${m} Mbit/s`);
const text = (p: { link_mbit: number; ip: string }) => (p.link_mbit === 0 ? "no link" : `${link(p.link_mbit)} · ${p.ip}`);

test("the network tab lists every device's interfaces as the server read them", async ({ page }) => {
  const msgs: string[] = [];
  page.on("console", (m: ConsoleMessage) => {
    if (m.type() === "error" || m.type() === "warning") msgs.push(`${m.type()}: ${m.text()}`);
  });
  page.on("pageerror", (e) => msgs.push(`pageerror: ${e.message}`));

  await page.goto("/");
  await page.getByTestId("tab-network").click();
  await expect(page).toHaveURL(/#network$/);
  const row = page.locator(`[data-testid="network-row"][data-name="${DEVICE}"]`);
  await expect(row.getByTestId("network-primary")).toBeVisible({ timeout: 30_000 });

  const api = await page.evaluate(() => fetch("/api/devices").then((r) => r.json()));
  const dev = api.devices.find((d: { name: string }) => d.name === DEVICE);
  // The device runs on the box's routed address: its primary carries that address.
  expect(dev.network.length).toBeGreaterThan(0);
  expect(dev.address.startsWith(`${dev.network[0].ip}:`)).toBe(true);
  await expect(row.getByTestId("network-primary")).toHaveText(text(dev.network[0]));

  await expect(page.getByTestId("network-row")).toHaveCount(api.devices.length);
  for (const d of api.devices) {
    const r = page.locator(`[data-testid="network-row"][data-name="${d.name}"]`);
    if (d.network?.length) await expect(r.getByTestId("network-primary")).toHaveText(text(d.network[0]));
    else await expect(r.getByTestId("network-none")).toBeVisible();
  }
  await expect(page.getByTestId("network-summary")).toContainText("devices report their network");

  // The device table shows the same interface.
  await page.getByTestId("tab-devices").click();
  const card = page.locator(`[data-testid="device"][data-name="${DEVICE}"]`);
  await card.getByTestId("device-name").click();
  const prim = card
    .getByTestId("settings-table")
    .locator("tr", { has: page.getByRole("rowheader", { name: "Primary network", exact: true }) })
    .locator("td");
  await expect(prim).toHaveText(text(dev.network[0]));
  expect(msgs).toEqual([]);
});
