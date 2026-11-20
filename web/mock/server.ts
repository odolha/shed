// A mock nexus for developing the web app: the HTTP API and the `/api`
// WebSocket of crates/shed-api, with pretend workspaces. `npm run mock`.
//
//   PORT=8787            where it listens (the dev server's proxy expects 8787)
//   MOCK_FRESH=1         start without an account, to try the setup
//   MOCK_VIEW=1          devbox allows viewing only
//   MOCK_LATENCY=300     delay every answer by this many ms
//   MOCK_NO_REVIEW=1     AI review answers `unsupported`

import { createServer } from 'node:http';
import { WebSocketServer, type WebSocket } from 'ws';
import type { ClientFrame, ServerFrame } from '../src/api/gen/api.ts';
import { handleHttp, sessionOf, setOnSessionEnd, setupCode } from './http.ts';
import { API_VERSION, Fail, env } from './util.ts';
import { handleCall, setHooks, states, subscribeTopic, topicKey, workspaces } from './world.ts';

interface Conn {
  ws: WebSocket;
  token: string;
  hello: boolean;
  subs: Map<number, { key: string; stop: () => void }>;
}

const conns = new Set<Conn>();

function send(c: Conn, frame: ServerFrame): void {
  if (c.ws.readyState === c.ws.OPEN) c.ws.send(JSON.stringify(frame));
}

function broadcast(frame: ServerFrame): void {
  for (const c of conns) if (c.hello) send(c, frame);
}

setHooks({
  stateChanged(workspace) {
    const state = states[workspace];
    if (state) broadcast({ type: 'State', content: { workspace, state } });
  },
  workspacesChanged() {
    broadcast({ type: 'Workspaces', content: workspaces });
  },
  endTopics(keys) {
    for (const c of conns) {
      for (const [id, s] of c.subs) {
        if (keys.includes(s.key)) {
          s.stop();
          c.subs.delete(id);
          send(c, { type: 'Ended', content: { sub: id, error: { code: 'notFound', message: 'it closed' } } });
        }
      }
    }
  },
});

setOnSessionEnd((token) => {
  for (const c of conns) {
    if (c.token !== token) continue;
    send(c, { type: 'Goodbye', content: { code: 'unauthorized', message: 'signed out' } });
    c.ws.close(1000, 'signed out');
  }
});

function onFrame(c: Conn, frame: ClientFrame): void {
  if (frame.type === 'Hello') {
    if (frame.content.api !== API_VERSION) {
      send(c, { type: 'Goodbye', content: { code: 'unsupported', message: `API ${frame.content.api} is not accepted (1 is)` } });
      c.ws.close();
      return;
    }
    c.hello = true;
    send(c, {
      type: 'Welcome',
      content: {
        api: API_VERSION,
        nexus: '0.1.0-mock',
        workspaces,
        states: Object.entries(states).map(([workspace, state]) => ({ workspace, state })),
      },
    });
    return;
  }
  if (!c.hello) return;
  switch (frame.type) {
    case 'Request': {
      const { id, workspace, call } = frame.content;
      const started = Date.now();
      handleCall(workspace ?? '', call)
        .then((ok) => ({ id, ok, error: null }))
        .catch((e: unknown) => {
          const f = e instanceof Fail ? e : new Fail('failed', e instanceof Error ? e.message : String(e));
          return { id, ok: null, error: { code: f.code, message: f.message } };
        })
        .then((response) => {
          const wait = Math.max(0, env.latency - (Date.now() - started));
          setTimeout(() => send(c, { type: 'Response', content: response }), wait);
        });
      break;
    }
    case 'Subscribe': {
      const { id, workspace, topic } = frame.content;
      c.subs.get(id)?.stop();
      // Kept before subscribing: some topics send their first event at once.
      const entry = { key: topicKey(topic), stop: () => {} };
      c.subs.set(id, entry);
      try {
        entry.stop = subscribeTopic(workspace ?? '', topic, (event) => {
          if (c.subs.get(id) === entry) send(c, { type: 'Event', content: { sub: id, event } });
        });
      } catch (e) {
        c.subs.delete(id);
        const f = e instanceof Fail ? e : new Fail('failed', String(e));
        send(c, { type: 'Ended', content: { sub: id, error: { code: f.code, message: f.message } } });
      }
      break;
    }
    case 'Unsubscribe':
      c.subs.get(frame.content.id)?.stop();
      c.subs.delete(frame.content.id);
      break;
  }
}

const server = createServer((req, res) => {
  if (req.url?.startsWith('/api/v1/')) {
    void handleHttp(req, res);
    return;
  }
  res.writeHead(404, { 'Content-Type': 'text/plain' });
  res.end('The mock nexus serves /api/v1 and /api only; open the dev server (npm start).\n');
});

const wss = new WebSocketServer({ noServer: true });

server.on('upgrade', (req, socket, head) => {
  const path = (req.url ?? '').split('?')[0];
  if (path !== '/api') {
    socket.destroy();
    return;
  }
  const session = sessionOf(req);
  wss.handleUpgrade(req, socket, head, (ws) => {
    if (!session) {
      ws.send(JSON.stringify({ type: 'Goodbye', content: { code: 'unauthorized', message: 'not signed in' } } satisfies ServerFrame));
      ws.close(1008, 'not signed in');
      return;
    }
    const c: Conn = { ws, token: session.token, hello: false, subs: new Map() };
    conns.add(c);
    ws.on('message', (data, binary) => {
      if (binary) return;
      let frame: ClientFrame;
      try {
        frame = JSON.parse(data.toString()) as ClientFrame;
      } catch {
        return;
      }
      onFrame(c, frame);
    });
    ws.on('close', () => {
      for (const s of c.subs.values()) s.stop();
      conns.delete(c);
    });
  });
});

server.listen(env.port, '127.0.0.1', () => {
  const lines = [
    '',
    `  Shed mock nexus on http://127.0.0.1:${env.port} (HTTP /api/v1, WebSocket /api)`,
    '',
    '  Run the web app with `npm start` and open http://localhost:4200',
  ];
  if (env.fresh) {
    lines.push('', '  No account yet. Set it up at:', `    http://localhost:4200/setup?code=${setupCode}`, '  Any 6-digit code works as the authenticator code (000000 is wrong).');
  } else {
    lines.push('', '  Sign in as `you`, password `shed`; any 6-digit code (000000 is wrong).');
  }
  lines.push(
    '',
    `  Workspaces: devbox (online${env.viewOnly ? ', view only' : ''}), laptop (offline).`,
    '  In a terminal tab, `help` lists the pretend commands; in the AI tab, a prompt with "ask" gets questions.',
    '',
  );
  console.log(lines.join('\n'));
});
