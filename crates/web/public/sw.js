// dcontroller service worker: network-first, so a deploy is visible at once.
// The cache answers only when the network is down or the tunnel/origin fails
// (5xx), and never for /api/: device state must not be shown stale.
const CACHE = "dcontroller-v1";

self.addEventListener("install", () => self.skipWaiting());

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches.keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  const req = event.request;
  const url = new URL(req.url);
  if (req.method !== "GET" || url.origin !== self.location.origin) return;
  if (url.pathname.startsWith("/api/")) return; // straight to the network, never cached
  const cached = () => caches.match(req);
  event.respondWith(
    fetch(req)
      .then((res) => {
        // Only our own direct answers: a redirect (e.g. Cloudflare Access login
        // after a session expiry) must not be stored under the asset's URL.
        if (res.ok && res.type === "basic" && !res.redirected) {
          const copy = res.clone();
          caches.open(CACHE).then((c) => c.put(req, copy));
          return res;
        }
        if (res.status >= 500) return cached().then((hit) => hit || res);
        return res;
      })
      .catch(() => cached().then((hit) => hit || Response.error())),
  );
});
