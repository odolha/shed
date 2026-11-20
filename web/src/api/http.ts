/**
 * The nexus's HTTP API (`crates/shed-api/src/http.rs`): the account, signing
 * in, devices, pairing and push. A browser signs in with a session cookie;
 * changes carry `X-Shed-Csrf` with the token `SignedIn` gave. That token is
 * kept in memory only: after a reload it comes again from `GET /session`.
 */

import { Injectable, signal } from '@angular/core';
import type {
  ApiError,
  AuditEntry,
  ChangePassword,
  Device,
  Login,
  NexusStatus,
  PairApprove,
  PairReject,
  PendingPairing,
  PushInfo,
  PushPrefs,
  PushSubscribe,
  RenameDevice,
  Revoke,
  SetupConfirm,
  SetupStart,
  SetupTotp,
  SignedIn,
} from './gen/api';
import { ApiFailure } from './errors';

const BASE = '/api/v1';

type Empty = Record<string, never>;

/** Calls made before signing in, or that ask for a password or code again. */
const OPEN = ['/status', '/login', '/setup', '/setup/confirm'];

/** The nexus refused a change for its CSRF token (lost or from another sign-in). */
function csrfRefused(f: ApiFailure): boolean {
  return f.code === 'forbidden' && /csrf/i.test(f.message);
}

@Injectable({ providedIn: 'root' })
export class NexusHttp {
  /** The CSRF token for changes, for this page only. */
  private readonly csrf = signal<string | null>(null);
  /** A `GET /session` under way, so changes at once wait for one answer. */
  private recovering: Promise<string | null> | null = null;
  /** Called when the nexus says we are not signed in (any call). */
  onUnauthorized: (() => void) | null = null;

  constructor() {
    // Earlier versions kept the token across reloads; it is fetched now.
    try {
      localStorage.removeItem('shed.csrf');
    } catch {
      // Private mode.
    }
  }

  setCsrf(token: string | null | undefined): void {
    this.csrf.set(token ?? null);
  }

  hasCsrf(): boolean {
    return this.csrf() !== null;
  }

  status(): Promise<NexusStatus> {
    return this.get('/status');
  }

  /** Who is signed in, with the session's CSRF token (after a reload). */
  async session(): Promise<SignedIn> {
    const s = await this.get<SignedIn>('/session');
    if (s.csrf) this.setCsrf(s.csrf);
    return s;
  }

  setup(body: SetupStart): Promise<SetupTotp> {
    return this.post('/setup', body);
  }

  setupConfirm(body: SetupConfirm): Promise<SignedIn> {
    return this.post('/setup/confirm', body);
  }

  login(body: Login): Promise<SignedIn> {
    return this.post('/login', body);
  }

  logout(): Promise<Empty> {
    return this.post('/logout', {});
  }

  devices(): Promise<Device[]> {
    return this.get('/devices');
  }

  revoke(body: Revoke): Promise<Empty> {
    return this.post('/devices/revoke', body);
  }

  renameDevice(body: RenameDevice): Promise<Empty> {
    return this.post('/devices/rename', body);
  }

  changePassword(body: ChangePassword): Promise<Empty> {
    return this.post('/password', body);
  }

  pendingPairings(): Promise<PendingPairing[]> {
    return this.get('/pair/pending');
  }

  approvePairing(body: PairApprove): Promise<Empty> {
    return this.post('/pair/approve', body);
  }

  rejectPairing(body: PairReject): Promise<Empty> {
    return this.post('/pair/reject', body);
  }

  push(): Promise<PushInfo> {
    return this.get('/push');
  }

  pushSubscribe(body: PushSubscribe): Promise<Empty> {
    return this.post('/push/subscribe', body);
  }

  pushUnsubscribe(): Promise<Empty> {
    return this.post('/push/unsubscribe', {});
  }

  pushPrefs(body: PushPrefs): Promise<Empty> {
    return this.post('/push/prefs', body);
  }

  audit(): Promise<AuditEntry[]> {
    return this.get('/audit');
  }

  private get<T>(path: string): Promise<T> {
    return this.send<T>('GET', path);
  }

  private post<T>(path: string, body: unknown): Promise<T> {
    return this.send<T>('POST', path, body);
  }

  private async send<T>(method: 'GET' | 'POST', path: string, body?: unknown): Promise<T> {
    const change = method !== 'GET' && !OPEN.includes(path);
    if (change && this.csrf() === null) await this.recoverCsrf();
    try {
      return await this.sendOnce<T>(method, path, body);
    } catch (e) {
      // A token from before (another tab signed in again): fetch it and try once more.
      if (!change || !(e instanceof ApiFailure) || !csrfRefused(e)) throw e;
      this.csrf.set(null);
      if ((await this.recoverCsrf()) === null) throw e;
      return this.sendOnce<T>(method, path, body);
    }
  }

  /** The session's CSRF token again, or null when the nexus does not say. */
  private recoverCsrf(): Promise<string | null> {
    this.recovering ??= this.session()
      .then((s) => s.csrf ?? null)
      .catch(() => null)
      .finally(() => (this.recovering = null));
    return this.recovering;
  }

  private async sendOnce<T>(method: 'GET' | 'POST', path: string, body?: unknown): Promise<T> {
    const headers: Record<string, string> = { Accept: 'application/json' };
    if (body !== undefined) headers['Content-Type'] = 'application/json';
    const csrf = this.csrf();
    if (method !== 'GET' && csrf) headers['X-Shed-Csrf'] = csrf;
    let res: Response;
    try {
      res = await fetch(BASE + path, {
        method,
        headers,
        credentials: 'same-origin',
        body: body === undefined ? undefined : JSON.stringify(body),
      });
    } catch (e) {
      throw new ApiFailure('offline', e instanceof Error ? e.message : String(e));
    }
    const text = await res.text();
    let json: unknown = undefined;
    if (text) {
      try {
        json = JSON.parse(text);
      } catch {
        json = undefined;
      }
    }
    if (!res.ok) {
      const err = json as Partial<ApiError> | undefined;
      const failure =
        err && typeof err.code === 'string'
          ? ApiFailure.from(err as ApiError, res.status)
          : new ApiFailure(statusCode(res.status), text.slice(0, 200), res.status);
      // A wrong password or code where one is asked again is not a lost session.
      const asks = path === '/login' || path === '/password' || path === '/devices/revoke' || path.startsWith('/setup');
      if (failure.code === 'unauthorized' && !asks) {
        this.onUnauthorized?.();
      }
      throw failure;
    }
    return (json ?? {}) as T;
  }
}

function statusCode(status: number): ApiError['code'] {
  switch (status) {
    case 401:
      return 'unauthorized';
    case 403:
      return 'forbidden';
    case 404:
      return 'notFound';
    case 429:
      return 'rateLimited';
    case 400:
    case 422:
      return 'invalid';
    case 502:
    case 503:
      return 'offline';
    case 504:
      return 'timeout';
    default:
      return 'failed';
  }
}

/** "Firefox on Linux": how this browser is named in the device list. */
export function browserName(ua = typeof navigator === 'undefined' ? '' : navigator.userAgent): string {
  const browser = /Edg\//.test(ua)
    ? 'Edge'
    : /OPR\//.test(ua)
      ? 'Opera'
      : /Firefox\//.test(ua)
        ? 'Firefox'
        : /Chrome\//.test(ua)
          ? /Brave/.test(ua)
            ? 'Brave'
            : 'Chrome'
          : /Safari\//.test(ua)
            ? 'Safari'
            : 'Browser';
  const os = /Android/.test(ua)
    ? 'Android'
    : /iPhone|iPad|iPod/.test(ua)
      ? 'iOS'
      : /Mac OS X|Macintosh/.test(ua)
        ? 'macOS'
        : /Windows/.test(ua)
          ? 'Windows'
          : /CrOS/.test(ua)
            ? 'ChromeOS'
            : /Linux/.test(ua)
              ? 'Linux'
              : '';
  return os ? `${browser} on ${os}` : browser;
}
