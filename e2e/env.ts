import { execFileSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { resolve } from "node:path";

export const root = resolve(__dirname, "..");
// Not under test-results: Playwright empties that directory when it starts.
export const run = resolve(__dirname, ".run");
mkdirSync(run, { recursive: true });

// The interface and IPv4 of the default route: where the test device runs
// and where the server discovers it.
function routedIface(): { iface: string; ip: string } {
  const out = execFileSync("ip", ["-4", "route", "get", "192.0.2.1"], { encoding: "utf8" });
  const iface = /\bdev (\S+)/.exec(out)?.[1];
  const ip = /\bsrc (\S+)/.exec(out)?.[1];
  if (!iface || !ip) throw new Error(`no default route: ${out}`);
  return { iface, ip };
}

export const routed = routedIface();

// A dsync clock service is another program on another PC (dcontroller #124):
// the suite stands a small HTTP server in for it, answering `/status` with
// the fields dsync's contract names. The second address has nothing behind
// it: a node that does not answer. Two more stand in for an older dsync
// that names no status schema and a newer one on schema 2 (#124).
export const DSYNC = {
  up: "127.0.0.1:18898",
  down: "127.0.0.1:18899",
  old: "127.0.0.1:18897",
  newer: "127.0.0.1:18896",
} as const;

// camera-box's video nodes (#124 phase 1, contract camera-box#1540): the
// suite stands in for the node list and two nodes; the OBS node reports one
// JPEG XS input from the cambox whose frames start to be lost after a few
// reads, so its line turns red.
export const VIDEO = {
  list: "127.0.0.1:18895",
  nodes: "127.0.0.1:18894",
  cam: "cam-e2e",
  obs: "obs-e2e",
  conn: "jxs:cam-e2e->obs-e2e:JXS cam-e2e src",
} as const;

/// Opens the menu behind the burger and goes to a page (#122: the menu is
/// closed until asked for).
export async function nav(page: import("@playwright/test").Page, slug: string) {
  await page.getByTestId("menu-button").click();
  await page.getByTestId(`tab-${slug}`).click();
}
