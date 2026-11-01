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
