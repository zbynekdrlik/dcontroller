import { test, expect, type ConsoleMessage, type Page } from "@playwright/test";
import { execFileSync } from "node:child_process";
import { resolve } from "node:path";
import { run, root } from "../env";

// A subscription that stops receiving, and receives again, is logged with
// its time, so an error period can be matched to an event. dstack's own
// device (global-setup.ts) takes its RX "Stage L" from its own TX "Mix L";
// `dstack-dev ctl tx-end <flow>` ends that TX flow as if its receiver had
// gone quiet: Stage L stops within seconds and comes back by itself.
const DEVICE = "dc-e2e-device";

const ctl = (...req: string[]) =>
  execFileSync(resolve(root, "target/debug/dstack-dev"), ["ctl", "--socket", resolve(run, "control.sock"), ...req], {
    encoding: "utf8",
  });
// The device's live TX flows, by the status's flow number.
const liveTx = (): number[] =>
  (JSON.parse(ctl("status")).tx as { flow: number; live: number }[]).filter((f) => f.live).map((f) => f.flow);

type Entry = { kind: string; rx_label: string; rx_device: string; ok: boolean; message: string; at: number };
const stageL = (page: Page, since: number) =>
  page.evaluate(
    ([dev, t]) =>
      fetch("/api/changes")
        .then((r) => r.json())
        .then((l: Entry[]) =>
          l.filter((e) => e.kind === "status" && e.rx_device === dev && e.rx_label === "Stage L" && e.at >= t),
        ),
    [DEVICE, since] as const,
  );

test("a subscription that stops and receives again shows in Changes with its time", async ({ page }) => {
  // A device is read every 30 s: the stop and the recovery take a read each.
  test.setTimeout(270_000);
  const msgs: string[] = [];
  page.on("console", (m: ConsoleMessage) => {
    if (m.type() === "error" || m.type() === "warning") msgs.push(`${m.type()}: ${m.text()}`);
  });
  page.on("pageerror", (e) => msgs.push(`pageerror: ${e.message}`));
  await page.goto("/#changes");
  // Start from Stage L receiving, as the server last read it: a spec before
  // this one (a latency change) can leave it briefly down, and a stop or a
  // recovery from that must not count here.
  const stageLState = () =>
    page.evaluate(
      (dev) =>
        fetch("/api/devices")
          .then((r) => r.json())
          .then(
            (d: { devices: { name: string; channels?: { rx: { label: string; state?: string }[] } }[] }) =>
              d.devices.find((x) => x.name === dev)?.channels?.rx.find((r) => r.label === "Stage L")?.state,
          ),
      DEVICE,
    );
  await expect.poll(stageLState, { timeout: 75_000, intervals: [2_000] }).toBe("connected");
  const since = Math.floor(Date.now() / 1000);
  const stops = async () => (await stageL(page, since)).filter((e) => e.message.startsWith("stopped receiving"));
  const recoveries = async () => (await stageL(page, since)).filter((e) => e.message === "receiving again");

  // Keep its flow ended until a device read sees Stage L down (it comes
  // back by itself within 30 s, a read is due every 30 s).
  await expect
    .poll(
      async () => {
        for (const f of liveTx()) ctl("tx-end", String(f));
        return (await stops()).length;
      },
      { timeout: 100_000, intervals: [3_000] },
    )
    .toBe(1);
  const [stop] = await stops();
  expect(stop.ok).toBe(false);
  expect(stop.message).toMatch(/^stopped receiving: /);

  // Left alone, it receives again; the next read logs that.
  await expect.poll(async () => (await recoveries()).length, { timeout: 75_000 }).toBe(1);
  const [back] = await recoveries();
  expect(back.at).toBeGreaterThanOrEqual(stop.at);
  expect(back).toMatchObject({ ok: true, message: "receiving again" });

  // The Changes tab lists both, the stop as an error row.
  await page.reload();
  const rows = page.locator('[data-testid="change-row"][data-kind="status"]').filter({ hasText: `Stage L @ ${DEVICE}` });
  await expect(rows.first()).toContainText("receiving again");
  await expect(rows.first()).toContainText("device status");
  await expect(rows.nth(1)).toContainText("stopped receiving");
  await expect(rows.nth(1)).toHaveClass(/err/);
  expect(msgs).toEqual([]);
});
