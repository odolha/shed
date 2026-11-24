// Shed end to end, with real processes: a nexus serving the web app, a
// keeper with its link, and the web app in a headless Chrome. Run by
// tools/e2e.sh, which builds, sets up the folders and the environment
// (a keeper and settings of their own, never the user's) and cleans up.
//
// node tools/e2e/e2e.mjs   with E2E_DIR, E2E_URL, E2E_SHED, E2E_NEXUS_DATA set.

import { spawn, execFileSync } from 'node:child_process';
import { createHmac } from 'node:crypto';
import { mkdirSync, readFileSync } from 'node:fs';
import { setTimeout as sleep } from 'node:timers/promises';
import { Browser } from './cdp.mjs';

const DIR = need('E2E_DIR');
const URL_ = need('E2E_URL');
const SHED = need('E2E_SHED');
const NEXUS_DATA = need('E2E_NEXUS_DATA');
const PROJECT = need('E2E_PROJECT');
const SHOTS = `${DIR}/shots`;
const USER = 'you';
const PASSWORD = 'a long enough password for e2e';

function need(name) {
  const v = process.env[name];
  if (!v) throw new Error(`${name} is not set (run tools/e2e.sh)`);
  return v;
}

let step = '';
const t0 = Date.now();
function say(what) {
  step = what;
  console.log(`[e2e ${((Date.now() - t0) / 1000).toFixed(1).padStart(5)}s] ${what}`);
}

function check(ok, what) {
  if (!ok) throw new Error(`check failed: ${what}`);
}

// ------------------------------------------------------------------ TOTP

function base32(s) {
  const abc = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567';
  let bits = '';
  for (const c of s.replace(/=+$/, '').toUpperCase()) bits += abc.indexOf(c).toString(2).padStart(5, '0');
  const out = [];
  for (let i = 0; i + 8 <= bits.length; i += 8) out.push(parseInt(bits.slice(i, i + 8), 2));
  return Buffer.from(out);
}

function totpAt(secret, counter) {
  const msg = Buffer.alloc(8);
  msg.writeBigUInt64BE(BigInt(counter));
  const h = createHmac('sha1', base32(secret)).update(msg).digest();
  const o = h[h.length - 1] & 15;
  const n = (h.readUInt32BE(o) & 0x7fffffff) % 1_000_000;
  return String(n).padStart(6, '0');
}

/** The nexus takes each 30 s step once: waits for one not used yet. */
let lastStep = 0;
async function freshCode(secret) {
  for (;;) {
    const now = Math.floor(Date.now() / 1000 / 30);
    if (now > lastStep) {
      lastStep = now;
      return totpAt(secret, now);
    }
    const wait = (now + 1) * 30_000 - Date.now() + 200;
    say(`waiting ${Math.round(wait / 1000)} s for a fresh authenticator code`);
    await sleep(wait);
  }
}

// ------------------------------------------------------------------ HTTP

async function http(method, path, body, token) {
  const headers = { 'content-type': 'application/json' };
  if (token) headers.authorization = `Bearer ${token}`;
  const r = await fetch(`${URL_}/api/v1${path}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
  const text = await r.text();
  if (!r.ok) throw new Error(`${method} ${path}: ${r.status} ${text}`);
  return text ? JSON.parse(text) : null;
}

// ------------------------------------------------------- the API socket

/** The connectors' socket, as a phone (a bearer token). */
class Api {
  static async open(token) {
    const ws = new WebSocket(`${URL_.replace(/^http/, 'ws')}/api`, { headers: { authorization: `Bearer ${token}` } });
    const api = new Api(ws);
    await new Promise((ok, fail) => {
      ws.onopen = ok;
      ws.onerror = (e) => fail(new Error(`the /api socket: ${e.message ?? 'failed'}`));
    });
    ws.send(JSON.stringify({ type: 'Hello', content: { api: 1, client: 'e2e' } }));
    await api.until((f) => f.type === 'Welcome', 'Welcome');
    return api;
  }

  constructor(ws) {
    this.ws = ws;
    this.frames = [];
    this.next = 1;
    this.listeners = new Set();
    ws.onmessage = (m) => {
      const f = JSON.parse(m.data);
      this.frames.push(f);
      for (const l of this.listeners) l(f);
    };
  }

  /** Waits for a frame (seen already, or to come) that `pick` returns something for. */
  until(pick, what, timeout = 20000) {
    for (const f of this.frames) {
      const v = pick(f);
      if (v) return Promise.resolve(v);
    }
    return new Promise((ok, fail) => {
      const timer = setTimeout(() => {
        this.listeners.delete(l);
        fail(new Error(`timed out waiting for ${what}`));
      }, timeout);
      const l = (f) => {
        const v = pick(f);
        if (v) {
          clearTimeout(timer);
          this.listeners.delete(l);
          ok(v);
        }
      };
      this.listeners.add(l);
    });
  }

  async call(workspace, call) {
    const id = this.next++;
    this.ws.send(JSON.stringify({ type: 'Request', content: { id, workspace, call } }));
    const r = await this.until((f) => f.type === 'Response' && f.content.id === id && f.content, `the answer to ${call.type}`, 30000);
    if (r.error) throw new Error(`${call.type}: ${r.error.code}: ${r.error.message}`);
    return r.ok;
  }

  /** The workspace's latest state that `pick` likes. */
  state(workspace, pick, what) {
    let latest = null;
    for (const f of this.frames) if (f.type === 'State' && f.content.workspace === workspace) latest = f.content.state;
    if (latest && pick(latest)) return Promise.resolve(latest);
    return this.until((f) => f.type === 'State' && f.content.workspace === workspace && pick(f.content.state) && f.content.state, what);
  }

  close() {
    this.ws.close();
  }
}

// ------------------------------------------------------------------ run

async function main() {
  mkdirSync(SHOTS, { recursive: true });

  say('the nexus is up, with no account yet');
  const status = await http('GET', '/status');
  check(status.ready === false, 'a new nexus has no account');

  say('setting up the account (setup link, password, TOTP)');
  const link = readFileSync(`${NEXUS_DATA}/setup-link.txt`, 'utf8');
  const setupCode = /code=([^\s&]+)/.exec(link)?.[1];
  check(setupCode, 'the setup link has a code');

  // The setup page as a person sees it: its QR code (the production build's
  // chunk for the CommonJS qrcode package once drew none) and the key.
  // Starting setup again below replaces what this page started.
  const setupBrowser = await Browser.launch(`${DIR}/chrome-setup`, { width: 900, height: 1100 });
  try {
    await setupBrowser.goto(`${URL_}/setup#code=${setupCode}`);
    await setupBrowser.type('input[name=username]', USER);
    await setupBrowser.type('input[name=password]', PASSWORD);
    await setupBrowser.type('input[name=password2]', PASSWORD);
    await setupBrowser.click('button[type=submit]', null, 'Next');
    await setupBrowser.waitFor('document.querySelector("input[name=totp]")', 'the authenticator step');
    const qr = await setupBrowser.waitFor(
      `(() => { const i = document.querySelector('img.qr'); return i?.complete && i.naturalWidth > 0 && i.src.startsWith('data:image/svg+xml') && i.src; })()`,
      'the QR code',
    );
    check(decodeURIComponent(qr).includes('<path'), 'the QR code is drawn');
    const key = await setupBrowser.eval(`document.querySelector('code.secret').textContent.replace(/\\s/g, '')`);
    check(/^[A-Z2-7]{32}$/.test(key), `the page shows a 32-letter base32 key (got ${key})`);
    await setupBrowser.screenshot(`${SHOTS}/00-setup.png`);
  } finally {
    await setupBrowser.close();
  }

  const totp = await http('POST', '/setup', { code: setupCode, username: USER, password: PASSWORD });
  check(/[?&]digits=6(&|$)/.test(totp.url) && /[?&]period=30(&|$)/.test(totp.url) && /[?&]algorithm=SHA1(&|$)/.test(totp.url) && /[?&]issuer=Shed(&|$)/.test(totp.url), `the otpauth URL says issuer, digits, period and algorithm (got ${totp.url})`);
  const secret = totp.secret;
  const signed = await http('POST', '/setup/confirm', {
    code: setupCode,
    totp: await freshCode(secret),
    device: { kind: 'phone', name: 'e2e script' },
  });
  const token = signed.token;
  check(token && signed.recoveryCodes.length > 0, 'a phone gets a token and recovery codes');

  say('pairing the workspace: shed nexus join');
  const join = spawn(SHED, ['nexus', 'join', URL_, '--name', 'e2e-box'], { stdio: ['ignore', 'pipe', 'pipe'] });
  let joinOut = '';
  join.stdout.on('data', (d) => (joinOut += d));
  join.stderr.on('data', (d) => (joinOut += d));
  const joined = new Promise((ok) => join.on('exit', ok));
  for (let i = 0; !/Pairing code: (\S+)/.test(joinOut); i++) {
    if (i > 200 || join.exitCode !== null) throw new Error(`shed nexus join printed no code:\n${joinOut}`);
    await sleep(50);
  }
  const pairCode = /Pairing code: (\S+)/.exec(joinOut)[1];
  const pending = await http('GET', '/pair/pending', undefined, token);
  check(pending.length === 1 && pending[0].code === '' && pending[0].id, 'one pending request, with an id and no code');
  await http('POST', '/pair/approve', { code: pairCode.toLowerCase(), name: 'e2e-box', totp: await freshCode(secret) }, token);
  const exit = await joined;
  check(exit === 0, `shed nexus join ended well:\n${joinOut}`);

  say('waiting for the link to connect');
  for (let i = 0; ; i++) {
    const out = execFileSync(SHED, ['nexus', 'status'], { encoding: 'utf8' });
    if (/link: connected/.test(out)) break;
    if (i > 100) throw new Error(`the link did not connect:\n${out}`);
    await sleep(200);
  }

  const api = await Api.open(token);
  const ws = await api.until((f) => f.type === 'Workspaces' ? f.content.find((w) => w.online) : f.type === 'Welcome' && f.content.workspaces.find((w) => w.online), 'the workspace online');
  check(ws.name === 'e2e-box', 'the workspace has the name it was approved with');
  say(`workspace ${ws.id} is online`);

  say('making a bench with a terminal, a project and an AI tab (through the API)');
  const bench = (await api.call(ws.id, { type: 'NewBench', content: { name: 'e2e', cwd: PROJECT } })).content;
  const benchRef = { type: 'Running', content: bench };
  const projectTab = (await api.call(ws.id, { type: 'NewTab', content: { bench: benchRef, kind: { type: 'Project', content: { folder: PROJECT } } } })).content;
  const aiTab = (await api.call(ws.id, { type: 'NewTab', content: { bench: benchRef, kind: { type: 'Ai', content: { folder: PROJECT } } } })).content;
  const state = await api.state(ws.id, (s) => s.benches.find((b) => b.id === bench)?.tabs.length === 3, 'the bench with three tabs');
  const tabs = state.benches.find((b) => b.id === bench).tabs;
  const termTab = tabs.find((t) => t.id !== projectTab && t.id !== aiTab).id;
  const browsed = await api.call(ws.id, { type: 'Browse', content: { path: '~' } });
  check(browsed.type === 'Browsed' && browsed.content.entries.some((e) => e.name === 'projects'), 'Browse lists the home folder');
  const found = await api.call(ws.id, { type: 'QuickOpen', content: { tab: projectTab, query: 'emoji' } });
  const hit = found.content.find((f) => f.path === 'docs/🦀-emoji.md');
  check(hit, 'quick open finds a file with an emoji in its name');
  // UTF-16 positions: the 'e' of "emoji" comes after "docs/" (5) and the crab (2 units) and "-" (1).
  check(hit.hits[0] === 8, `quick open's hits are UTF-16 positions (got ${hit.hits})`);
  const upload = await api.call(ws.id, { type: 'AiUpload', content: { tab: aiTab, name: '../notes.txt', data: Buffer.from('for Claude\n').toString('base64') } });
  check(upload.type === 'Text' && upload.content.startsWith(`${DIR}/state/shed/ai-files/${aiTab}/`) && upload.content.endsWith('/notes.txt'), `AiUpload keeps a file under the tab's uploads (got ${upload.content})`);
  check(readFileSync(upload.content, 'utf8') === 'for Claude\n', 'the uploaded file reads back');

  say('signing in to the web app (password and TOTP)');
  const browser = await Browser.launch(`${DIR}/chrome`);
  try {
    await browser.goto(`${URL_}/`);
    await browser.waitFor('location.pathname === "/login" && document.querySelector("input[name=totp]")', 'the sign-in page');
    // The page's styles all apply (an inline script to switch them on would be refused by the CSP).
    check(await browser.eval(`[...document.styleSheets].every((s) => s.media.mediaText !== 'print')`), 'no stylesheet is left print-only');
    await browser.type('input[name=username]', USER);
    await browser.type('input[name=password]', PASSWORD);
    await browser.type('input[name=totp]', await freshCode(secret));
    await browser.screenshot(`${SHOTS}/01-sign-in.png`);
    await browser.click('button[type=submit]', null, 'Sign In');
    await browser.waitFor('location.pathname !== "/login"', 'signing in');

    say('the overview shows the workspace and its bench');
    await browser.waitFor(`document.body.innerText.includes('e2e-box') && document.body.innerText.includes('e2e')`, 'the workspace in the overview');
    await browser.goto(`${URL_}/w/${ws.id}`);
    await browser.waitFor(`document.body.innerText.includes('e2e')`, 'the bench in the overview');
    await browser.screenshot(`${SHOTS}/02-overview.png`);

    say('a reload keeps the session, and changes still work (CSRF token again)');
    await browser.goto(`${URL_}/w/${ws.id}`);
    await browser.waitFor(`document.body.innerText.includes('e2e-box')`, 'the overview after a reload');
    const after = await browser.eval(`fetch('/api/v1/session').then((r) => r.json())`);
    check(after.username === USER && after.csrf, 'GET /api/v1/session gives the CSRF token back');

    say('the terminal: typing echo e2e-ok');
    await browser.goto(`${URL_}/w/${ws.id}/b/${bench}/t/${termTab}`);
    await browser.waitFor(`document.querySelector('shed-terminal-view textarea.catcher')`, 'the terminal');
    await browser.waitFor(`document.querySelector('shed-terminal-view').innerText.includes('you@shed')`, 'the prompt');
    await browser.type('shed-terminal-view textarea.catcher', 'echo e2e-$((40+2))');
    await browser.key('Enter');
    await browser.waitFor(`document.querySelector('shed-terminal-view').innerText.includes('e2e-42')`, 'the output of echo');
    await browser.screenshot(`${SHOTS}/03-terminal.png`);

    say('the logbook has the command');
    await browser.goto(`${URL_}/w/${ws.id}/logbook`);
    await browser.waitFor(`document.body.innerText.includes('echo e2e-$((40+2))')`, 'the command in the logbook');
    await browser.screenshot(`${SHOTS}/04-logbook.png`);

    say('the project: its changes and a diff');
    await browser.goto(`${URL_}/w/${ws.id}/b/${bench}/t/${projectTab}`);
    await browser.click('shed-project-tab button[role=tab]', 'Changes');
    await browser.click('shed-git *', 'README.md', 'README.md in the changes');
    await browser.waitFor(`document.querySelector('shed-diff')?.innerText.includes('a line changed by the e2e test')`, 'the diff');
    await browser.screenshot(`${SHOTS}/05-diff.png`);

    say('the project: a file with colours');
    await browser.click('shed-project-tab button[role=tab]', 'Files');
    await browser.click('shed-file-tree *', 'src', 'the src folder in the tree');
    await browser.click('shed-file-tree *', 'main.rs', 'main.rs in the tree');
    await browser.waitFor(`document.querySelector('shed-file-view')?.innerText.includes('fn main')`, 'the file');
    await browser.screenshot(`${SHOTS}/06-file.png`);

    say('the AI tab: a prompt and the conversation (a stand-in claude)');
    await browser.goto(`${URL_}/w/${ws.id}/b/${bench}/t/${aiTab}`);
    await browser.type('shed-ai-prompt textarea', 'count the lines');
    await browser.click('shed-ai-prompt button.primary', null, 'Send');
    await browser.waitFor(`document.querySelector('shed-ai-tab')?.innerText.includes('count the lines')`, 'the prompt in the conversation');
    await browser.waitFor(`document.querySelector('shed-ai-tab')?.innerText.includes('has 3 lines now')`, "Claude's answer", 30000);
    await browser.screenshot(`${SHOTS}/07-ai.png`);
    await browser.waitFor(`[...document.querySelectorAll('shed-ai-prompt button')].some((b) => b.innerText.includes('Attach'))`, 'the Attach button');

    const errors = browser.console.filter((l) => l.startsWith('exception') || l.startsWith('error'));
    if (errors.length) console.log(`the page logged:\n  ${errors.join('\n  ')}`);
  } catch (e) {
    await browser.screenshot(`${SHOTS}/failed.png`).catch(() => {});
    console.log(`the page logged:\n  ${browser.console.slice(-30).join('\n  ')}`);
    throw e;
  } finally {
    await browser.close();
    api.close();
  }
  say('all good');
}

main().then(
  () => process.exit(0),
  (e) => {
    console.error(`\nE2E FAILED at "${step}": ${e.message}`);
    process.exit(1);
  },
);
