import type { ClientFrame, ServerFrame, WorkspaceInfo } from './gen/api';
import { ApiFailure } from './errors';
import { SocketClient, type SocketLike } from './socket';

/** A WebSocket the test drives: it records what is sent and plays frames back. */
class FakeSocket implements SocketLike {
  readyState = 0;
  sent: ClientFrame[] = [];
  closed = false;
  onopen: ((ev: unknown) => void) | null = null;
  onclose: ((ev: unknown) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;

  send(data: string): void {
    this.sent.push(JSON.parse(data) as ClientFrame);
  }

  close(): void {
    this.closed = true;
    this.readyState = 3;
  }

  open(): void {
    this.readyState = 1;
    this.onopen?.({});
  }

  receive(frame: ServerFrame): void {
    this.onmessage?.({ data: JSON.stringify(frame) });
  }

  drop(): void {
    this.readyState = 3;
    this.onclose?.({});
  }

  last<T extends ClientFrame['type']>(type: T): Extract<ClientFrame, { type: T }> | undefined {
    return [...this.sent].reverse().find((f) => f.type === type) as Extract<ClientFrame, { type: T }> | undefined;
  }

  all<T extends ClientFrame['type']>(type: T): Extract<ClientFrame, { type: T }>[] {
    return this.sent.filter((f) => f.type === type) as Extract<ClientFrame, { type: T }>[];
  }
}

const ws = (id: string, online: boolean): WorkspaceInfo => ({ id, name: id, online, seen: 0, policy: 'full' });

function welcome(sock: FakeSocket, workspaces = [ws('devbox', true)]): void {
  sock.receive({ type: 'Welcome', content: { api: 1, nexus: 'test', workspaces, states: [] } });
}

describe('SocketClient', () => {
  let sockets: FakeSocket[];
  let client: SocketClient;

  beforeEach(() => {
    vi.useFakeTimers();
    sockets = [];
    client = new SocketClient({
      url: () => 'ws://test/api',
      create: () => {
        const s = new FakeSocket();
        sockets.push(s);
        return s;
      },
      timeout: 1000,
      backoff: () => 100,
    });
  });

  afterEach(() => {
    client.disconnect();
    vi.useRealTimers();
  });

  function connected(): FakeSocket {
    client.connect();
    const s = sockets[sockets.length - 1]!;
    s.open();
    welcome(s);
    return s;
  }

  it('says hello first and is open after the welcome', () => {
    client.connect();
    const s = sockets[0]!;
    expect(client.state()).toBe('connecting');
    s.open();
    expect(s.sent[0]).toEqual({ type: 'Hello', content: { api: 1, client: 'web' } });
    welcome(s);
    expect(client.state()).toBe('open');
    expect(client.welcome()?.nexus).toBe('test');
  });

  it('answers requests by id', async () => {
    const s = connected();
    const a = client.request('devbox', { type: 'Previews' });
    const b = client.call('devbox', { type: 'GitStatus', content: { tab: 't1' } }, 'Text');
    const [ra, rb] = s.all('Request');
    expect(ra!.content.workspace).toBe('devbox');
    expect(ra!.content.id).not.toBe(rb!.content.id);
    // Answers may come in any order.
    s.receive({ type: 'Response', content: { id: rb!.content.id, ok: { type: 'Text', content: 'abc' } } });
    s.receive({ type: 'Response', content: { id: ra!.content.id, ok: { type: 'Previews', content: [] } } });
    await expect(a).resolves.toEqual({ type: 'Previews', content: [] });
    await expect(b).resolves.toBe('abc');
  });

  it('turns an error response into an ApiFailure', async () => {
    const s = connected();
    const p = client.request('devbox', { type: 'Previews' });
    const id = s.last('Request')!.content.id;
    s.receive({ type: 'Response', content: { id, error: { code: 'forbidden', message: 'view only' } } });
    await expect(p).rejects.toMatchObject({ code: 'forbidden', message: 'view only' });
  });

  it('rejects a reply of the wrong type', async () => {
    const s = connected();
    const p = client.call('devbox', { type: 'Previews' }, 'Previews');
    s.receive({ type: 'Response', content: { id: s.last('Request')!.content.id, ok: { type: 'Done' } } });
    await expect(p).rejects.toBeInstanceOf(ApiFailure);
  });

  it('times a request out', async () => {
    connected();
    const p = client.request('devbox', { type: 'Previews' });
    vi.advanceTimersByTime(1001);
    await expect(p).rejects.toMatchObject({ code: 'timeout' });
  });

  it('queues requests until the welcome', async () => {
    client.connect();
    const s = sockets[0]!;
    const p = client.request('devbox', { type: 'Previews' });
    s.open();
    expect(s.all('Request')).toHaveLength(0);
    welcome(s);
    const req = s.last('Request')!;
    s.receive({ type: 'Response', content: { id: req.content.id, ok: { type: 'Done' } } });
    await expect(p).resolves.toEqual({ type: 'Done' });
  });

  it('delivers events to their subscription and unsubscribes', () => {
    const s = connected();
    const events: unknown[] = [];
    const sub = client.subscribe('devbox', { type: 'Screen', content: { pane: 'p1' } }, { event: (e) => events.push(e) });
    const frame = s.last('Subscribe')!;
    expect(frame.content).toEqual({ id: sub.id, workspace: 'devbox', topic: { type: 'Screen', content: { pane: 'p1' } } });
    s.receive({ type: 'Event', content: { sub: sub.id, event: { type: 'GitChanged' } } });
    s.receive({ type: 'Event', content: { sub: sub.id + 99, event: { type: 'GitChanged' } } });
    expect(events).toEqual([{ type: 'GitChanged' }]);
    sub.close();
    expect(s.last('Unsubscribe')).toEqual({ type: 'Unsubscribe', content: { id: sub.id } });
    expect(client.subscriptions).toBe(0);
  });

  it('reconnects with backoff and subscribes again', async () => {
    const s1 = connected();
    const sub = client.subscribe('devbox', { type: 'Ai', content: { tab: 't1' } }, { event: () => undefined });
    expect(s1.all('Subscribe')).toHaveLength(1);
    const pending = client.request('devbox', { type: 'Previews' });
    s1.drop();
    expect(client.state()).toBe('closed');
    // The request on the wire is lost with the connection.
    await expect(pending).rejects.toMatchObject({ code: 'offline' });
    expect(sockets).toHaveLength(1);
    vi.advanceTimersByTime(100);
    expect(sockets).toHaveLength(2);
    const s2 = sockets[1]!;
    s2.open();
    expect(s2.all('Subscribe')).toHaveLength(0);
    welcome(s2);
    expect(client.state()).toBe('open');
    expect(s2.last('Subscribe')!.content.id).toBe(sub.id);
  });

  it('pauses subscriptions while their workspace is offline and resumes them', () => {
    const s = connected();
    const paused = vi.fn();
    const sub = client.subscribe('devbox', { type: 'Ai', content: { tab: 't1' } }, { event: () => undefined, paused });
    s.receive({ type: 'Workspaces', content: [ws('devbox', false)] });
    s.receive({ type: 'Ended', content: { sub: sub.id, error: { code: 'offline', message: '' } } });
    expect(paused).toHaveBeenCalled();
    expect(client.subscriptions).toBe(1);
    expect(s.all('Subscribe')).toHaveLength(1);
    s.receive({ type: 'Workspaces', content: [ws('devbox', true)] });
    expect(s.all('Subscribe')).toHaveLength(2);
  });

  it('ends a subscription for good when its thing is gone', () => {
    const s = connected();
    const ended = vi.fn();
    const sub = client.subscribe('devbox', { type: 'Ai', content: { tab: 't1' } }, { event: () => undefined, ended });
    s.receive({ type: 'Ended', content: { sub: sub.id, error: { code: 'notFound', message: '' } } });
    expect(ended).toHaveBeenCalledWith({ code: 'notFound', message: '' });
    expect(client.subscriptions).toBe(0);
  });

  it('stops for good on goodbye', async () => {
    const s = connected();
    const p = client.request('devbox', { type: 'Previews' });
    s.receive({ type: 'Goodbye', content: { code: 'unauthorized', message: 'signed out' } });
    await expect(p).rejects.toMatchObject({ code: 'unauthorized' });
    expect(client.state()).toBe('goodbye');
    expect(client.goodbye()?.code).toBe('unauthorized');
    vi.advanceTimersByTime(60_000);
    expect(sockets).toHaveLength(1);
  });
});
