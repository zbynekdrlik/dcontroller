import { defineConfig } from "@playwright/test";
import { rmSync, writeFileSync } from "node:fs";
import { networkInterfaces } from "node:os";
import { resolve } from "node:path";
import { DSYNC, root, routed, run } from "./env";

// The suite sends real Dante and mDNS packets (a test device, fake dstack
// peers, floods, 96 s browsers): only inside isolated.sh's network
// namespace, never on a real network (dcontroller #95: a run on dev1 put
// them on the site's Dante network). Checked before the server starts.
const seen = Object.keys(networkInterfaces()).sort();
if (process.env.DC_E2E_NETNS !== "1" || seen.join(",") !== "e2e0,lo") {
  throw new Error(
    `E2E runs only inside e2e/isolated.sh (dcontroller #95): this one sees ${seen.join(", ")}, so its packets would reach a real network`,
  );
}

// The real server binary serves the real trunk build and discovers a real
// Dante device (global-setup.ts); nothing is mocked.
const port = 18080;
const config = resolve(run, "config.toml");
const changes = resolve(run, "changes.jsonl");
const history = resolve(run, "history.db");
rmSync(changes, { force: true });
for (const f of [history, `${history}-wal`, `${history}-shm`]) rmSync(f, { force: true });
writeFileSync(
  config,
  `bind = "127.0.0.1:${port}"\nweb_dir = "${resolve(root, "crates/web/dist")}"\ndante_interface = "${routed.iface}"\nchanges_log = "${changes}"\nhistory_db = "${history}"\ndsync_nodes = ["${DSYNC.up}", "${DSYNC.down}", "${DSYNC.old}", "${DSYNC.newer}"]\n`,
);

export default defineConfig({
  testDir: "tests",
  globalSetup: "./global-setup.ts",
  forbidOnly: !!process.env.CI,
  retries: 0,
  workers: 1,
  reporter: [["list"]],
  use: { baseURL: `http://127.0.0.1:${port}` },
  projects: [{ name: "chromium", use: { browserName: "chromium" } }],
  webServer: {
    command: `${resolve(root, "target/debug/dcontroller")} ${config}`,
    url: `http://127.0.0.1:${port}/healthz`,
    reuseExistingServer: false,
    stdout: "pipe",
    stderr: "pipe",
  },
});
