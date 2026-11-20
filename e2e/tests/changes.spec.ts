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

  await page.goto(`/#map/device/${DEVICE}`);
  await expect(page.getByTestId("map-what")).toContainText("4 TX · 4 RX", { timeout: 30_000 });

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
    // #142: `#changes/<device>` lists only what concerns that device (the
    // Map's "Changes of" link); one with no change lists nothing of it.
    await page.goto(`/#changes/${DEVICE}`);
    await expect(page.getByTestId("changes-only")).toHaveText(`only ${DEVICE}`);
    await expect(page.getByTestId("change-row").filter({ hasText: "Ext 3" }).first()).toBeVisible();
    await page.goto("/#changes/no-such-device");
    await expect(page.getByTestId("changes-only")).toHaveText("only no-such-device");
    await expect(page.getByTestId("change-row")).toHaveCount(0);
    await page.getByTestId("changes-every").click();
    await expect(page).toHaveURL(/#changes$/);
    await expect(page.getByTestId("change-row").filter({ hasText: "Ext 3" }).first()).toBeVisible();
    // #142: what is opened in the app is recorded; the page lists it, the
    // most opened first, each a link back to it.
    await page.goto(`/#map/device/${DEVICE}`);
    await expect(page.getByTestId("map-title")).toHaveText(DEVICE, { timeout: 30_000 });
    await page.goto("/#changes");
    await page.getByTestId("changes-usage").click();
    const used = page.getByTestId("usage-row").filter({ hasText: `device/${DEVICE}` });
    await expect(used.first()).toBeVisible({ timeout: 10_000 });
    await expect(used.first().locator("a")).toHaveAttribute("href", `#map/device/${DEVICE}`);
    const usage = await page.evaluate(() => fetch("/api/usage").then((r) => r.json()));
    expect(usage.days).toBe(30);
    expect(usage.rows.some((r: any) => r.view === "changes")).toBe(true);
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
