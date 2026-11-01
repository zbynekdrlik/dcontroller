import { defineConfig } from "@playwright/test";
import { rmSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { root, routed, run } from "./env";

// The real server binary serves the real trunk build and discovers a real
// Dante device (global-setup.ts); nothing is mocked.
const port = 18080;
const config = resolve(run, "config.toml");
const changes = resolve(run, "changes.jsonl");
rmSync(changes, { force: true });
writeFileSync(
  config,
  `bind = "127.0.0.1:${port}"\nweb_dir = "${resolve(root, "crates/web/dist")}"\ndante_interface = "${routed.iface}"\nchanges_log = "${changes}"\n`,
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
