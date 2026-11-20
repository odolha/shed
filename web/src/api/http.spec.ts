import { NexusHttp } from './http';

interface Sent {
  method: string;
  path: string;
  csrf: string | null;
}

/** A pretend nexus behind `fetch`: its session's CSRF token is `good`. */
function fakeNexus(token = 'good') {
  const sent: Sent[] = [];
  const fetchFn = vi.fn(async (url: string, init: RequestInit) => {
    const headers = (init.headers ?? {}) as Record<string, string>;
    const method = init.method ?? 'GET';
    const path = url.replace('/api/v1', '');
    sent.push({ method, path, csrf: headers['X-Shed-Csrf'] ?? null });
    const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status });
    if (path === '/session') return json(200, { username: 'you', device: 'd-1', csrf: token, recoveryCodes: [] });
    if (method === 'POST' && headers['X-Shed-Csrf'] !== token) {
      return json(403, { code: 'forbidden', message: 'missing or wrong CSRF token' });
    }
    return json(200, {});
  });
  return { sent, fetchFn };
}

describe('NexusHttp and the CSRF token', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('asks for the session before the first change after a reload', async () => {
    const nexus = fakeNexus();
    vi.stubGlobal('fetch', nexus.fetchFn);
    const http = new NexusHttp();
    await http.pushPrefs({ kinds: ['aiDone'] });
    expect(nexus.sent).toEqual([
      { method: 'GET', path: '/session', csrf: null },
      { method: 'POST', path: '/push/prefs', csrf: 'good' },
    ]);
    // Known now: no more asking.
    await http.pushUnsubscribe();
    expect(nexus.sent.at(-1)).toEqual({ method: 'POST', path: '/push/unsubscribe', csrf: 'good' });
    expect(nexus.sent).toHaveLength(3);
  });

  it('fetches the token again once when the nexus refuses an old one', async () => {
    const nexus = fakeNexus();
    vi.stubGlobal('fetch', nexus.fetchFn);
    const http = new NexusHttp();
    http.setCsrf('stale');
    await http.rejectPairing({ id: 'p-1' });
    expect(nexus.sent.map((s) => `${s.method} ${s.path} ${s.csrf}`)).toEqual([
      'POST /pair/reject stale',
      'GET /session null',
      'POST /pair/reject good',
    ]);
  });

  it('signing in needs no token', async () => {
    const nexus = fakeNexus();
    vi.stubGlobal('fetch', nexus.fetchFn);
    const http = new NexusHttp();
    await expect(http.login({ username: 'you', password: 'x', totp: '1', device: { kind: 'browser', name: 'b' } })).rejects.toThrow();
    expect(nexus.sent.map((s) => s.path)).toEqual(['/login']);
  });
});
