import { test, expect, type ConsoleMessage } from "@playwright/test";
import { createSocket } from "node:dgram";
import { routed } from "../env";

// dcontroller #86 (dstack #289): the server counts the mDNS senders on its
// Dante leg with dstack's meter, one closed minute at a time. This test
// floods the group from the box's routed address (the Dante interface in
// the E2E) at 200 datagrams a second, as the Companion module of dstack
// #285 did at 40/s, and reads the alarm, the Network table and the changes
// log; then it stops and reads the alarm end. The meter's minute runs from
// the server's start, so each side waits up to two minutes.
const DEVICE = "dc-e2e-device";
const RATE = 200;

// A query for a name nobody answers: a responder parses and drops it.
function query(id: number): Buffer {
  const name = Buffer.concat(
    ["dc-e2e-flood", "local"].map((l) => Buffer.concat([Buffer.from([l.length]), Buffer.from(l)])),
  );
  const head = Buffer.alloc(12);
  head.writeUInt16BE(id & 0xffff, 0);
  head.writeUInt16BE(1, 4);
  return Buffer.concat([head, name, Buffer.from([0, 0, 1, 0, 1])]);
}

test("an mDNS flood on the Dante leg raises an alarm naming its source, and its end clears it", async ({ page }) => {
  test.setTimeout(330_000);
  const msgs: string[] = [];
  page.on("console", (m: ConsoleMessage) => {
    if (m.type() === "error" || m.type() === "warning") msgs.push(`${m.type()}: ${m.text()}`);
  });
  page.on("pageerror", (e) => msgs.push(`pageerror: ${e.message}`));

  const sock = createSocket({ type: "udp4", reuseAddr: true });
  await new Promise<void>((res) => sock.bind(0, routed.ip, () => res()));
  sock.setMulticastInterface(routed.ip);
  sock.setMulticastLoopback(true);
  let n = 0;
  const flood = setInterval(() => {
    for (let i = 0; i < RATE / 20; i++) sock.send(query(n++), 5353, "224.0.0.251");
  }, 50);
  try {
    await page.goto("/#network");
    await expect(page.getByTestId("mdns-summary")).toBeVisible({ timeout: 30_000 });

    // The alarm, on every page, names the source and the devices there.
    const alarm = page.getByTestId("mdns-alarm");
    await expect(alarm).toBeVisible({ timeout: 150_000 });
    await expect(alarm).toContainText(`mDNS flood on the Dante leg: ${routed.ip} (`);
    await expect(alarm).toContainText(DEVICE);
    await expect(alarm).toContainText("this server");
    await expect(alarm).toContainText("over the cap of 50/s");
    await expect(page.getByTestId("mdns-summary")).toHaveClass(/error/);

    const api = await page.evaluate(() => fetch("/api/mdns").then((r) => r.json()));
    expect(api.running).toBe(true);
    expect(api.cap_per_s).toBe(50);
    const over = api.minute.over_cap.find((s: { source: string }) => s.source === routed.ip);
    expect(over.per_s).toBeGreaterThan(50);
    expect(over.devices).toContain(DEVICE);

    const row = page.locator(`[data-testid="mdns-row"][data-source="${routed.ip}"]`);
    await expect(row).toHaveClass(/warn/);
    await expect(row.getByTestId("mdns-rate")).toHaveText(String(api.minute.top.find((s: { source: string }) => s.source === routed.ip).per_s));

    await page.getByTestId("tab-devices").click();
    await expect(alarm).toBeVisible();

    await page.getByTestId("tab-changes").click();
    const started = page
      .locator('[data-testid="change-row"][data-kind="status"]')
      .filter({ hasText: `mDNS sender ${routed.ip}` })
      .filter({ hasText: "over the cap" });
    await expect(started.first()).toBeVisible({ timeout: 10_000 });
    await expect(started.first()).toHaveClass(/err/);
  } finally {
    clearInterval(flood);
  }
  sock.close();

  // Quiet again: the next whole minute ends the alarm and logs it.
  await expect(page.getByTestId("mdns-alarm")).toHaveCount(0, { timeout: 150_000 });
  await page.reload();
  const ended = page
    .locator('[data-testid="change-row"][data-kind="status"]')
    .filter({ hasText: `mDNS sender ${routed.ip}` })
    .filter({ hasText: "back under the cap" });
  await expect(ended.first()).toBeVisible({ timeout: 10_000 });
  await page.getByTestId("tab-network").click();
  await expect(page.getByTestId("mdns-summary")).not.toHaveClass(/error/);

  expect(msgs).toEqual([]);
});
