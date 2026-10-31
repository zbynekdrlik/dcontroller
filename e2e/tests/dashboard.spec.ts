import { test, expect, type ConsoleMessage } from "@playwright/test";

const VERSION_RE = /^v\d+\.\d+\.\d+(-dev\.\d+)?(\s\([0-9a-f]{7}\))?$/;

test.describe("dashboard", () => {
  let console_msgs: string[];

  test.beforeEach(async ({ page }) => {
    console_msgs = [];
    page.on("console", (m: ConsoleMessage) => {
      if (m.type() === "error" || m.type() === "warning") console_msgs.push(`${m.type()}: ${m.text()}`);
    });
    page.on("pageerror", (e) => console_msgs.push(`pageerror: ${e.message}`));
  });

  test("shows the build version, same as the server's /api/version", async ({ page }) => {
    await page.goto("/");
    const label = page.getByTestId("version");
    await expect(label).toBeVisible();
    const shown = (await label.textContent())?.trim() ?? "";
    expect(shown).toMatch(VERSION_RE);
    const api = await page.evaluate(() => fetch("/api/version").then((r) => r.text()));
    expect(shown).toBe(api.trim());
    expect(console_msgs).toEqual([]);
  });

  test("version label is visible on a client-side route too", async ({ page }) => {
    await page.goto("/devices");
    await expect(page.getByTestId("version")).toHaveText(VERSION_RE);
    expect(console_msgs).toEqual([]);
  });

  test("is an installable PWA: manifest and active service worker", async ({ page }) => {
    await page.goto("/");
    const href = await page.locator('link[rel="manifest"]').getAttribute("href");
    expect(href).toBe("/manifest.webmanifest");
    const manifest = await page.evaluate((u) => fetch(u).then((r) => r.json()), href!);
    expect(manifest.name).toBe("dcontroller");
    expect(manifest.display).toBe("standalone");
    expect(manifest.start_url).toBe("/");
    expect(manifest.icons.length).toBeGreaterThan(0);
    const scope = await page.evaluate(async () => (await navigator.serviceWorker.ready).scope);
    expect(scope).toBe(new URL("/", page.url()).href);
    expect(console_msgs).toEqual([]);
  });
});
