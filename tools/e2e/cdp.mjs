// A small Chrome DevTools Protocol driver, without dependencies (Node 22+:
// its WebSocket and fetch). Starts a headless Chrome of its own (its own
// profile folder), drives one page and stops it again.

import { spawn } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { setTimeout as sleep } from 'node:timers/promises';

/** The Chrome to use: `$CHROME`, else the first one found. */
export function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  for (const c of ['/usr/bin/google-chrome', '/usr/bin/google-chrome-stable', '/usr/bin/chromium', '/usr/bin/chromium-browser']) {
    if (existsSync(c)) return c;
  }
  throw new Error('no Chrome found: set CHROME to one');
}

export class Browser {
  static async launch(profile, { width = 1400, height = 900 } = {}) {
    const chrome = spawn(
      findChrome(),
      [
        '--headless=new',
        '--remote-debugging-port=0',
        `--user-data-dir=${profile}`,
        '--no-first-run',
        '--no-default-browser-check',
        '--disable-gpu',
        '--disable-extensions',
        '--disable-background-networking',
        '--disable-sync',
        '--password-store=basic',
        `--window-size=${width},${height}`,
        'about:blank',
      ],
      { stdio: ['ignore', 'ignore', 'pipe'] },
    );
    let stderr = '';
    chrome.stderr.on('data', (d) => (stderr = (stderr + d).slice(-4000)));
    const portFile = `${profile}/DevToolsActivePort`;
    for (let i = 0; !existsSync(portFile); i++) {
      if (i > 200 || chrome.exitCode !== null) throw new Error(`Chrome did not start:\n${stderr}`);
      await sleep(50);
    }
    const port = readFileSync(portFile, 'utf8').split('\n')[0];
    const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
    const page = targets.find((t) => t.type === 'page');
    const b = new Browser(chrome, new WebSocket(page.webSocketDebuggerUrl));
    await new Promise((ok, fail) => {
      b.ws.onopen = ok;
      b.ws.onerror = fail;
    });
    await b.send('Page.enable');
    await b.send('Runtime.enable');
    await b.send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false });
    return b;
  }

  constructor(proc, ws) {
    this.proc = proc;
    this.ws = ws;
    this.next = 1;
    this.waiting = new Map();
    /** What the page logged and threw, for the report. */
    this.console = [];
    ws.onmessage = (m) => {
      const msg = JSON.parse(m.data);
      if (msg.id && this.waiting.has(msg.id)) {
        const { ok, fail } = this.waiting.get(msg.id);
        this.waiting.delete(msg.id);
        if (msg.error) fail(new Error(`${msg.error.message} ${msg.error.data ?? ''}`));
        else ok(msg.result);
      } else if (msg.method === 'Runtime.consoleAPICalled') {
        const text = msg.params.args.map((a) => a.value ?? a.description ?? '').join(' ');
        this.console.push(`${msg.params.type}: ${text}`);
      } else if (msg.method === 'Runtime.exceptionThrown') {
        const d = msg.params.exceptionDetails;
        this.console.push(`exception: ${d.exception?.description ?? d.text}`);
      }
    };
  }

  send(method, params = {}) {
    const id = this.next++;
    this.ws.send(JSON.stringify({ id, method, params }));
    return new Promise((ok, fail) => this.waiting.set(id, { ok, fail }));
  }

  /** Evaluates `expr` (an expression, or an async function's body if
   * `body`) in the page and gives back its value. */
  async eval(expr) {
    const r = await this.send('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true, userGesture: true });
    if (r.exceptionDetails) throw new Error(`in the page: ${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`);
    return r.result.value;
  }

  async goto(url) {
    await this.send('Page.navigate', { url });
    await this.waitFor('document.readyState === "complete"', 'the page to load');
  }

  /** Waits until `expr` is truthy in the page; gives its value. */
  async waitFor(expr, what, timeout = 20000) {
    const end = Date.now() + timeout;
    let last;
    for (;;) {
      try {
        const v = await this.eval(expr);
        if (v) return v;
      } catch (e) {
        last = e;
      }
      if (Date.now() > end) throw new Error(`timed out waiting for ${what}${last ? ` (${last.message})` : ''}`);
      await sleep(150);
    }
  }

  /** Clicks the first visible element matching `selector` whose text has
   * `text` (any if none), the innermost if they nest. */
  async click(selector, text, what = text ?? selector) {
    const js = `(() => {
      const t = ${JSON.stringify(text ?? null)};
      const all = [...document.querySelectorAll(${JSON.stringify(selector)})].filter((e) =>
        e.getClientRects().length && (t === null || (e.textContent + ' ' + (e.getAttribute('aria-label') ?? '') + ' ' + (e.getAttribute('title') ?? '')).includes(t)));
      // The innermost: not a container of another match.
      const el = all.find((e) => !all.some((o) => o !== e && e.contains(o)));
      if (!el) return false;
      el.scrollIntoView({ block: 'center' });
      el.click();
      return true;
    })()`;
    await this.waitFor(js, `${what} to click`);
  }

  /** Focuses the element and types `text` as a keyboard would insert it. */
  async type(selector, text) {
    await this.waitFor(`(() => { const e = document.querySelector(${JSON.stringify(selector)}); if (!e) return false; e.focus(); return document.activeElement === e; })()`, `${selector} to focus`);
    await this.send('Input.insertText', { text });
  }

  /** A key press (Enter, Backspace, Escape, Tab, ArrowUp…) to the focused element. */
  async key(key, code = key, keyCode = { Enter: 13, Escape: 27, Tab: 9, Backspace: 8 }[key] ?? 0) {
    const base = { key, code, windowsVirtualKeyCode: keyCode, nativeVirtualKeyCode: keyCode };
    await this.send('Input.dispatchKeyEvent', { type: 'keyDown', ...base, text: key === 'Enter' ? '\r' : undefined });
    await this.send('Input.dispatchKeyEvent', { type: 'keyUp', ...base });
  }

  async screenshot(path) {
    const r = await this.send('Page.captureScreenshot', { format: 'png' });
    writeFileSync(path, Buffer.from(r.data, 'base64'));
  }

  async text() {
    return this.eval('document.body.innerText');
  }

  async close() {
    try {
      await this.send('Browser.close');
    } catch {
      // Gone already.
    }
    this.ws.close();
    for (let i = 0; i < 40 && this.proc.exitCode === null; i++) await sleep(50);
    if (this.proc.exitCode === null) this.proc.kill('SIGKILL');
  }
}
