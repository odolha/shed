// Takes the web app's screenshots for the docs (docs/images/web), against
// the mock nexus, in a headless Chrome of its own (tools/e2e/cdp.mjs): the
// overview, an AI tab and a terminal on a computer, and the AI tab on a phone.
// Mock data only. Start the mock and the app first, from web/:
//   npm run mock & npm start
//   node ../tools/screenshots/web.mjs        (from the repository root: node tools/screenshots/web.mjs)
// APP=http://localhost:4200 is where the app is; OUT=docs/images/web where
// the pictures go.

import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { Browser } from '../e2e/cdp.mjs';

const APP = process.env.APP ?? 'http://localhost:4200';
const OUT = process.env.OUT ?? join(import.meta.dirname, '../../docs/images/web');

async function signedIn(name, { width, height, scale, mobile }) {
  const profile = mkdtempSync(join(tmpdir(), `shed-shots-${name}-`));
  const b = await Browser.launch(profile, { width, height });
  b.profile = profile;
  await b.send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: scale, mobile });
  await b.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: 'dark' }] });
  await b.goto(`${APP}/`);
  await b.waitFor('location.pathname === "/login" && document.querySelector("input[name=totp]")', 'the sign-in page');
  // The mock's account: any code but 000000.
  await b.type('input[name=username]', 'you');
  await b.type('input[name=password]', 'shed');
  await b.type('input[name=totp]', '123456');
  await b.click('button[type=submit]', null, 'Sign In');
  await b.waitFor(`document.body.innerText.includes('devbox')`, 'the overview');
  await sleep(2500);
  return b;
}

async function done(b) {
  await b.close();
  rmSync(b.profile, { recursive: true, force: true });
}

const desk = await signedIn('desk', { width: 1280, height: 800, scale: 1, mobile: false });
try {
  await desk.screenshot(`${OUT}/overview.png`);
  await desk.click('*', 'claude', 'the AI tab');
  await desk.waitFor(`document.querySelector('shed-ai-tab')`, 'the AI tab');
  await sleep(4000);
  await desk.screenshot(`${OUT}/ai.png`);
  await desk.click('*', 'shell', 'the terminal tab');
  await desk.waitFor(`document.querySelector('shed-terminal-view')`, 'the terminal');
  await sleep(2500);
  await desk.screenshot(`${OUT}/terminal.png`);
} finally {
  await done(desk);
}

const phone = await signedIn('phone', { width: 390, height: 844, scale: 1.5, mobile: true });
try {
  await phone.click('*', 'claude', 'the AI tab');
  await phone.waitFor(`document.querySelector('shed-ai-tab')`, 'the AI tab');
  await sleep(4000);
  await phone.screenshot(`${OUT}/phone-ai.png`);
} finally {
  await done(phone);
}
console.log(`screenshots in ${OUT}`);
