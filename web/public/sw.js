// Shed's service worker: shows push notifications and opens the tab they are
// about when clicked. It caches nothing; the app needs the nexus anyway.
//
// A push's payload is a `PushPayload` (crates/shed-api/src/http.rs: kind,
// title, body, workspace, workspaceName, bench, tab), the workspace's
// `Notification` with the workspace's id and name added by the nexus.

/* eslint-env serviceworker */

self.addEventListener('install', () => {
  self.skipWaiting();
});

self.addEventListener('activate', (event) => {
  event.waitUntil(self.clients.claim());
});

function readPayload(data) {
  if (!data) return {};
  try {
    const json = data.json();
    return json && typeof json === 'object' ? json : { title: String(json) };
  } catch {
    try {
      return { title: data.text() };
    } catch {
      return {};
    }
  }
}

/** Where a notification leads: its tab when all is known, else the overview. */
function urlFor(p) {
  const seg = (s) => encodeURIComponent(String(s));
  if (p.workspace && p.bench && p.tab) return `/w/${seg(p.workspace)}/b/${seg(p.bench)}/t/${seg(p.tab)}`;
  if (p.workspace && p.bench) return `/w/${seg(p.workspace)}/b/${seg(p.bench)}`;
  if (p.workspace) return `/w/${seg(p.workspace)}`;
  return '/';
}

self.addEventListener('push', (event) => {
  const p = readPayload(event.data);
  const title = typeof p.title === 'string' && p.title ? p.title : 'Shed';
  const kind = typeof p.kind === 'string' ? p.kind : 'shed';
  event.waitUntil(
    self.registration.showNotification(title, {
      body: typeof p.body === 'string' ? p.body : '',
      icon: '/icons/shed-192.png',
      badge: '/icons/shed-badge-96.png',
      // One notification per tab and kind: a newer one replaces it.
      tag: `${kind}:${p.tab || p.workspace || ''}`,
      renotify: kind === 'aiWaiting',
      // A workspace gone offline is minor: no sound, no buzz.
      silent: kind === 'workspaceOffline',
      data: { url: urlFor(p) },
    }),
  );
});

self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  const url = new URL((event.notification.data && event.notification.data.url) || '/', self.location.origin).href;
  event.waitUntil(
    (async () => {
      const windows = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
      const own = windows.filter((c) => new URL(c.url).origin === self.location.origin);
      const client = own.find((c) => c.focused) || own[0];
      if (client) {
        await client.focus();
        if ('navigate' in client) {
          try {
            await client.navigate(url);
            return;
          } catch {
            // Not controlled by this worker yet: tell the page instead.
          }
        }
        client.postMessage({ type: 'shed-open', url });
        return;
      }
      await self.clients.openWindow(url);
    })(),
  );
});
