/**
 * The one WebSocket to the nexus (`/api`, `crates/shed-api/src/wire.rs`).
 *
 * - `Hello` → `Welcome` opens it; until then requests wait in a queue.
 * - Requests are answered by id; each gets a timeout and is a Promise.
 * - Subscriptions are kept here: sent again after a reconnect, paused while
 *   their workspace is offline and sent again when it is back.
 * - A dropped connection is retried with a growing delay; `Goodbye` is final
 *   (signed out, revoked, an API version the nexus does not take).
 */

import { Injectable, signal } from '@angular/core';
import {
  API_VERSION,
  type ApiError,
  type Call,
  type ClientFrame,
  type Event as ApiEvent,
  type Reply,
  type ServerFrame,
  type Topic,
  type Welcome,
  type WorkspaceInfo,
} from './gen/api';
import { ApiFailure } from './errors';

export type SocketState = 'idle' | 'connecting' | 'open' | 'closed' | 'goodbye';

/** What the client needs of a WebSocket (a fake one in tests). */
export interface SocketLike {
  readonly readyState: number;
  send(data: string): void;
  close(code?: number, reason?: string): void;
  onopen: ((ev: unknown) => void) | null;
  onclose: ((ev: unknown) => void) | null;
  onerror: ((ev: unknown) => void) | null;
  onmessage: ((ev: { data: unknown }) => void) | null;
}

export interface SocketOptions {
  url: () => string;
  create?: (url: string) => SocketLike;
  /** How long a request may take, in ms. */
  timeout?: number;
  /** The delay before reconnect attempt `n` (1, 2, …), in ms. */
  backoff?: (attempt: number) => number;
  /** `web 0.1.0`, for the nexus's logs. */
  client?: string;
}

export interface SubHandler {
  event(event: ApiEvent): void;
  /** It ended for good (the tab closed, it is not there). */
  ended?(error: ApiError | null): void;
  /** Its workspace went away; it resumes by itself when the workspace is back. */
  paused?(error: ApiError | null): void;
}

/** A subscription; `close()` unsubscribes. */
export interface Sub {
  readonly id: number;
  close(): void;
}

/** The content of the reply of type `K`. */
export type ReplyOf<K extends Reply['type']> =
  Extract<Reply, { type: K }> extends { content?: infer C } ? C : never;

interface PendingRequest {
  frame: ClientFrame;
  sent: boolean;
  resolve(reply: Reply): void;
  reject(e: unknown): void;
  timer: ReturnType<typeof setTimeout>;
}

interface SubRecord {
  workspace: string;
  topic: Topic;
  handler: SubHandler;
  /** Paused: the workspace is offline. */
  dormant: boolean;
}

const OPEN = 1;

export function defaultBackoff(attempt: number): number {
  const base = Math.min(15_000, 500 * 2 ** Math.max(0, attempt - 1));
  return Math.round(base * (0.75 + Math.random() * 0.5));
}

export class SocketClient {
  readonly state = signal<SocketState>('idle');
  readonly welcome = signal<Welcome | null>(null);
  /** Why the nexus closed the connection for good. */
  readonly goodbye = signal<ApiError | null>(null);

  private ws: SocketLike | null = null;
  private stopped = true;
  private attempt = 0;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private nextRequest = 1;
  private nextSub = 1;
  private readonly pending = new Map<number, PendingRequest>();
  private readonly subs = new Map<number, SubRecord>();
  private readonly listeners = new Set<(frame: ServerFrame) => void>();
  private readonly online = new Map<string, boolean>();

  constructor(private readonly opts: SocketOptions) {}

  /** Opens the connection (again), if it is not open or opening. */
  connect(): void {
    this.stopped = false;
    this.goodbye.set(null);
    if (this.ws) return;
    if (this.retryTimer) {
      clearTimeout(this.retryTimer);
      this.retryTimer = null;
    }
    this.open();
  }

  /** Closes the connection and stops retrying; requests waiting fail. */
  disconnect(): void {
    this.stopped = true;
    if (this.retryTimer) {
      clearTimeout(this.retryTimer);
      this.retryTimer = null;
    }
    const ws = this.ws;
    this.ws = null;
    if (ws) {
      ws.onclose = ws.onmessage = ws.onerror = ws.onopen = null;
      ws.close(1000, 'bye');
    }
    this.failAll(new ApiFailure('offline', 'disconnected'), true);
    this.state.set('idle');
    this.welcome.set(null);
  }

  /** Retries now rather than after the delay (the page became visible). */
  wake(): void {
    if (this.stopped || this.ws || this.state() === 'goodbye') return;
    this.connect();
  }

  /** Every frame the nexus sends, after this client handled it. */
  listen(fn: (frame: ServerFrame) => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  request(workspace: string, call: Call, timeout = this.opts.timeout ?? 20_000): Promise<Reply> {
    const id = this.nextRequest++;
    const frame: ClientFrame = { type: 'Request', content: { id, workspace, call } };
    return new Promise<Reply>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new ApiFailure('timeout'));
      }, timeout);
      const p: PendingRequest = { frame, sent: false, resolve, reject, timer };
      this.pending.set(id, p);
      if (this.state() === 'open') {
        this.transmit(frame);
        p.sent = true;
      } else if (this.stopped) {
        this.connect();
      }
    });
  }

  /** A request whose reply must be of type `expect`; resolves to its content. */
  async call<K extends Reply['type']>(
    workspace: string,
    call: Call,
    expect: K,
    timeout?: number,
  ): Promise<ReplyOf<K>> {
    const reply = await this.request(workspace, call, timeout);
    if (reply.type !== expect) {
      throw new ApiFailure('invalid', `expected ${expect}, got ${reply.type}`);
    }
    return (reply as { content?: unknown }).content as ReplyOf<K>;
  }

  subscribe(workspace: string, topic: Topic, handler: SubHandler): Sub {
    const id = this.nextSub++;
    this.subs.set(id, { workspace, topic, handler, dormant: false });
    if (this.state() === 'open') this.sendSubscribe(id);
    return {
      id,
      close: () => {
        if (!this.subs.delete(id)) return;
        if (this.state() === 'open') this.transmit({ type: 'Unsubscribe', content: { id } });
      },
    };
  }

  /** How many subscriptions are kept (tests, debugging). */
  get subscriptions(): number {
    return this.subs.size;
  }

  private open(): void {
    this.state.set('connecting');
    let ws: SocketLike;
    try {
      ws = (this.opts.create ?? ((url) => new WebSocket(url) as unknown as SocketLike))(this.opts.url());
    } catch {
      this.scheduleRetry();
      return;
    }
    this.ws = ws;
    ws.onopen = () => {
      this.transmit({ type: 'Hello', content: { api: API_VERSION, client: this.opts.client ?? 'web' } });
    };
    ws.onmessage = (ev) => {
      if (typeof ev.data !== 'string') return;
      let frame: ServerFrame;
      try {
        frame = JSON.parse(ev.data) as ServerFrame;
      } catch {
        return;
      }
      this.handle(frame);
    };
    ws.onerror = () => {
      // onclose follows.
    };
    ws.onclose = () => {
      if (this.ws !== ws) return;
      this.ws = null;
      this.welcome.set(null);
      // Requests on the wire are lost; unsent ones wait for the next connection.
      this.failAll(new ApiFailure('offline', 'connection lost'), false);
      if (this.state() === 'goodbye' || this.stopped) return;
      this.state.set('closed');
      this.scheduleRetry();
    };
  }

  private scheduleRetry(): void {
    if (this.stopped) return;
    this.state.set('closed');
    this.attempt += 1;
    const delay = (this.opts.backoff ?? defaultBackoff)(this.attempt);
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null;
      if (!this.stopped && !this.ws) this.open();
    }, delay);
  }

  private handle(frame: ServerFrame): void {
    switch (frame.type) {
      case 'Welcome':
        this.attempt = 0;
        this.noteWorkspaces(frame.content.workspaces);
        this.welcome.set(frame.content);
        this.state.set('open');
        for (const [id, s] of this.subs) {
          s.dormant = this.online.get(s.workspace) === false;
          if (!s.dormant) this.sendSubscribe(id);
        }
        for (const p of this.pending.values()) {
          if (!p.sent) {
            this.transmit(p.frame);
            p.sent = true;
          }
        }
        break;
      case 'Response': {
        const { id, ok, error } = frame.content;
        const p = this.pending.get(id);
        if (!p) break;
        this.pending.delete(id);
        clearTimeout(p.timer);
        if (error) p.reject(ApiFailure.from(error));
        else if (ok) p.resolve(ok);
        else p.resolve({ type: 'Done' });
        break;
      }
      case 'Event': {
        const s = this.subs.get(frame.content.sub);
        if (s) s.handler.event(frame.content.event);
        break;
      }
      case 'Ended': {
        const { sub, error } = frame.content;
        const s = this.subs.get(sub);
        if (!s) break;
        if (error?.code === 'offline') {
          s.dormant = true;
          s.handler.paused?.(error);
        } else {
          this.subs.delete(sub);
          s.handler.ended?.(error ?? null);
        }
        break;
      }
      case 'Workspaces':
        this.noteWorkspaces(frame.content);
        break;
      case 'State':
        // A state can only come from a workspace that is there.
        this.resume(frame.content.workspace);
        break;
      case 'Goodbye': {
        this.goodbye.set(frame.content);
        this.state.set('goodbye');
        this.stopped = true;
        this.failAll(ApiFailure.from(frame.content), true);
        const ws = this.ws;
        this.ws = null;
        if (ws) {
          ws.onclose = null;
          ws.close(1000, 'goodbye');
        }
        break;
      }
    }
    for (const fn of this.listeners) fn(frame);
  }

  private noteWorkspaces(list: WorkspaceInfo[]): void {
    for (const w of list) {
      const was = this.online.get(w.id);
      this.online.set(w.id, w.online);
      if (w.online && was === false) this.resume(w.id);
    }
  }

  private resume(workspace: string): void {
    if (this.state() !== 'open') return;
    for (const [id, s] of this.subs) {
      if (s.workspace === workspace && s.dormant) {
        s.dormant = false;
        this.sendSubscribe(id);
      }
    }
  }

  private sendSubscribe(id: number): void {
    const s = this.subs.get(id);
    if (!s) return;
    this.transmit({ type: 'Subscribe', content: { id, workspace: s.workspace, topic: s.topic } });
  }

  private transmit(frame: ClientFrame): void {
    const ws = this.ws;
    if (ws && ws.readyState === OPEN) ws.send(JSON.stringify(frame));
  }

  /** Fails requests on the wire (and the queued ones too if `all`). */
  private failAll(error: ApiFailure, all: boolean): void {
    for (const [id, p] of this.pending) {
      if (!all && !p.sent) continue;
      clearTimeout(p.timer);
      this.pending.delete(id);
      p.reject(error);
    }
  }
}

export function apiUrl(loc: Location = location): string {
  return `${loc.protocol === 'https:' ? 'wss' : 'ws'}://${loc.host}/api`;
}

/** The app's connection, retried when the page comes back into view. */
@Injectable({ providedIn: 'root' })
export class NexusSocket extends SocketClient {
  constructor() {
    super({ url: () => apiUrl(), client: 'web 0.1.0' });
    if (typeof document !== 'undefined') {
      document.addEventListener('visibilitychange', () => {
        if (document.visibilityState === 'visible') this.wake();
      });
      window.addEventListener('online', () => this.wake());
    }
  }
}
