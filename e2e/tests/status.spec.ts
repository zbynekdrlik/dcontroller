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

  // Left alone, it receives again; the next read logs that. Only a return
  // after this stop counts: the return of an earlier stop (a spec before
  // this one) can be logged in the very second `since` was taken.
  const after = async () => (await recoveries()).filter((e) => e.at >= stop.at);
  await expect.poll(async () => (await after()).length, { timeout: 75_000 }).toBe(1);
  const [back] = await after();
  expect(back.at).toBeGreaterThanOrEqual(stop.at);
  expect(back).toMatchObject({ ok: true, message: "receiving again" });

  // The Changes tab folds the stop and the return into one line for the
  // connection (#123): how long it did not receive, and why.
  await page.reload();
  const rows = page
    .locator('[data-testid="change-row"][data-kind="status"]')
    .filter({ hasText: "Stage L \u2190" });
  // The newest outage of this connection is this one (a spec before this
  // one may have left an older one).
  const row = rows.first();
  await expect(row).toHaveAttribute("data-outage", "over");
  await expect(row).toContainText(`${DEVICE} \u2192 ${DEVICE}`);
  await expect(row).toContainText("device status");
  await expect(row).toContainText("not receiving");
  await expect(row).toContainText(/received again after \d+ (s|min)/);
  await expect(row).not.toHaveClass(/err/);
  await expect(row.getByRole("link")).toHaveAttribute("href", `#map/link/${DEVICE}/${DEVICE}`);
  // One line per outage, not one per channel flip: fewer rows than the
  // log's entries for this channel.
  const flips = (await stageL(page, 0)).length;
  expect(await rows.count()).toBeLessThan(flips);
  // The filter: device status only, then the changes made only.
  await page.getByTestId("changes-status").click();
  await expect(rows.first()).toBeVisible();
  await page.getByTestId("changes-made").click();
  await expect(rows).toHaveCount(0);
  expect(msgs).toEqual([]);
});
