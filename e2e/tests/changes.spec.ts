import { test, expect, type ConsoleMessage } from "@playwright/test";
import { execFileSync } from "node:child_process";
import { resolve } from "node:path";
import { root, routed } from "../env";

// A change made outside dcontroller (here a rename straight over ARC by the
// `arc_rename` helper, through dstack, as Dante Controller or a mixer would)
// shows in Changes once the next read sees it.
const DEVICE = "dc-e2e-device";
const rename = (arc: string, channel: number, label: string) =>
  execFileSync(resolve(root, "target/debug/examples/arc_rename"), [
    routed.ip,
    arc,
    "rx",
    String(channel),
    label,
  ]).toString();

// The list reads Slovak time, whatever zone the browser is in.
test.use({ timezoneId: "America/New_York" });

test("a channel renamed outside dcontroller shows in Changes", async ({ page }) => {
  test.setTimeout(120_000);
  const msgs: string[] = [];
  page.on("console", (m: ConsoleMessage) => {
    if (m.type() === "error" || m.type() === "warning") msgs.push(`${m.type()}: ${m.text()}`);
  });
  page.on("pageerror", (e) => msgs.push(`pageerror: ${e.message}`));

  await page.goto("/#devices");
  const card3 = page.locator(`[data-testid="device"][data-name="${DEVICE}"]`);
  await expect(card3.getByTestId("device-counts")).toHaveText("4 TX · 4 RX", { timeout: 30_000 });

  const devices = await page.evaluate(() => fetch("/api/devices").then((r) => r.json()));
  const arc = devices.devices.find((d: { name: string }) => d.name === DEVICE).address;
  expect(rename(arc, 3, "Ext 3")).toBe("Ext 3\n");
  try {
    // The next read (within 30 s) sees it and logs it as made outside.
    await expect
      .poll(
        async () => {
          const log = await page.evaluate(() => fetch("/api/changes").then((r) => r.json()));
          return log.find((e: { what: string; name_after: string }) => e.what === "rx" && e.name_after === "Ext 3");
        },
        { timeout: 45_000, intervals: [2_000] },
      )
      .toMatchObject({ user: "outside dcontroller", kind: "rename", rx_channel: 3, name_before: "Spare 3", ok: true });
    await page.getByTestId("tab-changes").click();
    const row = page.getByTestId("change-row").filter({ hasText: "Ext 3" }).first();
    await expect(row).toContainText("outside dcontroller");
    await expect(row).toContainText(`rename RX 3 @ ${DEVICE}`);
    const logged = await page.evaluate(() => fetch("/api/changes").then((r) => r.json()));
    const at = logged.find((e: { name_after: string }) => e.name_after === "Ext 3").at;
    await expect(row).toContainText(new Date(at * 1000).toLocaleString("sk-SK", { timeZone: "Europe/Bratislava" }));
    await expect(row).toContainText("Spare 3");
  } finally {
    rename(arc, 3, "Spare 3");
    // The other tests see the device as it was: wait until a read has it back.
    await expect
      .poll(
        async () => {
          const d = await page.evaluate(() => fetch("/api/devices").then((r) => r.json()));
          const dev = d.devices.find((x: { name: string }) => x.name === "dc-e2e-device");
          return dev?.channels?.rx?.find((r: { channel: number }) => r.channel === 3)?.label;
        },
        { timeout: 45_000, intervals: [2_000] },
      )
      .toBe("Spare 3");
  }
  expect(msgs).toEqual([]);
});
