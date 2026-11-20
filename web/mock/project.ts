// A pretend project (files in memory) and its Git repository, shared by every
// project and AI tab of the mock: listing, quick open, reading with colours,
// searching, and Git status, diffs, staging, commits, branches.

import type {
  Branches,
  Commit,
  CommitFile,
  Diff,
  DiffSource,
  Event,
  FileContent,
  FileMatch,
  FolderEntry,
  GitHunk,
  GitStatus,
  Highlight,
  Hunk,
  SearchTopic,
} from '../src/api/gen/api.ts';
import { Fail, hex, now, sleep } from './util.ts';
import { gradientPng } from './png.ts';

// ------------------------------------------------------------------ files

const MAIN_RS = `//! Shed's window process: the command line and the app.

use std::path::PathBuf;

mod ui;

/// What to open when Shed starts.
#[derive(Debug, Default)]
pub struct Args {
    pub dir: Option<PathBuf>,
    pub bench: Option<String>,
}

fn main() {
    let args = parse_args(std::env::args().skip(1));
    let count = 42;
    if let Some(dir) = &args.dir {
        println!("opening {} ({count})", dir.display());
    }
    ui::run(args);
}

fn parse_args(mut it: impl Iterator<Item = String>) -> Args {
    let mut args = Args::default();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--bench" => args.bench = it.next(),
            other => args.dir = Some(PathBuf::from(other)),
        }
    }
    args
}
`;

const LIB_RS = `//! The bench model: benches, tabs, panes.

pub mod color;

/// A bench: a project's window, colour and tabs.
pub struct Bench {
    pub name: String,
    pub tabs: Vec<Tab>,
}

impl Bench {
    pub fn new(name: &str) -> Self {
        Bench { name: name.to_string(), tabs: Vec::new() }
    }

    /// How many tabs it has.
    pub fn len(&self) -> usize {
        self.tabs.len()
    }
}

pub struct Tab {
    pub title: String,
}
`;

const UI_RS = `//! Drawing: one window per bench.

use crate::Args;

pub fn run(args: Args) {
    let title = args.bench.unwrap_or_else(|| "Shed".into());
    println!("window: {title}");
}
`;

const CARGO_TOML = `[package]
name = "shed"
version = "0.1.0"
edition = "2024"

# Kept small on purpose.
[dependencies]
serde = { version = "1", features = ["derive"] }
toml = "0.8"
`;

const README = `# Shed

A workbench for Linux and macOS: per project a **bench** with its own window,
colour and tabs.

## Building

\`\`\`sh
cargo build
\`\`\`

See [the guide](docs/guide.md).
`;

const GUIDE = `# Guide

## Benches

Each bench has tabs: terminals, projects, AI tabs and remote shells.

## Keys

- \`Ctrl+Shift+T\`: new tab
- \`Ctrl+R\`: the logbook
`;

const PACKAGE_JSON = `{
  "name": "shed-web",
  "version": "0.1.0",
  "private": true,
  "scripts": {
    "start": "ng serve",
    "build": "ng build"
  }
}
`;

type FileBody = { text: string } | { picture: string } | { binary: number };

const files = new Map<string, FileBody>([
  ['README.md', { text: README }],
  ['Cargo.toml', { text: CARGO_TOML }],
  ['.gitignore', { text: '/target\nnode_modules\n' }],
  ['src/main.rs', { text: MAIN_RS }],
  ['src/lib.rs', { text: LIB_RS }],
  ['src/ui/mod.rs', { text: UI_RS }],
  ['web/package.json', { text: PACKAGE_JSON }],
  ['docs/guide.md', { text: GUIDE }],
  ['docs/new.md', { text: '# Notes\n\nWhat changed this week.\n' }],
  ['assets/logo.png', { picture: gradientPng(96, 96, '#FF9A5A', '#F0475E') }],
  ['assets/font.bin', { binary: 18432 }],
]);
/** Folders with nothing listed in them, and ignored ones. */
const emptyDirs = new Set<string>(['target']);
const ignored = new Set<string>(['target']);

function dirsOf(path: string): string[] {
  const parts = path.split('/');
  return parts.slice(0, -1).map((_, i) => parts.slice(0, i + 1).join('/'));
}

function allDirs(): Set<string> {
  const out = new Set<string>(emptyDirs);
  for (const p of files.keys()) for (const d of dirsOf(p)) out.add(d);
  for (const d of emptyDirs) for (const p of dirsOf(d + '/x')) out.add(p);
  return out;
}

function clean(path: string): string {
  return path.replace(/^\/+|\/+$/g, '').replace(/\/+/g, '/');
}

export function listFolder(path: string): FolderEntry[] {
  const dir = clean(path);
  const dirs = allDirs();
  if (dir && !dirs.has(dir)) throw new Fail('notFound', `no folder ${dir}`);
  const prefix = dir ? dir + '/' : '';
  const names = new Map<string, boolean>();
  for (const d of dirs) if (d.startsWith(prefix) && !d.slice(prefix.length).includes('/') && d !== dir) names.set(d.slice(prefix.length), true);
  for (const f of files.keys()) if (f.startsWith(prefix) && !f.slice(prefix.length).includes('/')) names.set(f.slice(prefix.length), false);
  return [...names.entries()]
    .map(([name, isDir]) => ({ name, dir: isDir, ignored: ignored.has(prefix + name) }))
    .sort((a, b) => (a.dir === b.dir ? a.name.localeCompare(b.name) : a.dir ? -1 : 1));
}

/** Files whose path holds the query's letters in order; tighter and later matches first. */
export function quickOpen(query: string, limit: number): FileMatch[] {
  const q = query.toLowerCase().replace(/\s+/g, '');
  const out: (FileMatch & { score: number })[] = [];
  for (const path of files.keys()) {
    const lower = path.toLowerCase();
    const hits: number[] = [];
    let from = 0;
    for (const ch of q) {
      const i = lower.indexOf(ch, from);
      if (i < 0) break;
      hits.push(i);
      from = i + 1;
    }
    if (hits.length !== q.length) continue;
    const spread = hits.length ? hits[hits.length - 1]! - hits[0]! : 0;
    const inName = hits.filter((h) => h > path.lastIndexOf('/')).length;
    out.push({ path, hits, score: inName * 10 - spread - path.length / 10 });
  }
  return out
    .sort((a, b) => b.score - a.score)
    .slice(0, limit || 50)
    .map(({ path, hits }) => ({ path, hits }));
}

function langOf(path: string): string | null {
  const ext = path.split('.').pop() ?? '';
  return ({ rs: 'Rust', toml: 'TOML', json: 'JSON', md: 'Markdown' } as Record<string, string>)[ext] ?? null;
}

const RULES: Record<string, [RegExp, string][]> = {
  Rust: [
    [/\/\/.*$/g, 'comment'],
    [/"(?:\\.|[^"\\])*"/g, 'string'],
    [/#!?\[[^\]]*\]/g, 'attribute'],
    [/\b(?:fn|let|mut|pub|use|mod|struct|enum|impl|match|if|else|for|while|loop|return|self|Self|crate|const|static|async|await|move|ref|in|as|where|trait|type|dyn|Some|None)\b/g, 'keyword'],
    [/\b[A-Z][A-Za-z0-9_]*\b/g, 'type'],
    [/\b\d+(?:\.\d+)?\b/g, 'number'],
    [/\b[a-z_][a-z0-9_]*(?=\s*[(!])/g, 'function'],
  ],
  TOML: [
    [/#.*$/g, 'comment'],
    [/"(?:\\.|[^"\\])*"/g, 'string'],
    [/^\s*\[[^\]]*\]/g, 'type'],
    [/^[\w.-]+(?=\s*=)/g, 'property'],
    [/\b(?:true|false)\b/g, 'keyword'],
    [/\b\d+(?:\.\d+)?\b/g, 'number'],
  ],
  JSON: [
    [/"(?:\\.|[^"\\])*"(?=\s*:)/g, 'property'],
    [/"(?:\\.|[^"\\])*"/g, 'string'],
    [/\b(?:true|false|null)\b/g, 'keyword'],
    [/-?\b\d+(?:\.\d+)?\b/g, 'number'],
  ],
  Markdown: [
    [/^#+ .*$/g, 'keyword'],
    [/`[^`]*`/g, 'string'],
    [/\[[^\]]*\]\([^)]*\)/g, 'function'],
    [/\*\*[^*]+\*\*/g, 'type'],
  ],
};

/** Per line, coloured ranges; earlier rules win where they overlap. */
export function highlight(text: string, lang: string | null): Highlight[][] {
  const rules = lang ? RULES[lang] : undefined;
  return text.split('\n').map((l) => {
    if (!rules) return [];
    const taken: Highlight[] = [];
    for (const [re, kind] of rules) {
      re.lastIndex = 0;
      for (let m = re.exec(l); m; m = re.exec(l)) {
        if (m[0].length === 0) {
          re.lastIndex++;
          continue;
        }
        const start = m.index;
        const end = start + m[0].length;
        if (!taken.some((h) => start < h.end && end > h.start)) taken.push({ start, end, kind });
      }
    }
    return taken.sort((a, b) => a.start - b.start);
  });
}

export function readFile(path: string): FileContent {
  const p = clean(path);
  const body = files.get(p);
  if (!body) throw new Fail('notFound', `no file ${p}`);
  if ('picture' in body) {
    return { path: p, kind: 'picture', picture: { mediaType: 'image/png', data: body.picture }, size: Math.round((body.picture.length * 3) / 4) };
  }
  if ('binary' in body) return { path: p, kind: 'binary', size: body.binary };
  const lang = langOf(p);
  return { path: p, kind: 'text', text: body.text, lang, highlights: highlight(body.text, lang), size: Buffer.byteLength(body.text) };
}

export function fileText(path: string): string | null {
  const body = files.get(clean(path));
  return body && 'text' in body ? body.text : null;
}

export function writeFile(path: string, text: string): string[] {
  const p = clean(path);
  if (!p) throw new Fail('invalid', 'no path');
  files.set(p, { text });
  return [p];
}

export function createEntry(path: string): string[] {
  const folder = path.endsWith('/');
  const p = clean(path);
  if (!p) throw new Fail('invalid', 'no path');
  if (files.has(p) || allDirs().has(p)) throw new Fail('invalid', `${p} is there already`);
  if (folder) emptyDirs.add(p);
  else files.set(p, { text: '' });
  return [p];
}

// ------------------------------------------------------------------ project topic

const projectSubs = new Set<(e: Event) => void>();

export function subscribeProject(emit: (e: Event) => void): () => void {
  projectSubs.add(emit);
  emit({ type: 'FilesChanged', content: { paths: [] } });
  return () => projectSubs.delete(emit);
}

export function filesChanged(paths: string[]): void {
  for (const emit of projectSubs) emit({ type: 'FilesChanged', content: { paths } });
}

function gitChanged(): void {
  for (const emit of projectSubs) emit({ type: 'GitChanged' });
}

// ------------------------------------------------------------------ search

/** Streams hits file by file, ~100 ms apart, then `SearchDone`; stops when unsubscribed. */
export function subscribeSearch(topic: SearchTopic, emit: (e: Event) => void): () => void {
  let stopped = false;
  void (async () => {
    let re: RegExp;
    try {
      const source = topic.regex ? topic.query : topic.query.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
      re = new RegExp(topic.wholeWord ? `\\b(?:${source})\\b` : source, topic.caseSensitive ? 'g' : 'gi');
    } catch (e) {
      emit({ type: 'SearchDone', content: { truncated: false, error: e instanceof Error ? e.message : String(e) } });
      return;
    }
    const include = topic.include ? globRe(topic.include) : null;
    const exclude = topic.exclude ? globRe(topic.exclude) : null;
    for (const [path, body] of files) {
      if (stopped) return;
      if (!('text' in body) || !topic.query) continue;
      if (include && !include.test(path)) continue;
      if (exclude && exclude.test(path)) continue;
      const hits = body.text.split('\n').flatMap((text, i) => {
        const ranges: number[] = [];
        re.lastIndex = 0;
        for (let m = re.exec(text); m; m = re.exec(text)) {
          if (m[0].length === 0) {
            re.lastIndex++;
            continue;
          }
          ranges.push(m.index, m.index + m[0].length);
        }
        return ranges.length ? [{ line: i + 1, text, ranges }] : [];
      });
      if (!hits.length) continue;
      await sleep(100);
      if (stopped) return;
      emit({ type: 'SearchHits', content: { path, hits } });
    }
    await sleep(50);
    if (!stopped) emit({ type: 'SearchDone', content: { truncated: false } });
  })();
  return () => {
    stopped = true;
  };
}

/** `*.rs, docs/**` → one pattern matching any of them. */
function globRe(globs: string): RegExp {
  const parts = globs
    .split(',')
    .map((g) => g.trim())
    .filter(Boolean)
    .map((g) =>
      g
        .replace(/[.+^${}()|[\]\\]/g, '\\$&')
        .replace(/\*\*/g, '\u0000')
        .replace(/\*/g, '[^/]*')
        .replace(/\?/g, '.')
        .replace(/\u0000/g, '.*'),
    );
  return new RegExp(`(^|/)(?:${parts.join('|')})$`);
}

// ------------------------------------------------------------------ git

interface Change {
  path: string;
  /** M, A, D, or ? for a file Git does not track. */
  kind: string;
  staged: Hunk[];
  unstaged: Hunk[];
}

function hunk(header: string, oldStart: number, newStart: number, lines: [' ' | '+' | '-', string][]): Hunk {
  let o = oldStart;
  let n = newStart;
  return {
    header,
    lines: lines.map(([sign, text]) => {
      if (sign === ' ') return { kind: 'context', text, old: o++, new: n++ };
      if (sign === '+') return { kind: 'added', text, old: null, new: n++ };
      return { kind: 'removed', text, old: o++, new: null };
    }),
  };
}

const changes: Change[] = [
  {
    path: 'src/lib.rs',
    kind: 'M',
    staged: [
      hunk('@@ -1,4 +1,5 @@', 1, 1, [
        [' ', '//! The bench model: benches, tabs, panes.'],
        [' ', ''],
        ['+', 'pub mod color;'],
        [' ', ''],
        [' ', '/// A bench: a project\'s window, colour and tabs.'],
      ]),
    ],
    unstaged: [
      hunk('@@ -14,6 +15,11 @@ impl Bench {', 14, 15, [
        [' ', '        Bench { name: name.to_string(), tabs: Vec::new() }'],
        [' ', '    }'],
        ['+', ''],
        ['+', '    /// How many tabs it has.'],
        ['+', '    pub fn len(&self) -> usize {'],
        ['+', '        self.tabs.len()'],
        ['+', '    }'],
        [' ', '}'],
      ]),
    ],
  },
  {
    path: 'src/main.rs',
    kind: 'M',
    staged: [],
    unstaged: [
      hunk('@@ -12,8 +12,9 @@ pub struct Args {', 12, 12, [
        [' ', ''],
        [' ', 'fn main() {'],
        [' ', '    let args = parse_args(std::env::args().skip(1));'],
        ['-', '    let count = 41;'],
        ['+', '    let count = 42;'],
        [' ', '    if let Some(dir) = &args.dir {'],
        ['-', '        println!("opening {}", dir.display());'],
        ['+', '        println!("opening {} ({count})", dir.display());'],
        [' ', '    }'],
      ]),
      hunk('@@ -24,7 +25,7 @@ fn parse_args(mut it: impl Iterator<Item = String>) -> Args {', 24, 25, [
        [' ', '    while let Some(arg) = it.next() {'],
        [' ', '        match arg.as_str() {'],
        ['-', '            "-b" => args.bench = it.next(),'],
        ['+', '            "--bench" => args.bench = it.next(),'],
        [' ', '            other => args.dir = Some(PathBuf::from(other)),'],
        [' ', '        }'],
      ]),
    ],
  },
  {
    path: 'docs/new.md',
    kind: '?',
    staged: [],
    unstaged: [hunk('@@ -0,0 +1,3 @@', 0, 1, [['+', '# Notes'], ['+', ''], ['+', 'What changed this week.']])],
  },
  {
    path: 'Cargo.toml',
    kind: 'M',
    staged: [
      hunk('@@ -6,3 +6,4 @@ edition = "2024"', 6, 6, [
        [' ', '# Kept small on purpose.'],
        [' ', '[dependencies]'],
        [' ', 'serde = { version = "1", features = ["derive"] }'],
        ['+', 'toml = "0.8"'],
      ]),
    ],
    unstaged: [],
  },
];

const SUBJECTS = [
  'Pictures open with zoom and show side by side in diffs',
  'Project tabs can make new files and folders',
  'AI sessions show their whole title on hover',
  'Remote tabs reconnect after a dropped network',
  'The logbook finds commands by tag',
  'Bench windows reopen where they were',
  'Usage limits show how fast they are used',
  'Splits can be resized by dragging',
  'Quick open ranks file names first',
  'Search in files streams its results',
  'Themes follow the system',
  'Closed benches can be edited in place',
  'Tabs can be grouped with dividers',
  'Private tabs hide their content',
  'First version of benches',
];

const log: Commit[] = SUBJECTS.map((subject, i) => {
  const hash = hex(20);
  return { hash, short: hash.slice(0, 7), subject, author: 'You', time: now() - 3600 * (i * 9 + 3) };
});
let outgoing: Commit[] = [log[0]!];
let ahead = 1;
let behind = 0;
let branch = 'main';
const local = ['main', 'feature/web'];

function letter(c: Change, staged: boolean): string {
  if (c.kind === '?') return staged ? '?' : '?';
  if (staged) return c.staged.length ? c.kind : ' ';
  return c.unstaged.length ? c.kind : ' ';
}

export function gitStatus(): GitStatus {
  return {
    repo: true,
    branch,
    upstream: `origin/${branch}`,
    ahead,
    behind,
    operation: null,
    files: changes.map((c) => ({ path: c.path, staged: letter(c, true), unstaged: letter(c, false), conflict: false, from: null })),
    outgoing,
  };
}

function find(path: string): Change {
  const c = changes.find((x) => x.path === path);
  if (!c) throw new Fail('notFound', `no changes in ${path}`);
  return c;
}

function tidy(): void {
  for (let i = changes.length - 1; i >= 0; i--) {
    const c = changes[i]!;
    if (!c.staged.length && !c.unstaged.length) changes.splice(i, 1);
  }
  gitChanged();
}

export function gitDiff(path: string, source: DiffSource): Diff {
  if (source.type === 'Commit') return commitDiff(source.content, path);
  if (path === 'assets/logo.png') {
    return { hunks: [], binary: true, oldPicture: null, newPicture: { mediaType: 'image/png', data: gradientPng(96, 96, '#5CB2FF', '#2459E8') } };
  }
  const c = changes.find((x) => x.path === path);
  if (!c) return { hunks: [] };
  return { hunks: source.type === 'Staged' ? c.staged : c.unstaged };
}

export function gitStage(paths: string[]): void {
  for (const p of paths) {
    const c = find(p);
    if (c.kind === '?') c.kind = 'A';
    c.staged.push(...c.unstaged);
    c.unstaged = [];
  }
  tidy();
}

export function gitUnstage(paths: string[]): void {
  for (const p of paths) {
    const c = find(p);
    c.unstaged.unshift(...c.staged);
    c.staged = [];
    if (c.kind === 'A') c.kind = '?';
  }
  tidy();
}

export function gitDiscard(paths: string[]): void {
  for (const p of paths) {
    const c = find(p);
    if (c.kind === '?') files.delete(p);
    c.unstaged = [];
  }
  tidy();
  filesChanged(paths);
}

export function gitHunk(h: GitHunk): void {
  const c = find(h.path);
  const from = h.source.type === 'Staged' ? c.staged : c.unstaged;
  const target = from[h.hunk];
  if (!target || target.header !== h.header) return;
  from.splice(h.hunk, 1);
  if (h.action === 'stage') {
    if (c.kind === '?') c.kind = 'A';
    c.staged.push(target);
  } else if (h.action === 'unstage') c.unstaged.push(target);
  tidy();
}

export function gitCommit(message: string, amend: boolean): string {
  const subject = message.split('\n')[0]!.trim();
  if (!subject) throw new Fail('invalid', 'the message is empty');
  const staged = changes.filter((c) => c.staged.length);
  if (!staged.length && !amend) throw new Fail('invalid', 'nothing staged');
  for (const c of staged) c.staged = [];
  let commit: Commit;
  if (amend && log[0]) {
    const hash = hex(20);
    commit = { ...log[0], hash, short: hash.slice(0, 7), subject, time: now() };
    log[0] = commit;
    outgoing = [commit, ...outgoing.slice(1)];
    if (!ahead) ahead = 1;
  } else {
    const hash = hex(20);
    commit = { hash, short: hash.slice(0, 7), subject, author: 'You', time: now() };
    log.unshift(commit);
    outgoing = [commit, ...outgoing];
    ahead += 1;
  }
  tidy();
  return commit.hash;
}

export function gitLog(path: string | null | undefined, limit: number): Commit[] {
  const list = path ? log.filter((_, i) => i % 3 === 0) : log;
  return list.slice(0, limit || 100);
}

const COMMIT_FILES = ['src/main.rs', 'src/lib.rs', 'docs/guide.md', 'Cargo.toml', 'README.md', 'src/ui/mod.rs'];

export function gitCommitFiles(hash: string): CommitFile[] {
  const n = (parseInt(hash.slice(0, 2), 16) % 3) + 1;
  const start = parseInt(hash.slice(2, 4), 16) % COMMIT_FILES.length;
  return Array.from({ length: n }, (_, i) => ({
    status: i === 2 ? 'A' : 'M',
    path: COMMIT_FILES[(start + i) % COMMIT_FILES.length]!,
  }));
}

function commitDiff(hash: string, path: string): Diff {
  const c = log.find((x) => x.hash === hash || x.short === hash);
  if (!c) throw new Fail('notFound', `no commit ${hash}`);
  return {
    hunks: [
      hunk('@@ -3,6 +3,7 @@', 3, 3, [
        [' ', `// ${path}`],
        [' ', ''],
        ['-', '// before: ' + c.subject.toLowerCase()],
        ['+', '// after: ' + c.subject.toLowerCase()],
        ['+', '// (and one more line)'],
        [' ', ''],
      ]),
    ],
  };
}

export function gitBranches(): Branches {
  return { current: branch, local: [...local], remote: ['origin/main'] };
}

export function gitSwitch(name: string): void {
  if (!local.includes(name)) throw new Fail('notFound', `no branch ${name}`);
  branch = name;
  gitChanged();
}

export function gitCreateBranch(name: string): void {
  if (!/^[\w./-]+$/.test(name)) throw new Fail('invalid', `not a branch name: ${name}`);
  if (!local.includes(name)) local.push(name);
  branch = name;
  gitChanged();
}

export async function gitRemote(what: 'fetch' | 'pull' | 'push', force = false): Promise<string> {
  await sleep(800);
  let text: string;
  if (what === 'fetch') {
    text = 'From github.com:example/shed\n   1a2b3c4..5d6e7f8  main       -> origin/main';
  } else if (what === 'pull') {
    text = behind ? `Updating 1a2b3c4..5d6e7f8\nFast-forward\n src/main.rs | 2 +-\n 1 file changed, 1 insertion(+), 1 deletion(-)` : 'Already up to date.';
    behind = 0;
  } else {
    const n = ahead;
    text = n
      ? `Enumerating objects: ${n * 5}, done.\nWriting objects: 100% (${n * 3}/${n * 3}), 1.21 KiB | 1.21 MiB/s, done.\nTo github.com:example/shed.git\n   1a2b3c4..${log[0]!.short}  ${branch} -> ${branch}${force ? ' (forced update)' : ''}`
      : 'Everything up-to-date';
    ahead = 0;
    outgoing = [];
  }
  gitChanged();
  return text;
}
