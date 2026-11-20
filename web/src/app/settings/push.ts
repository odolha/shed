/** Web Push in this browser: support, the service worker, subscribing. */

import type { NotificationKind, PushSubscribe } from '../../api/gen/api';

export const KINDS: NotificationKind[] = ['aiWaiting', 'aiDone', 'commandDone', 'workspaceOffline'];

export type PushSupport = 'ok' | 'insecure' | 'unsupported' | 'ios-install';

export function pushSupport(): PushSupport {
  if (!window.isSecureContext) return 'insecure';
  const ok = 'serviceWorker' in navigator && 'PushManager' in window && 'Notification' in window;
  if (ok) return 'ok';
  const ios = /iPhone|iPad|iPod/.test(navigator.userAgent);
  const standalone = matchMedia('(display-mode: standalone)').matches;
  return ios && !standalone ? 'ios-install' : 'unsupported';
}

/** A VAPID key (base64url) as the bytes `subscribe` wants. */
export function base64UrlToBytes(b64url: string): Uint8Array<ArrayBuffer> {
  const pad = '='.repeat((4 - (b64url.length % 4)) % 4);
  const b64 = (b64url + pad).replace(/-/g, '+').replace(/_/g, '/');
  const raw = atob(b64);
  const out = new Uint8Array(new ArrayBuffer(raw.length));
  for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i);
  return out;
}

export async function registration(): Promise<ServiceWorkerRegistration> {
  const existing = await navigator.serviceWorker.getRegistration('/');
  if (existing) return existing;
  return navigator.serviceWorker.register('/sw.js');
}

export async function currentSubscription(): Promise<PushSubscription | null> {
  if (pushSupport() !== 'ok') return null;
  const reg = await navigator.serviceWorker.getRegistration('/');
  return reg ? reg.pushManager.getSubscription() : null;
}

/** Asks for permission and subscribes; what to send the nexus. */
export async function subscribe(vapidKey: string): Promise<PushSubscribe> {
  const permission = await Notification.requestPermission();
  if (permission !== 'granted') throw new Error('denied');
  const reg = await registration();
  const key = base64UrlToBytes(vapidKey);
  let sub = await reg.pushManager.getSubscription();
  if (sub) {
    // Made for another key (the nexus's keys changed): start again.
    const old = sub.options.applicationServerKey;
    if (old && !sameBytes(new Uint8Array(old), key)) {
      await sub.unsubscribe();
      sub = null;
    }
  }
  sub ??= await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: key });
  const json = sub.toJSON();
  return { endpoint: sub.endpoint, p256dh: json.keys?.['p256dh'] ?? '', auth: json.keys?.['auth'] ?? '' };
}

function sameBytes(a: Uint8Array, b: Uint8Array): boolean {
  return a.length === b.length && a.every((v, i) => v === b[i]);
}
