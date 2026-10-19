// The phone page's service worker: shows what claude-remote pushes (decrypted by the browser), and
// a tap on it opens the session it is about.
"use strict";

self.addEventListener("push", (event) => {
  const notification = event.data ? event.data.json() : { title: "Claude sessions", body: "" };
  event.waitUntil(self.registration.showNotification(notification.title, {
    body: notification.body,
    tag: notification.topic,
    data: { session: notification.session || null },
  }));
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const session = event.notification.data && event.notification.data.session;
  event.waitUntil((async () => {
    const windows = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
    if (windows.length > 0) {
      windows[0].postMessage({ session });
      return windows[0].focus();
    }
    return self.clients.openWindow(session ? `/#session=${session}` : "/");
  })());
});
