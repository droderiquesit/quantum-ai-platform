self.addEventListener("install", function (event) {
  event.waitUntil(
    caches
      .open("readease")
      .then(function (cache) {
        return cache.addAll(["../../index.html", "../../home.html"]);
      })
      .catch((err) => console.log("SW Install cache fail:", err)),
  );
});

self.addEventListener("fetch", function (event) {
  event.respondWith(
    caches
      .match(event.request)
      .then(function (response) {
        return response || fetch(event.request);
      })
      .catch((err) => {
        console.log("SW Fetch fail:", err);
        return fetch(event.request);
      }),
  );
});

// Explicitly handle messages to prevent "message channel closed" error in extensions
self.addEventListener("message", (event) => {
  if (event.data && event.data.type === "SKIP_WAITING") {
    self.skipWaiting();
  }
});
