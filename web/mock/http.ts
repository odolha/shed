// The nexus's HTTP API (crates/shed-api/src/http.rs): account, sign-in,
// devices, pairing, push and the audit log, kept in memory.

import { createECDH, randomBytes } from 'node:crypto';
import type { IncomingMessage, ServerResponse } from 'node:http';
import type {
  AuditEntry,
  ChangePassword,
  Device,
  Login,
  NotificationKind,
  PairApprove,
  PairReject,
  PendingPairing,
  PushPrefs,
  PushSubscribe,
  RenameDevice,
  Revoke,
  SetupConfirm,
  SetupStart,
  SignedIn,
} from '../src/api/gen/api.ts';
import { Fail, env, hex, now } from './util.ts';
import { addWorkspace } from './world.ts';

export interface Session {
  token: string;
  csrf: string;
  device: string;
}

const account = {
  ready: !env.fresh,
  username: 'you',
  password: 'shed',
  /** Set between the two setup steps. */
  secret: '',
};

export const setupCode = hex(8);
export const sessions = new Map<string, Session>();
/** Ends a session's sockets (revoked, signed out). */
export let onSessionEnd: (token: string) => void = () => {};
export function setOnSessionEnd(fn: (token: string) => void): void {
  onSessionEnd = fn;
}

const t0 = now();
const devices: Device[] = [
  { id: 'd-phone', kind: 'phone', name: 'Pixel 8', created: t0 - 40 * 86400, seen: t0 - 1800, address: '203.0.113.24', country: 'NL', current: false, online: false },
  { id: 'ws-devbox', kind: 'workspace', name: 'devbox', created: t0 - 60 * 86400, seen: t0, address: '198.51.100.7', country: 'NL', current: false, online: true },
  { id: 'ws-laptop', kind: 'workspace', name: 'laptop', created: t0 - 20 * 86400, seen: t0 - 7200, address: '203.0.113.80', country: 'DE', current: false, online: false },
];

const pending: PendingPairing[] = [
  { code: 'K7QF-2M9D', id: 'p-buildbox', name: 'buildbox', version: '0.1.0', address: '192.0.2.15', country: 'NL', expires: t0 + 15 * 60 },
];

const audit: AuditEntry[] = [
  { at: t0 - 86400 * 3, what: 'pair', device: 'ws-laptop', address: '203.0.113.80', country: 'DE', detail: 'laptop' },
  { at: t0 - 86400, what: 'login-failed', device: null, address: '192.0.2.99', country: 'US', detail: 'wrong password' },
  { at: t0 - 7200, what: 'login', device: 'd-phone', address: '203.0.113.24', country: 'NL', detail: '' },
];

const ecdh = createECDH('prime256v1');
ecdh.generateKeys();
const vapidKey = ecdh.getPublicKey().toString('base64url');
const pushPrefs = new Map<string, NotificationKind[]>();
const pushSubs = new Map<string, PushSubscribe>();

let failures: number[] = [];

function note(what: string, req: IncomingMessage, device: string | null, detail = ''): void {
  audit.unshift({ at: now(), what, device, address: address(req), country: null, detail });
}

function address(req: IncomingMessage): string {
  return req.socket.remoteAddress?.replace(/^::ffff:/, '') ?? '';
}

function totpOk(code: string): void {
  const c = code.replace(/\s/g, '');
  if (/^\d{6}$/.test(c) && c !== '000000') return;
  if (/^[a-z0-9]{4}-[a-z0-9]{4}$/i.test(c)) return; // a recovery code
  throw new Fail('wrongCode', 'that code is not right');
}

export function cookies(req: IncomingMessage): Record<string, string> {
  const out: Record<string, string> = {};
  for (const part of (req.headers.cookie ?? '').split(';')) {
    const i = part.indexOf('=');
    if (i > 0) out[part.slice(0, i).trim()] = decodeURIComponent(part.slice(i + 1).trim());
  }
  return out;
}

export function sessionOf(req: IncomingMessage): Session | null {
  const token = cookies(req)['shed_session'];
  return (token && sessions.get(token)) || null;
}

function signIn(res: ServerResponse, kind: Device['kind'], name: string, req: IncomingMessage, recovery: string[] = []): SignedIn {
  const device: Device = { id: `d-${hex(4)}`, kind, name, created: now(), seen: now(), address: address(req), country: null, current: false, online: true };
  devices.unshift(device);
  const token = hex(24);
  const csrf = hex(16);
  sessions.set(token, { token, csrf, device: device.id });
  res.setHeader('Set-Cookie', `shed_session=${token}; HttpOnly; SameSite=Strict; Path=/; Max-Age=${7 * 86400}`);
  return { username: account.username, device: device.id, token: kind === 'phone' ? token : null, csrf, recoveryCodes: recovery };
}

const STATUS: Record<string, number> = {
  unauthorized: 401,
  forbidden: 403,
  notFound: 404,
  offline: 503,
  invalid: 400,
  unsupported: 400,
  rateLimited: 429,
  wrongCode: 400,
  timeout: 504,
  failed: 500,
};

function send(res: ServerResponse, status: number, body: unknown): void {
  res.writeHead(status, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
  res.end(JSON.stringify(body));
}

async function body<T>(req: IncomingMessage): Promise<T> {
  const chunks: Buffer[] = [];
  for await (const c of req) chunks.push(c as Buffer);
  const text = Buffer.concat(chunks).toString('utf8');
  if (!text) return {} as T;
  try {
    return JSON.parse(text) as T;
  } catch {
    throw new Fail('invalid', 'the body is not JSON');
  }
}

/** Handles `/api/v1/…`; resolves when answered. */
export async function handleHttp(req: IncomingMessage, res: ServerResponse): Promise<void> {
  const url = new URL(req.url ?? '/', 'http://mock');
  const route = `${req.method} ${url.pathname.replace(/^\/api\/v1/, '')}`;
  try {
    if (env.latency) await new Promise((r) => setTimeout(r, env.latency));
    const out = await answer(route, req, res);
    send(res, 200, out ?? {});
  } catch (e) {
    const f = e instanceof Fail ? e : new Fail('failed', e instanceof Error ? e.message : String(e));
    send(res, STATUS[f.code] ?? 500, { code: f.code, message: f.message });
  }
}

async function answer(route: string, req: IncomingMessage, res: ServerResponse): Promise<unknown> {
  const session = sessionOf(req);
  const open = ['GET /status', 'POST /setup', 'POST /setup/confirm', 'POST /login', 'POST /pair/start', 'POST /pair/poll'];
  if (!open.includes(route)) {
    if (!session) throw new Fail('unauthorized', 'not signed in');
    const d = devices.find((x) => x.id === session.device);
    if (d) d.seen = now();
    if (req.method === 'POST' && req.headers['x-shed-csrf'] !== session.csrf) throw new Fail('forbidden', 'missing or wrong X-Shed-Csrf');
  }
  switch (route) {
    case 'GET /status':
      return { version: '0.1.0-mock', api: 1, ready: account.ready, signedIn: !!session };

    case 'POST /setup': {
      const b = await body<SetupStart>(req);
      if (account.ready) throw new Fail('forbidden', 'the account is set up already');
      if (b.code !== setupCode) throw new Fail('wrongCode', 'that setup code is not right');
      if (!b.username?.trim() || (b.password ?? '').length < 8) throw new Fail('invalid', 'a user name and a password of 8 or more characters');
      account.username = b.username.trim();
      account.password = b.password;
      account.secret = Array.from(randomBytes(32), (b) => 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567'[b % 32]).join('');
      return { secret: account.secret, url: `otpauth://totp/Shed:${encodeURIComponent(account.username)}?secret=${account.secret}&issuer=Shed` };
    }
    case 'POST /setup/confirm': {
      const b = await body<SetupConfirm>(req);
      if (account.ready) throw new Fail('forbidden', 'the account is set up already');
      if (b.code !== setupCode || !account.secret) throw new Fail('wrongCode', 'that setup code is not right');
      totpOk(b.totp);
      account.ready = true;
      const recovery = Array.from({ length: 8 }, () => `${hex(2)}-${hex(2)}`);
      note('setup', req, null);
      return signIn(res, b.device?.kind ?? 'browser', b.device?.name ?? 'Browser', req, recovery);
    }
    case 'POST /login': {
      const b = await body<Login>(req);
      if (!account.ready) throw new Fail('forbidden', 'the account is not set up');
      failures = failures.filter((t) => t > Date.now() - 60_000);
      if (failures.length >= 5) throw new Fail('rateLimited', 'too many attempts; wait a minute');
      if (b.username !== account.username || b.password !== account.password) {
        failures.push(Date.now());
        note('login-failed', req, null, 'wrong user name or password');
        throw new Fail('unauthorized', 'wrong user name or password');
      }
      try {
        totpOk(b.totp ?? '');
      } catch (e) {
        failures.push(Date.now());
        note('login-failed', req, null, 'wrong code');
        throw e;
      }
      const out = signIn(res, b.device?.kind ?? 'browser', b.device?.name ?? 'Browser', req);
      note('login', req, out.device);
      return out;
    }
    case 'GET /session':
      // A browser that reloaded gets its session's CSRF token again.
      return { username: account.username, device: session!.device, token: null, csrf: session!.csrf, recoveryCodes: [] } satisfies SignedIn;
    case 'POST /logout': {
      sessions.delete(session!.token);
      const i = devices.findIndex((d) => d.id === session!.device);
      if (i >= 0) devices.splice(i, 1);
      res.setHeader('Set-Cookie', 'shed_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0');
      onSessionEnd(session!.token);
      note('logout', req, session!.device);
      return {};
    }

    case 'GET /devices':
      return devices.map((d) => ({ ...d, current: d.id === session!.device, online: d.id === session!.device || d.online }));
    case 'POST /devices/revoke': {
      const b = await body<Revoke>(req);
      totpOk(b.totp);
      const i = devices.findIndex((d) => d.id === b.device);
      if (i < 0) throw new Fail('notFound', 'no such device');
      devices.splice(i, 1);
      for (const s of [...sessions.values()]) {
        if (s.device === b.device) {
          sessions.delete(s.token);
          onSessionEnd(s.token);
        }
      }
      note('revoke', req, session!.device, b.device);
      return {};
    }
    case 'POST /devices/rename': {
      const b = await body<RenameDevice>(req);
      const d = devices.find((x) => x.id === b.device);
      if (!d) throw new Fail('notFound', 'no such device');
      if (!b.name?.trim()) throw new Fail('invalid', 'a device needs a name');
      d.name = b.name.trim();
      return {};
    }
    case 'POST /password': {
      const b = await body<ChangePassword>(req);
      if (b.old !== account.password) throw new Fail('unauthorized', 'the current password is not right');
      totpOk(b.totp);
      if ((b.new ?? '').length < 8) throw new Fail('invalid', 'a password of 8 or more characters');
      account.password = b.new;
      for (const s of [...sessions.values()]) {
        if (s.token !== session!.token) {
          sessions.delete(s.token);
          onSessionEnd(s.token);
        }
      }
      note('password', req, session!.device);
      return {};
    }

    case 'GET /pair/pending':
      // The list is for information: the code must be typed from the workspace.
      return pending.filter((p) => p.expires > now()).map((p) => ({ ...p, code: '' }));
    case 'POST /pair/approve': {
      const b = await body<PairApprove>(req);
      totpOk(b.totp);
      const i = pending.findIndex((p) => sameCode(p.code, b.code));
      if (i < 0) throw new Fail('notFound', 'no such pairing request');
      const p = pending.splice(i, 1)[0]!;
      const name = b.name?.trim() || p.name;
      devices.push({ id: `ws-${name}`, kind: 'workspace', name, created: now(), seen: now(), address: p.address, country: p.country, current: false, online: false });
      addWorkspace(name);
      note('pair', req, session!.device, name);
      return {};
    }
    case 'POST /pair/reject': {
      const b = await body<PairReject>(req);
      const i = pending.findIndex((p) => (b.id ? p.id === b.id : sameCode(p.code, b.code ?? '')));
      if (i < 0) throw new Fail('notFound', 'no such pairing request');
      const p = pending.splice(i, 1)[0]!;
      note('pair-rejected', req, session!.device, p.name);
      return {};
    }
    case 'POST /pair/start':
    case 'POST /pair/poll':
      throw new Fail('unsupported', 'the mock has no workspaces to pair');

    case 'GET /push':
      return { vapidKey, prefs: { kinds: pushPrefs.get(session!.device) ?? ['aiWaiting', 'aiDone'] }, subscribed: pushSubs.has(session!.device) };
    case 'POST /push/subscribe': {
      const b = await body<PushSubscribe>(req);
      if (!b.endpoint || !b.p256dh || !b.auth) throw new Fail('invalid', 'endpoint, p256dh and auth are needed');
      pushSubs.set(session!.device, b);
      console.log(`push: ${session!.device} subscribed (${new URL(b.endpoint).host})`);
      return {};
    }
    case 'POST /push/unsubscribe':
      pushSubs.delete(session!.device);
      console.log(`push: ${session!.device} unsubscribed`);
      return {};
    case 'POST /push/prefs': {
      const b = await body<PushPrefs>(req);
      pushPrefs.set(session!.device, b.kinds ?? []);
      return {};
    }
    case 'GET /audit':
      return audit.slice(0, 100);
  }
  throw new Fail('notFound', `no route ${route}`);
}

/** Pairing codes compare without case or dashes: `k7qf2m9d` is `K7QF-2M9D`. */
function sameCode(a: string, b: string): boolean {
  const bare = (c: string) => c.replace(/[\s-]/g, '').toUpperCase();
  return bare(a) !== '' && bare(a) === bare(b);
}
