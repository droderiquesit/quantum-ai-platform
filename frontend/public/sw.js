/**
 * The service worker, and the one thing it must never do.
 *
 * It caches the application shell so the installed app opens on a phone with no
 * connection — and it caches **no answer the platform ever gave**. Every
 * request under `/api/` is passed straight through untouched, so a position, a
 * fill, a limit or a halt state can only ever be shown when it was fetched just
 * now. A cached book rendered offline would look exactly like a live one, and
 * that is the single most dangerous thing a trading surface can do.
 *
 * The consequence is deliberate: opened with no connection, the app renders its
 * chrome, says the platform is unreachable, and shows no figures at all.
 */

// Bumping this evicts every previous cache on activate. The build's own asset
// hashes handle staleness within a version; this handles the worker itself.
const VERSION = "qip-shell-v1";
const SHELL = `${VERSION}-shell`;
const ASSETS = `${VERSION}-assets`;

/** Rendered when a navigation is attempted with no network. */
const OFFLINE_URL = "/offline";

/** What is fetched at install so a cold launch offline has something to draw. */
const PRECACHE = [
  OFFLINE_URL,
  "/manifest.webmanifest",
  "/icons/icon-192.png",
  "/icons/icon-512.png",
  "/icons/maskable-192.png",
  "/icons/maskable-512.png",
  "/icons/apple-touch-icon.png",
];

self.addEventListener("install", (event) => {
  event.waitUntil(
    (async () => {
      const cache = await caches.open(SHELL);
      // Individually, not addAll: one 404 in the list must not abandon the
      // whole install and leave the app with no offline page at all.
      await Promise.all(
        PRECACHE.map(async (url) => {
          try {
            const response = await fetch(url, { cache: "reload" });
            if (response.ok) await cache.put(url, response);
          } catch {
            /* left uncached; the fetch handler degrades to the network */
          }
        }),
      );
      await self.skipWaiting();
    })(),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    (async () => {
      const names = await caches.keys();
      await Promise.all(
        names.filter((name) => !name.startsWith(VERSION)).map((name) => caches.delete(name)),
      );
      await self.clients.claim();
    })(),
  );
});

/** True for anything carrying platform data. None of it may be stored. */
function isPlatformData(url) {
  return url.pathname === "/api" || url.pathname.startsWith("/api/");
}

/** True for build output, which is content-hashed and safe to keep. */
function isBuildAsset(url) {
  return (
    url.pathname.startsWith("/_next/static/") ||
    url.pathname.startsWith("/icons/") ||
    url.pathname === "/manifest.webmanifest"
  );
}

self.addEventListener("fetch", (event) => {
  const request = event.request;
  if (request.method !== "GET") return;

  const url = new URL(request.url);
  if (url.origin !== self.location.origin) return;

  // The rule this file exists to hold. Not intercepted at all: no cache read,
  // no cache write, no synthesised response. Offline, these fail, and the
  // console renders its own "the platform could not be reached" state.
  if (isPlatformData(url)) return;

  if (request.mode === "navigate") {
    event.respondWith(
      (async () => {
        try {
          return await fetch(request);
        } catch {
          const cache = await caches.open(SHELL);
          const offline = await cache.match(OFFLINE_URL);
          if (offline) return offline;
          return new Response(
            "The console is offline and its offline page was never cached.",
            { status: 503, headers: { "content-type": "text/plain; charset=utf-8" } },
          );
        }
      })(),
    );
    return;
  }

  if (!isBuildAsset(url)) return;

  event.respondWith(
    (async () => {
      const cache = await caches.open(ASSETS);
      const hit = await cache.match(request);
      if (hit) return hit;
      const response = await fetch(request);
      if (response.ok) await cache.put(request, response.clone());
      return response;
    })(),
  );
});
