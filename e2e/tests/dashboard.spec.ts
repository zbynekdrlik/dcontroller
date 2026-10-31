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

  test("service worker serves the shell offline and never caches /api", async ({ page, context }) => {
    await page.goto("/");
    await page.evaluate(() => navigator.serviceWorker.ready);
    // Second load goes through the now-controlling worker, which fills the cache.
    await page.reload();
    await expect(page.getByTestId("version")).toHaveText(VERSION_RE);
    expect(await page.evaluate(() => navigator.serviceWorker.controller !== null)).toBe(true);
    await page.evaluate(() => fetch("/api/version").then((r) => r.text()));
    const online = (await page.getByTestId("version").textContent())?.trim();

    await context.setOffline(true);
    await page.reload();
    await expect(page.getByTestId("version")).toHaveText(online!);
    const cachedPaths = await page.evaluate(async () => {
      const c = await caches.open("dcontroller-v1");
      return (await c.keys()).map((r) => new URL(r.url).pathname);
    });
    expect(cachedPaths).toContain("/");
    expect(cachedPaths.filter((p) => p.startsWith("/api/"))).toEqual([]);
    await context.setOffline(false);
    expect(console_msgs).toEqual([]);
  });
});
