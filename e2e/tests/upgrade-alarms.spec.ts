import { test, expect, type ConsoleMessage } from "@playwright/test";
import { createSocket, type Socket } from "node:dgram";
import { routed } from "../env";

// dcontroller #93 (dstack v0.6.145): the server's mDNS meter names two more
// things it relays as alarms. A dstack peer below the network floor
// (dstack #321): an ARC record's TXT states `dstack_vers`; 0.6.20 is below
// the floor 0.6.58, and the same peer answering with 0.6.145 is upgraded.
// A host that refreshes Dante records on the 96 s cycle (dstack #319): a
// Dante query from port 5353 every 96 s, three intervals in a row. This
// spec runs last (`u`): the browser's alarm lasts two more cycles after it.
const PEER = "dc-e2e-old";
const CYCLE_MS = 96_000;

function name(labels: string[]): Buffer {
  return Buffer.concat([
    ...labels.map((l) => Buffer.concat([Buffer.from([l.length]), Buffer.from(l)])),
    Buffer.from([0]),
  ]);
}

// An mDNS answer with the peer's ARC TXT record stating `version`.
function arcAnswer(version: string): Buffer {
  const head = Buffer.alloc(12);
  head.writeUInt16BE(0x8400, 2);
  head.writeUInt16BE(1, 6);
  const txt = Buffer.from(`dstack_vers=${version}`);
  const rr = Buffer.alloc(10);
  rr.writeUInt16BE(16, 0); // TXT
  rr.writeUInt16BE(1, 2); // IN
  rr.writeUInt32BE(4500, 4);
  rr.writeUInt16BE(txt.length + 1, 8);
  return Buffer.concat([head, name([PEER, "_netaudio-arc", "_udp", "local"]), rr, Buffer.from([txt.length]), txt]);
}

// A Dante query, as a standing browse refreshes it.
function netaudioQuery(): Buffer {
  const head = Buffer.alloc(12);
  head.writeUInt16BE(1, 4);
  return Buffer.concat([head, name(["_netaudio-arc", "_udp", "local"]), Buffer.from([0, 12, 0, 1])]);
}

async function socket(port: number): Promise<Socket> {
  const sock = createSocket({ type: "udp4", reuseAddr: true });
  await new Promise<void>((res) => sock.bind(port, routed.ip, () => res()));
  sock.setMulticastInterface(routed.ip);
  sock.setMulticastLoopback(true);
  return sock;
}

test("a dstack peer below the network floor and a 96 s Dante browser raise alarms, an upgrade ends one", async ({ page }) => {
  test.setTimeout(540_000);
  const msgs: string[] = [];
  page.on("console", (m: ConsoleMessage) => {
    if (m.type() === "error" || m.type() === "warning") msgs.push(`${m.type()}: ${m.text()}`);
  });
  page.on("pageerror", (e) => msgs.push(`pageerror: ${e.message}`));

  // The meter opens a few seconds after the server starts: what goes to
  // the group before it is not heard.
  await page.goto("/#network");
  await expect(page.getByTestId("floor-summary")).toBeVisible({ timeout: 30_000 });
  await expect
    .poll(() => page.evaluate(() => fetch("/api/mdns").then((r) => r.json()).then((m) => m.running)), {
      timeout: 30_000,
    })
    .toBe(true);

  // The browser: queries from port 5353, now and every 96 s.
  const browse = await socket(5353);
  const query = () => browse.send(netaudioQuery(), 5353, "224.0.0.251");
  query();
  const cycle = setInterval(query, CYCLE_MS);
  const peer = await socket(0);
  const answer = (v: string) => peer.send(arcAnswer(v), 5353, "224.0.0.251");
  try {
    answer("0.6.20");

    // The floor: the next minute names the peer, on every page.
    const floor = page.getByTestId("floor-alarm");
    await expect(floor).toBeVisible({ timeout: 90_000 });
    await expect(floor).toContainText(`dstack below the network floor 0.6.58: ${PEER} at ${routed.ip} (dstack 0.6.20)`);
    await expect(page.getByTestId("floor-summary")).toHaveClass(/error/);
    const row = page.locator(`[data-testid="floor-row"][data-name="${PEER}"]`);
    await expect(row.getByTestId("floor-version")).toHaveText("0.6.20");
    const api = await page.evaluate(() => fetch("/api/mdns").then((r) => r.json()));
    expect(api.floor).toBe("0.6.58");
    expect(api.below_floor).toContainEqual({ kind: "peer", name: PEER, address: routed.ip, version: "0.6.20" });

    await page.getByTestId("tab-changes").click();
    await expect(floor).toBeVisible();
    const started = page
      .locator('[data-testid="change-row"][data-kind="status"]')
      .filter({ hasText: `dstack ${PEER} at ${routed.ip}` })
      .filter({ hasText: "runs dstack 0.6.20" });
    await expect(started.first()).toBeVisible({ timeout: 10_000 });
    await expect(started.first()).toHaveClass(/err/);

    // Upgraded: the same peer answers with a version over the floor.
    answer("0.6.145");
    await expect(floor).toHaveCount(0, { timeout: 90_000 });
    await page.reload();
    const ended = page
      .locator('[data-testid="change-row"][data-kind="status"]')
      .filter({ hasText: `dstack ${PEER} at ${routed.ip}` })
      .filter({ hasText: "no longer below the network floor" });
    await expect(ended.first()).toBeVisible({ timeout: 10_000 });

    // The browser: named the minute after its fourth query (288 s).
    await page.getByTestId("tab-network").click();
    await expect(page.getByTestId("floor-summary")).not.toHaveClass(/error/);
    const browser = page.getByTestId("browser-alarm");
    await expect(browser).toBeVisible({ timeout: 400_000 });
    await expect(browser).toContainText(`Dante browsed on a 96 s cycle by ${routed.ip} (`);
    await expect(browser).toContainText("this server");
    await expect(page.getByTestId("browser-summary")).toHaveClass(/error/);
    const b = page.locator(`[data-testid="browser-row"][data-source="${routed.ip}"]`);
    const period = Number(await b.getByTestId("browser-period").textContent());
    expect(period).toBeGreaterThanOrEqual(94);
    expect(period).toBeLessThanOrEqual(98);

    await page.getByTestId("tab-changes").click();
    const browsing = page
      .locator('[data-testid="change-row"][data-kind="status"]')
      .filter({ hasText: `96 s browser ${routed.ip}` })
      .filter({ hasText: "refreshes Dante records every" });
    await expect(browsing.first()).toBeVisible({ timeout: 10_000 });
    await expect(browsing.first()).toHaveClass(/err/);
  } finally {
    clearInterval(cycle);
    browse.close();
    peer.close();
  }

  expect(msgs).toEqual([]);
});
