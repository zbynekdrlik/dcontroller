import { defineConfig } from "@playwright/test";
import { writeFileSync, mkdirSync } from "node:fs";
import { resolve } from "node:path";

// The real server binary serves the real trunk build; nothing is mocked.
const root = resolve(__dirname, "..");
const port = 18080;
// Not under test-results: Playwright empties that directory when it starts.
mkdirSync(resolve(__dirname, ".run"), { recursive: true });
const config = resolve(__dirname, ".run", "config.toml");
writeFileSync(config, `bind = "127.0.0.1:${port}"\nweb_dir = "${resolve(root, "crates/web/dist")}"\n`);

export default defineConfig({
  testDir: "tests",
  forbidOnly: !!process.env.CI,
  retries: 0,
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
