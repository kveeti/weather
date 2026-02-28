"use strict";

self.addEventListener("install", (event) => {
  self.skipWaiting();
});

self.addEventListener("activate", (event) => {
  event.waitUntil(clients.claim());
});

self.addEventListener("push", (event) => {
  console.log("push", event);
  let text = "Weather alert";
  if (event.data) {
    try {
      text = event.data.text();
    } catch (e) {
      text = "Weather update";
    }
  }

  const options = {
    body: text,
    icon: "/static/icon-192.png",
    badge: "/static/icon-192.png",
    tag: "weather",
    renotify: true,
    requireInteraction: false,
    data: { url: "/" },
  };

  event.waitUntil(
    self.registration.showNotification("Weather", options)
      .then(() => console.log("[SW] notification shown"))
      .catch((err) => console.error("[SW] showNotification failed:", err)),
  );
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const url = (event.notification.data && event.notification.data.url) || "/";
  event.waitUntil(
    clients
      .matchAll({ type: "window", includeUncontrolled: true })
      .then((windowClients) => {
        for (const client of windowClients) {
          if (client.url.includes(self.location.origin) && "focus" in client) {
            return client.focus();
          }
        }
        if (clients.openWindow) {
          return clients.openWindow(url);
        }
      }),
  );
});
