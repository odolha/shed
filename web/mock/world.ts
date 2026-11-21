// The pretend workspaces: `devbox` (online) with its benches, and `laptop`
// (offline, its last state only). Calls on a workspace and its topics are
// answered here, as a link would answer them.

import type {
  AiTab,
  Bench,
  BenchRef,
  Browsed,
  Call,
  ClosedBench,
  Event,
  Layout,
  Limits,
  Pane,
  Preview,
  Reply,
  Tab,
  Topic,
  WorkspaceInfo,
  WorkspaceState,
} from '../src/api/gen/api.ts';
import { AiRuntime } from './ai.ts';
import { logSearch, type Place } from './logbook.ts';
import * as project from './project.ts';
import { Shell } from './terminal.ts';
import { Fail, HOME, env, now, uid } from './util.ts';

export const DEVBOX = 'ws-devbox';
export const LAPTOP = 'ws-laptop';

/** Calls that only look (`Call::changes` is false for them). */
const LOOKING = new Set<Call['type']>([
  'Previews',
  'Scrollback',
  'LogSearch',
  'AiSessions',
  'AiPicture',
  'AiReview',
  'ListFolder',
  'QuickOpen',
  'ReadFile',
  'GitStatus',
  'GitDiff',
  'GitLog',
  'GitCommitFiles',
  'GitBranches',
  'BrowseFolders',
  'Browse',
]);

const NAMED_COLORS: Record<string, string> = {
  blue: '#5CB2FF',
  violet: '#B48BFF',
  pink: '#FF8AC8',
  coral: '#FF9A5A',
  amber: '#FFD15C',
  green: '#7BE38E',
  teal: '#4BE0C4',
  slate: '#B3BAC7',
  red: '#FF6B7A',
  orange: '#FFB066',
  lime: '#C6EF5A',
  cyan: '#5FE3FF',
  indigo: '#8C9BFF',
  magenta: '#F28CFF',
  brown: '#D9A47A',
  olive: '#D2CC6E',
};

const PROD = { label: 'prod', color: '#F0566A', tint: true };
const DOCS_TAG = { label: 'docs', color: '#5C9DFF', tint: false };

// ------------------------------------------------------------------ hooks to the server

export interface WorldHooks {
  /** A workspace's state changed (sent, debounced). */
  stateChanged(workspace: string): void;
  /** The workspace list changed. */
  workspacesChanged(): void;
  /** Subscriptions to these topic keys end (`screen:<pane>`, `ai:<tab>`, `project:<tab>`). */
  endTopics(keys: string[]): void;
}

let hooks: WorldHooks = { stateChanged() {}, workspacesChanged() {}, endTopics() {} };

export function setHooks(h: WorldHooks): void {
  hooks = h;
}

// ------------------------------------------------------------------ building tabs

function pane(cwd: string, extra: Partial<Pane> = {}): Pane {
  return { id: uid('p'), cwd, cols: 80, rows: 24, title: null, tool: null, waiting: null, held: false, asking: null, sizeHeld: false, ...extra };
}

function base(name: string | null): string {
  return name?.split('/').filter(Boolean).pop() ?? '~';
}

function terminalTab(cwd: string, panes: Pane[], extra: Partial<Tab> = {}): Tab {
  let layout: Layout = { type: 'Pane', content: panes[0]!.id };
  for (const p of panes.slice(1)) {
    layout = { type: 'Split', content: { dir: 'right', ratio: 0.5, first: layout, second: { type: 'Pane', content: p.id } } };
  }
  return { id: uid('t'), name: null, title: panes[0]!.tool ?? base(cwd), tags: [], private: false, kind: { type: 'Terminal' }, panes, layout, activePane: panes[0]!.id, divider: 'none', ...extra };
}

function aiTab(folder: string, extra: Partial<AiTab> = {}): Tab {
  const ai: AiTab = { folder, addDirs: [], session: null, draftSession: null, model: null, mode: 'auto', effort: null, activity: 'idle', drafts: [], queued: [], unsent: '', browser: false, follow: false, ...extra };
  return { id: uid('t'), name: null, title: base(folder), tags: [], private: false, kind: { type: 'Ai', content: ai }, panes: [], layout: null, activePane: null, divider: 'none' };
}

function projectTab(root: string): Tab {
  return { id: uid('t'), name: null, title: base(root), tags: [], private: false, kind: { type: 'Project', content: { root, open: ['src/main.rs', 'README.md'], active: 0, links: [] } }, panes: [], layout: null, activePane: null, divider: 'none' };
}

function remoteTab(host: string): Tab {
  const p = pane('/home/you', { title: `you@${host}: ~` });
  return { id: uid('t'), name: null, title: host, tags: [], private: false, kind: { type: 'Remote', content: { host, id: uid('r') } }, panes: [p], layout: { type: 'Pane', content: p.id }, activePane: p.id, divider: 'none' };
}

// ------------------------------------------------------------------ the world

const shells = new Map<string, Shell>();
const ais = new Map<string, AiRuntime>();
/** Where a pane's shell runs: its host. */
const paneHost = new Map<string, string>();

const t0 = now();

const limits: Limits = {
  fiveHour: { used: 0.42, resets: t0 + 2 * 3600 },
  week: { used: 0.73, resets: t0 + 3 * 86400 },
  seen: t0,
};

function makeDevbox(): WorkspaceState {
  const shed = `${HOME}/shed`;
  const left = pane(shed, { title: 'you@devbox: ~/shed' });
  const right = pane(shed, { tool: 'cargo watch', title: 'cargo watch -x check' });
  const shell = terminalTab(shed, [left, right], { title: 'shell', name: 'shell' });
  const ai = aiTab(shed);
  ai.title = 'claude';
  ai.name = 'claude';
  const proj = projectTab(shed);
  const remote = remoteTab('buildbox');
  remote.tags = [PROD];
  paneHost.set(remote.panes![0]!.id, 'buildbox');
  const notes = `${HOME}/notes`;
  const watch = pane(notes, { title: 'you@devbox: ~/notes' });
  const notesTab = terminalTab(notes, [watch], { tags: [DOCS_TAG] });
  const privateTab: Tab = { ...terminalTab(notes, [pane(notes)]), private: true, title: '', name: null };
  privateTab.panes = [];
  privateTab.layout = null;
  privateTab.activePane = null;
  const benches: Bench[] = [
    { id: uid('b'), name: 'shed', color: '#5CB2FF', badge: null, cwd: shed, tabs: [shell, ai, proj, remote], activeTab: ai.id, shown: true, closing: false, started: true, links: [] },
    { id: uid('b'), name: 'notes', color: '#7BE38E', badge: 'wip', cwd: notes, tabs: [notesTab, privateTab], activeTab: notesTab.id, shown: false, closing: false, started: false, links: [{ name: 'Docs', url: 'https://example.com/docs' }] },
  ];
  const site = `${HOME}/website`;
  const closed: ClosedBench[] = [
    {
      name: 'website',
      color: '#FF9A5A',
      badge: null,
      cwd: site,
      tabs: [terminalTab(site, [pane(site)], { title: 'dev server', name: 'dev server' }), projectTab(site)],
      saved: t0 - 86400 - 3600,
      links: [{ name: 'Preview', url: 'https://example.com/' }],
    },
  ];
  return { host: 'devbox', benches, closed, limits, home: HOME };
}

function makeLaptop(): WorkspaceState {
  const dir = `${HOME}/thesis`;
  const term = terminalTab(dir, [pane(dir, { title: 'you@laptop: ~/thesis' })]);
  const ai = aiTab(dir);
  return {
    host: 'laptop',
    benches: [{ id: uid('b'), name: 'thesis', color: '#B48BFF', badge: null, cwd: dir, tabs: [term, ai], activeTab: term.id, shown: true, closing: false, started: true, links: [] }],
    closed: [],
    limits: { ...limits, seen: t0 - 7200 },
    home: HOME,
  };
}

export const workspaces: WorkspaceInfo[] = [
  { id: DEVBOX, name: 'devbox', online: true, seen: t0 - 3 * 3600, policy: env.viewOnly ? 'view' : 'full', version: '0.1.0' },
  { id: LAPTOP, name: 'laptop', online: false, seen: t0 - 7200, policy: 'full', version: '0.1.0' },
];

export const states: Record<string, WorkspaceState> = {
  [DEVBOX]: makeDevbox(),
  [LAPTOP]: makeLaptop(),
};

const devbox = states[DEVBOX]!;

const SEED: Record<string, { command: string; output: string[] }[]> = {
  shell: [
    { command: 'cargo build', output: ['   Compiling shed-model v0.1.0 (/home/you/shed/crates/shed-model)', '   Compiling shed v0.1.0 (/home/you/shed)', '    Finished `dev` profile [unoptimized + debuginfo] target(s) in 14.21s'] },
    { command: 'git status --short', output: [' M src/main.rs', 'MM src/lib.rs', '?? docs/new.md'] },
  ],
  watch: [{ command: 'cargo watch -x check', output: ['[Running \'cargo check\']', '    Checking shed v0.1.0 (/home/you/shed)', '    Finished `dev` profile in 1.02s', '[Finished running. Exit status: 0]'] }],
};

function shellFor(p: Pane, host = paneHost.get(p.id) ?? 'devbox', seed?: { command: string; output: string[] }[]): Shell {
  let s = shells.get(p.id);
  if (!s) {
    s = new Shell({ user: 'you', host, cwd: p.cwd, home: HOME, seed });
    s.resize(p.cols, p.rows);
    shells.set(p.id, s);
  }
  return s;
}

function aiFor(tab: Tab, seeded = false): AiRuntime {
  let rt = ais.get(tab.id);
  if (!rt) {
    if (tab.kind.type !== 'Ai') throw new Fail('invalid', 'not an AI tab');
    rt = new AiRuntime(tab.id, tab.kind.content, () => changed(DEVBOX), seeded);
    ais.set(tab.id, rt);
  }
  return rt;
}

/** Shells and AI runtimes for what runs on devbox at start. */
function bootDevbox(): void {
  const [shedBench, notesBench] = devbox.benches;
  for (const tab of shedBench!.tabs) {
    if (tab.kind.type === 'Ai') aiFor(tab, true);
    tab.panes?.forEach((p, i) => shellFor(p, undefined, tab.title === 'shell' ? (i === 0 ? SEED['shell'] : SEED['watch']) : undefined));
  }
  for (const tab of notesBench!.tabs) for (const p of tab.panes ?? []) shellFor(p);
  // A pane that prints now and then, so live updates show.
  const watch = notesBench!.tabs[0]!.panes![0]!;
  setInterval(() => shells.get(watch.id)?.print(`[watch] rebuilt in 0.${Math.floor(Math.random() * 9) + 1}s`, { fg: '8' }), 3000);
}
bootDevbox();

let changeTimers = new Map<string, ReturnType<typeof setTimeout>>();

/** A workspace's state changed: broadcast it within ~100 ms. */
export function changed(workspace: string): void {
  if (changeTimers.has(workspace)) return;
  changeTimers.set(
    workspace,
    setTimeout(() => {
      changeTimers.delete(workspace);
      hooks.stateChanged(workspace);
    }, 100),
  );
}

export function addWorkspace(name: string): void {
  workspaces.push({ id: `ws-${name}`, name, online: false, seen: now(), policy: 'full', version: '0.1.0' });
  hooks.workspacesChanged();
}

// ------------------------------------------------------------------ finding things

function benchById(id: string): Bench {
  const b = devbox.benches.find((x) => x.id === id);
  if (!b) throw new Fail('notFound', `no bench ${id}`);
  return b;
}

function closedByName(name: string): ClosedBench {
  const b = devbox.closed.find((x) => x.name === name);
  if (!b) throw new Fail('notFound', `no closed bench ${name}`);
  return b;
}

function benchRef(ref: BenchRef): Bench | ClosedBench {
  return ref.type === 'Running' ? benchById(ref.content) : closedByName(ref.content);
}

function findTab(id: string): { bench: Bench | ClosedBench; tab: Tab; running: boolean } {
  for (const bench of devbox.benches) {
    const tab = bench.tabs.find((t) => t.id === id);
    if (tab) return { bench, tab, running: true };
  }
  for (const bench of devbox.closed) {
    const tab = bench.tabs.find((t) => t.id === id);
    if (tab) return { bench, tab, running: false };
  }
  throw new Fail('notFound', `no tab ${id}`);
}

function runningTab(id: string): { bench: Bench; tab: Tab } {
  const f = findTab(id);
  if (!f.running) throw new Fail('invalid', 'the tab is in a closed bench');
  return { bench: f.bench as Bench, tab: f.tab };
}

function findPane(id: string): { bench: Bench; tab: Tab; pane: Pane } {
  for (const bench of devbox.benches) {
    for (const tab of bench.tabs) {
      const pane = tab.panes?.find((p) => p.id === id);
      if (pane) return { bench, tab, pane };
    }
  }
  throw new Fail('notFound', `no pane ${id}`);
}

/** A project or AI tab's folder. */
function projectTabOf(id: string): Tab {
  const { tab } = runningTab(id);
  if (tab.kind.type !== 'Project' && tab.kind.type !== 'Ai') throw new Fail('invalid', 'not a project or AI tab');
  return tab;
}

function defaultTitle(tab: Tab): string {
  switch (tab.kind.type) {
    case 'Project':
      return base(tab.kind.content.root);
    case 'Ai':
      return base(tab.kind.content.folder);
    case 'Remote':
      return tab.kind.content.host;
    case 'Terminal': {
      const p = tab.panes?.[0];
      return p?.tool ?? base(p?.cwd ?? HOME);
    }
  }
}

function placeOf(bench: Bench | ClosedBench, tab: Tab): Place {
  const cwd = tab.panes?.[0]?.cwd ?? (tab.kind.type === 'Project' ? tab.kind.content.root : tab.kind.type === 'Ai' ? tab.kind.content.folder : bench.cwd);
  return { bench: bench.name, tab: tab.title, cwd, tags: (tab.tags ?? []).map((t) => t.label) };
}

/** Removes a pane from a layout; the split it was in becomes its sibling. */
function withoutPane(layout: Layout, id: string): Layout | null {
  if (layout.type === 'Pane') return layout.content === id ? null : layout;
  const first = withoutPane(layout.content.first, id);
  const second = withoutPane(layout.content.second, id);
  if (!first) return second;
  if (!second) return first;
  return { type: 'Split', content: { ...layout.content, first, second } };
}

function withSplit(layout: Layout, id: string, split: Layout): Layout {
  if (layout.type === 'Pane') return layout.content === id ? split : layout;
  return { type: 'Split', content: { ...layout.content, first: withSplit(layout.content.first, id, split), second: withSplit(layout.content.second, id, split) } };
}

function stopTab(tab: Tab): void {
  const keys = [`ai:${tab.id}`, `project:${tab.id}`];
  for (const p of tab.panes ?? []) {
    keys.push(`screen:${p.id}`);
    shells.get(p.id)?.dispose();
    shells.delete(p.id);
  }
  ais.get(tab.id)?.dispose();
  ais.delete(tab.id);
  hooks.endTopics(keys);
}

function startTab(tab: Tab): void {
  for (const p of tab.panes ?? []) shellFor(p, tab.kind.type === 'Remote' ? tab.kind.content.host : 'devbox');
  if (tab.kind.type === 'Ai') aiFor(tab);
}

let nextColor = 0;

function color(c: string): string {
  const named = NAMED_COLORS[c.toLowerCase()];
  if (named) return named;
  if (/^#[0-9a-f]{6}$/i.test(c)) return c.toUpperCase();
  throw new Fail('invalid', `not a colour: ${c}`);
}

// ------------------------------------------------------------------ browsing folders

const FOLDERS = ['shed', 'shed/crates', 'shed/crates/shed-model', 'shed/crates/shed-app', 'shed/web', 'shed/docs', 'notes', 'website', 'website/src', 'Downloads', 'thesis'];

/** A path as `Browse` takes it (absolute, `~/…`, `""` for home), made absolute. */
function absolute(path: string): string {
  let p = path.trim();
  if (p === '' || p === '~') p = HOME;
  else if (p.startsWith('~/')) p = HOME + p.slice(1);
  const parts: string[] = [];
  for (const part of p.split('/')) {
    if (!part || part === '.') continue;
    if (part === '..') parts.pop();
    else parts.push(part);
  }
  return '/' + parts.join('/');
}

function browsed(path: string): Browsed {
  const p = absolute(path);
  const i = p.lastIndexOf('/');
  return { path: p, parent: p === '/' ? null : i <= 0 ? '/' : p.slice(0, i), entries: browse(p) };
}

function browse(path: string): { name: string; dir: boolean; ignored: boolean }[] {
  const p = absolute(path);
  if (p === '/' || p === '/home') return [{ name: p === '/' ? 'home' : 'you', dir: true, ignored: false }];
  if (p !== HOME && !p.startsWith(HOME + '/')) throw new Fail('notFound', `no folder ${path}`);
  const rel = p === HOME ? '' : p.slice(HOME.length + 1);
  if (rel && !FOLDERS.includes(rel)) throw new Fail('notFound', `no folder ${path}`);
  const prefix = rel ? rel + '/' : '';
  return FOLDERS.filter((f) => f.startsWith(prefix) && !f.slice(prefix.length).includes('/') && f !== rel)
    .map((f) => ({ name: f.slice(prefix.length), dir: true, ignored: false }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

// ------------------------------------------------------------------ calls

export async function handleCall(workspace: string, call: Call): Promise<Reply> {
  const info = workspaces.find((w) => w.id === workspace);
  if (!info) throw new Fail('notFound', `no workspace ${workspace}`);
  if (!info.online) throw new Fail('offline', `${info.name} is not connected`);
  if (info.policy === 'view' && !LOOKING.has(call.type)) throw new Fail('forbidden', `${info.name} allows viewing only`);
  const reply = await dispatch(call);
  if (!LOOKING.has(call.type)) changed(workspace);
  return reply;
}

const DONE: Reply = { type: 'Done' };

async function dispatch(call: Call): Promise<Reply> {
  switch (call.type) {
    // ---- benches
    case 'NewBench': {
      const cwd = call.content.cwd || HOME;
      const p = pane(cwd);
      const tab = terminalTab(cwd, [p]);
      const used = new Set(devbox.benches.map((b) => b.color));
      const palette = Object.values(NAMED_COLORS);
      let c = palette.find((x) => !used.has(x)) ?? palette[nextColor++ % palette.length]!;
      const name = call.content.name?.trim() || `bench-${devbox.benches.length + 1}`;
      if (devbox.benches.some((b) => b.name === name) || devbox.closed.some((b) => b.name === name)) throw new Fail('invalid', `${name} is taken`);
      const bench: Bench = { id: uid('b'), name, color: c, badge: null, cwd, tabs: [tab], activeTab: tab.id, shown: false, closing: false, started: false, links: [] };
      devbox.benches.push(bench);
      startTab(tab);
      return { type: 'Bench', content: bench.id };
    }
    case 'LoadBench': {
      const closed = closedByName(call.content.name);
      devbox.closed = devbox.closed.filter((b) => b !== closed);
      const bench: Bench = { id: uid('b'), name: closed.name, color: closed.color, badge: closed.badge, cwd: closed.cwd, tabs: closed.tabs, activeTab: closed.tabs[0]?.id ?? '', shown: false, closing: false, started: call.content.start, links: closed.links };
      devbox.benches.push(bench);
      for (const t of bench.tabs) startTab(t);
      return { type: 'Bench', content: bench.id };
    }
    case 'CloseBench': {
      const bench = benchById(call.content.bench);
      devbox.benches = devbox.benches.filter((b) => b !== bench);
      for (const t of bench.tabs) stopTab(t);
      devbox.closed.push({ name: bench.name, color: bench.color, badge: bench.badge, cwd: bench.cwd, tabs: bench.tabs, saved: now(), links: bench.links });
      return DONE;
    }
    case 'DeleteBench':
      closedByName(call.content.name);
      devbox.closed = devbox.closed.filter((b) => b.name !== call.content.name);
      return DONE;
    case 'SaveBench':
    case 'RunTeardown':
      benchById(call.content.bench);
      return DONE;
    case 'RunStartup':
      benchById(call.content.bench).started = true;
      return DONE;
    case 'RenameBench': {
      const name = call.content.name.trim();
      if (!name) throw new Fail('invalid', 'a bench needs a name');
      benchRef(call.content.bench).name = name;
      return DONE;
    }
    case 'SetBenchColor':
      benchRef(call.content.bench).color = color(call.content.color);
      return DONE;
    case 'SetBadge':
      benchRef(call.content.bench).badge = call.content.badge?.trim() || null;
      return DONE;
    case 'Previews': {
      const out: Preview[] = [];
      for (const b of devbox.benches) {
        for (const t of b.tabs) {
          if (t.private) continue;
          const p = t.panes?.find((x) => x.id === t.activePane) ?? t.panes?.[0];
          const s = p && shells.get(p.id);
          if (p && s) out.push({ pane: p.id, lines: s.lastLines(6) });
        }
      }
      for (const b of devbox.closed) {
        for (const t of b.tabs) {
          const p = t.panes?.[0];
          if (p) out.push({ pane: p.id, lines: ['you@devbox:~/website$ npm run dev', '', '  VITE v7.1.2  ready in 412 ms', '', '  ➜  Local:   http://localhost:5173/'] });
        }
      }
      return { type: 'Previews', content: out };
    }

    // ---- tabs
    case 'NewTab': {
      const bench = benchRef(call.content.bench);
      const running = call.content.bench.type === 'Running';
      const k = call.content.kind;
      let tab: Tab;
      switch (k.type) {
        case 'Terminal':
          tab = terminalTab(k.content.folder || bench.cwd, [pane(k.content.folder || bench.cwd)]);
          break;
        case 'Command':
          tab = terminalTab(k.content.folder || bench.cwd, [pane(k.content.folder || bench.cwd, { tool: k.content.command })]);
          break;
        case 'Project':
          tab = projectTab(k.content.folder);
          tab.kind = { type: 'Project', content: { root: k.content.folder, open: [], active: null, links: [] } };
          break;
        case 'Ai':
          tab = aiTab(k.content.folder);
          break;
        case 'Remote': {
          tab = remoteTab(k.content.host);
          paneHost.set(tab.panes![0]!.id, k.content.host);
          break;
        }
      }
      bench.tabs.push(tab);
      if (running) {
        (bench as Bench).activeTab = tab.id;
        startTab(tab);
        if (k.type === 'Command') shells.get(tab.panes![0]!.id)?.run(k.content.command);
      }
      return { type: 'Tab', content: tab.id };
    }
    case 'CloseTab': {
      const { bench, tab, running } = findTab(call.content.tab);
      bench.tabs = bench.tabs.filter((t) => t !== tab);
      if (running) {
        stopTab(tab);
        const b = bench as Bench;
        if (b.activeTab === tab.id) b.activeTab = b.tabs[0]?.id ?? '';
      }
      return DONE;
    }
    case 'RenameTab': {
      const { tab } = findTab(call.content.tab);
      const name = call.content.name?.trim() || null;
      tab.name = name;
      tab.title = name ?? defaultTitle(tab);
      return DONE;
    }
    case 'SetTags':
      findTab(call.content.tab).tab.tags = call.content.tags;
      return DONE;
    case 'SetPrivate':
      findTab(call.content.tab).tab.private = call.content.private;
      return DONE;
    case 'MoveTab': {
      const { tab, bench: from } = findTab(call.content.tab);
      const to = benchRef(call.content.to);
      from.tabs = from.tabs.filter((t) => t !== tab);
      to.tabs.splice(Math.min(call.content.index, to.tabs.length), 0, tab);
      return DONE;
    }
    case 'SetDivider':
      findTab(call.content.tab).tab.divider = call.content.divider;
      return DONE;
    case 'FocusTab': {
      const { bench, tab } = runningTab(call.content.tab);
      bench.activeTab = tab.id;
      return DONE;
    }
    case 'ReconnectRemote': {
      const { tab } = runningTab(call.content.tab);
      if (tab.kind.type !== 'Remote') throw new Fail('invalid', 'not a remote tab');
      for (const p of tab.panes ?? []) shells.get(p.id)?.print(`Reconnecting to ${tab.kind.content.host}…`, { fg: '8' });
      return DONE;
    }

    // ---- terminals
    case 'Split': {
      const { tab, pane: at } = findPane(call.content.pane);
      const p = pane(call.content.folder || at.cwd, { cols: at.cols, rows: at.rows });
      tab.panes = [...(tab.panes ?? []), p];
      tab.layout = withSplit(tab.layout ?? { type: 'Pane', content: at.id }, at.id, {
        type: 'Split',
        content: { dir: call.content.dir, ratio: 0.5, first: { type: 'Pane', content: at.id }, second: { type: 'Pane', content: p.id } },
      });
      tab.activePane = p.id;
      shellFor(p, paneHost.get(at.id));
      return { type: 'Pane', content: p.id };
    }
    case 'ClosePane': {
      const { bench, tab, pane: p } = findPane(call.content.pane);
      const layout = tab.layout ? withoutPane(tab.layout, p.id) : null;
      if (!layout) {
        bench.tabs = bench.tabs.filter((t) => t !== tab);
        stopTab(tab);
        if (bench.activeTab === tab.id) bench.activeTab = bench.tabs[0]?.id ?? '';
        return DONE;
      }
      tab.layout = layout;
      tab.panes = tab.panes!.filter((x) => x !== p);
      if (tab.activePane === p.id) tab.activePane = tab.panes[0]?.id ?? null;
      shells.get(p.id)?.dispose();
      shells.delete(p.id);
      hooks.endTopics([`screen:${p.id}`]);
      return DONE;
    }
    case 'FocusPane': {
      const { tab, pane: p } = findPane(call.content.pane);
      tab.activePane = p.id;
      return DONE;
    }
    case 'StartNow':
    case 'CancelAsk':
      findPane(call.content.pane);
      return DONE;
    case 'Input': {
      const { pane: p } = findPane(call.content.pane);
      shellFor(p).feed(call.content.input);
      return DONE;
    }
    case 'TakeSize': {
      const { pane: p } = findPane(call.content.pane);
      p.cols = Math.max(10, Math.min(500, call.content.cols));
      p.rows = Math.max(3, Math.min(200, call.content.rows));
      p.sizeHeld = true;
      shellFor(p).resize(p.cols, p.rows);
      return DONE;
    }
    case 'ReleaseSize': {
      const { pane: p } = findPane(call.content.pane);
      p.cols = 80;
      p.rows = 24;
      p.sizeHeld = false;
      shellFor(p).resize(80, 24);
      return DONE;
    }
    case 'Scrollback': {
      const { pane: p } = findPane(call.content.pane);
      return { type: 'Lines', content: shellFor(p).scrollback(call.content.from, call.content.count) };
    }

    // ---- logbook
    case 'LogSearch': {
      const places: Place[] = [];
      for (const b of devbox.benches) for (const t of b.tabs) if (!t.private && t.kind.type !== 'Ai' && t.kind.type !== 'Project') places.push(placeOf(b, t));
      const here = call.content.tab ? (() => {
        const f = findTab(call.content.tab!);
        return placeOf(f.bench, f.tab);
      })() : null;
      return { type: 'Log', content: logSearch(places, call.content.scope, here, call.content.query ?? '', call.content.limit ?? 0) };
    }

    // ---- projects
    case 'ListFolder':
      projectTabOf(call.content.tab);
      return { type: 'Folder', content: project.listFolder(call.content.path) };
    case 'QuickOpen':
      projectTabOf(call.content.tab);
      return { type: 'Files', content: project.quickOpen(call.content.query, call.content.limit ?? 0) };
    case 'ReadFile':
      projectTabOf(call.content.tab);
      return { type: 'File', content: project.readFile(call.content.path) };
    case 'WriteFile':
      projectTabOf(call.content.tab);
      project.filesChanged(project.writeFile(call.content.path, call.content.text));
      return DONE;
    case 'CreateEntry':
      projectTabOf(call.content.tab);
      project.filesChanged(project.createEntry(call.content.path));
      return DONE;

    // ---- git
    case 'GitStatus':
      projectTabOf(call.content.tab);
      return { type: 'GitStatus', content: project.gitStatus() };
    case 'GitDiff':
      projectTabOf(call.content.tab);
      return { type: 'Diff', content: project.gitDiff(call.content.path, call.content.source) };
    case 'GitStage':
      projectTabOf(call.content.tab);
      project.gitStage(call.content.paths);
      return DONE;
    case 'GitUnstage':
      projectTabOf(call.content.tab);
      project.gitUnstage(call.content.paths);
      return DONE;
    case 'GitDiscard':
      projectTabOf(call.content.tab);
      project.gitDiscard(call.content.paths);
      return DONE;
    case 'GitHunk':
      projectTabOf(call.content.tab);
      project.gitHunk(call.content);
      return DONE;
    case 'GitCommit':
      projectTabOf(call.content.tab);
      return { type: 'Text', content: project.gitCommit(call.content.message, call.content.amend ?? false) };
    case 'GitLog':
      projectTabOf(call.content.tab);
      return { type: 'Commits', content: project.gitLog(call.content.path, call.content.limit ?? 0) };
    case 'GitCommitFiles':
      projectTabOf(call.content.tab);
      return { type: 'CommitFiles', content: project.gitCommitFiles(call.content.hash) };
    case 'GitBranches':
      projectTabOf(call.content.tab);
      return { type: 'Branches', content: project.gitBranches() };
    case 'GitSwitch':
      projectTabOf(call.content.tab);
      project.gitSwitch(call.content.branch);
      return DONE;
    case 'GitCreateBranch':
      projectTabOf(call.content.tab);
      project.gitCreateBranch(call.content.branch);
      return DONE;
    case 'GitFetch':
      projectTabOf(call.content.tab);
      return { type: 'Text', content: await project.gitRemote('fetch') };
    case 'GitPull':
      projectTabOf(call.content.tab);
      return { type: 'Text', content: await project.gitRemote('pull') };
    case 'GitPush':
      projectTabOf(call.content.tab);
      return { type: 'Text', content: await project.gitRemote('push', call.content.force ?? false) };

    // ---- the machine
    case 'BrowseFolders':
      return { type: 'Folder', content: browse(call.content.path) };
    case 'Browse':
      return { type: 'Browsed', content: browsed(call.content.path) };

    // ---- AI tabs
    case 'AiPicture':
    case 'AiRenameSession':
    case 'AiDeleteSession':
    case 'AiOpenSession':
    case 'AiSend':
    case 'AiStop':
    case 'AiAnswer':
    case 'AiAnswerQuestions':
    case 'AiEdit':
    case 'AiRevert':
    case 'AiQueued':
    case 'AiSettings':
    case 'AiSetUnsent':
    case 'AiAddDraft':
    case 'AiRemoveDraft':
    case 'AiSetViewed':
    case 'AiExplain':
    case 'AiUpload': {
      const { tab } = runningTab(call.content.tab);
      return aiFor(tab).call(call);
    }
    case 'AiSessions':
    case 'AiReview':
    case 'AiStart': {
      const { tab } = runningTab(call.content.tab);
      return aiFor(tab).call(call);
    }
  }
}

// ------------------------------------------------------------------ topics

/** The key subscriptions are ended by (see `WorldHooks.endTopics`). */
export function topicKey(topic: Topic): string {
  switch (topic.type) {
    case 'Ai':
      return `ai:${topic.content.tab}`;
    case 'Screen':
      return `screen:${topic.content.pane}`;
    case 'Project':
      return `project:${topic.content.tab}`;
    case 'Search':
      return `project:${topic.content.tab}`;
  }
}

/** Starts a topic's events; returns how to stop them. */
export function subscribeTopic(workspace: string, topic: Topic, emit: (e: Event) => void): () => void {
  const info = workspaces.find((w) => w.id === workspace);
  if (!info) throw new Fail('notFound', `no workspace ${workspace}`);
  if (!info.online) throw new Fail('offline', `${info.name} is not connected`);
  switch (topic.type) {
    case 'Screen': {
      const { tab, pane: p } = findPane(topic.content.pane);
      if (tab.private) throw new Fail('forbidden', 'a private tab');
      return shellFor(p).subscribe((screen) => emit({ type: 'Screen', content: screen }));
    }
    case 'Ai': {
      const { tab } = runningTab(topic.content.tab);
      if (tab.private) throw new Fail('forbidden', 'a private tab');
      return aiFor(tab).subscribe(emit);
    }
    case 'Project':
      projectTabOf(topic.content.tab);
      return project.subscribeProject(emit);
    case 'Search':
      projectTabOf(topic.content.tab);
      return project.subscribeSearch(topic.content, emit);
  }
}
