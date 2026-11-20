// An AI tab acted out: a conversation that streams, uses tools, asks for
// permission or asks questions, and the calls that drive it, as the link
// would serve them from Claude Code.

import type {
  AiActivity,
  AiCommand,
  AiEntry,
  AiModel,
  AiQuestion,
  AiSession,
  AiStatus,
  AiTab,
  Call,
  Event,
  Picture,
  Reply,
  Review,
  ReviewChange,
} from '../src/api/gen/api.ts';
import { Fail, env, hex, now, sleep, uid } from './util.ts';
import { gradientPng } from './png.ts';

const COMMANDS: AiCommand[] = [
  { name: 'compact', description: 'Clear the conversation but keep a summary in context', hint: '[instructions]' },
  { name: 'clear', description: 'Clear the conversation and free up context' },
  { name: 'review', description: 'Review a pull request' },
  { name: 'init', description: 'Write a CLAUDE.md with notes about this codebase' },
  { name: 'model', description: 'Set the model for this session', hint: '[model]' },
];

const MODELS: AiModel[] = [
  { value: '', name: 'Default (Opus 5.5)', description: 'The most capable, for complex work', efforts: ['low', 'medium', 'high', 'xhigh'] },
  { value: 'opus', name: 'Opus 5.5', description: 'The most capable, for complex work', efforts: ['low', 'medium', 'high', 'xhigh'] },
  { value: 'sonnet', name: 'Sonnet 5', description: 'Fast and capable, for everyday work', efforts: ['low', 'medium', 'high'] },
  { value: 'haiku', name: 'Haiku 4.5', description: 'The fastest, for quick answers', efforts: [] },
];

const ANSWER = `## What \`parse_args\` does

It walks the arguments once and fills an \`Args\`:

- \`--bench NAME\` picks the bench to open;
- anything else is taken as the folder.

| Argument | Field | Example |
| --- | --- | --- |
| \`--bench\` | \`bench\` | \`--bench shed\` |
| a path | \`dir\` | \`~/shed\` |

The smallest change that adds a \`--new\` flag:

\`\`\`rust
match arg.as_str() {
    "--bench" => args.bench = it.next(),
    "--new" => args.new = true,
    other => args.dir = Some(PathBuf::from(other)),
}
\`\`\`

Shall I make it?`;

const TEST_OUTPUT = `running 12 tests
test model::tests::bench_names ... ok
test model::tests::tab_order ... ok
test color::tests::parse_named ... ok
test color::tests::parse_hex ... ok

test result: ok. 12 passed; 0 failed; 0 ignored; finished in 0.31s`;

function seedEntries(t0: number): AiEntry[] {
  return [
    { type: 'User', content: { text: 'What does `parse_args` in src/main.rs do? Here is what I see when it starts:', at: t0, pictures: ['shot-1.png'], images: 1 } },
    { type: 'Thinking', content: { text: 'The user asks about parse_args. I should read src/main.rs first, then explain the match arms.' } },
    { type: 'Tool', content: { id: 'tool-1', name: 'Read', summary: 'Read src/main.rs', detail: '{"file_path": "/home/you/shed/src/main.rs"}', file: 'src/main.rs', result: '     1→//! Shed\'s window process: the command line and the app.\n     2→\n     3→use std::path::PathBuf;\n…', error: false } },
    { type: 'Assistant', content: { text: ANSWER, at: t0 + 14 } },
    { type: 'User', content: { text: 'Yes, and add a `len` to Bench while you are at it.', at: t0 + 60, pictures: [], images: 0 } },
    {
      type: 'Tool',
      content: {
        id: 'tool-2',
        name: 'Edit',
        summary: 'Edit src/lib.rs',
        detail: '{"file_path": "/home/you/shed/src/lib.rs"}',
        file: 'src/lib.rs',
        edit: { old: '        Bench { name: name.to_string(), tabs: Vec::new() }\n    }\n', new: '        Bench { name: name.to_string(), tabs: Vec::new() }\n    }\n\n    /// How many tabs it has.\n    pub fn len(&self) -> usize {\n        self.tabs.len()\n    }\n' },
        result: 'The file src/lib.rs has been updated.',
        error: false,
      },
    },
    { type: 'Tool', content: { id: 'tool-3', name: 'Bash', summary: '$ cargo clippy -- -D warnings', detail: 'cargo clippy -- -D warnings', result: 'error: struct `Bench` has a public `len` method, but no `is_empty` method\n  --> src/lib.rs:18:5', error: true } },
    { type: 'Tool', content: { id: 'tool-4', name: 'Bash', summary: '$ cargo test', detail: 'cargo test', result: TEST_OUTPUT, error: false } },
    { type: 'Assistant', content: { text: 'Done: `Bench::len` is in `src/lib.rs`, and the tests pass. Clippy asks for an `is_empty` beside it; say if you want that too.', at: t0 + 102 } },
    { type: 'Notice', content: { text: 'Turn done · 42 s · $0.08', error: false } },
  ];
}

interface Waiting {
  request: string;
  index: number;
  started: number;
}

let draftCounter = 0;

export class AiRuntime {
  readonly tabId: string;
  readonly ai: AiTab;
  private readonly changed: () => void;
  private entries: AiEntry[];
  private readonly sessions: AiSession[];
  /** Conversations of the sessions not shown, by id. */
  private readonly stored = new Map<string, AiEntry[]>();
  private readonly pictures = new Map<string, Picture>();
  private readonly subs = new Set<(e: Event) => void>();
  private status: AiStatus;
  private turn: { cancelled: boolean } | null = null;
  private waiting: Waiting | null = null;
  private review: Review;

  constructor(tabId: string, ai: AiTab, changed: () => void, seeded: boolean) {
    this.tabId = tabId;
    this.ai = ai;
    this.changed = changed;
    const t = now();
    const s1 = uid('s');
    this.sessions = seeded
      ? [
          { id: s1, title: 'Explain parse_args and add Bench::len', modified: t - 300, branch: 'main', prompts: 2, parent: null, depth: 0, draft: false, drafts: 0 },
          { id: uid('s'), title: 'Fix the tab bar wrapping on narrow windows', modified: t - 3 * 3600, branch: 'main', prompts: 7, parent: null, depth: 0, draft: false, drafts: 0 },
          { id: 'fam-parent', title: 'Plan the web app', modified: t - 26 * 3600, branch: 'feature/web', prompts: 31, parent: null, depth: 0, draft: false, drafts: 0 },
          { id: uid('s'), title: 'Plan the web app (continued)', modified: t - 25 * 3600, branch: 'feature/web', prompts: 12, parent: 'fam-parent', depth: 1, draft: false, drafts: 0 },
          { id: uid('s'), title: 'Plan the web app (branched after #29)', modified: t - 24 * 3600, branch: 'feature/web', prompts: 4, parent: 'fam-parent', depth: 2, draft: false, drafts: 0 },
          { id: uid('s'), title: 'Why does the keeper restart twice?', modified: t - 4 * 86400, branch: 'main', prompts: 3, parent: null, depth: 0, draft: false, drafts: 0 },
          { id: 'draft-0', title: 'Ideas for the logbook', modified: t - 2 * 86400, branch: null, prompts: 0, parent: null, depth: 0, draft: true, drafts: 0 },
        ]
      : [];
    if (seeded) {
      ai.session = s1;
      ai.drafts = [
        { session: s1, text: 'Also add `is_empty`, and a test for both.' },
        { session: 'draft-0', text: 'Make tag: queries work with several tags.' },
      ];
    }
    this.entries = seeded ? seedEntries(t - 3600) : [];
    this.pictures.set('shot-1.png', { mediaType: 'image/png', data: gradientPng(160, 100, '#5CB2FF', '#B48BFF') });
    this.status = { session: ai.session ?? null, model: 'claude-opus-5-5', state: 'idle', cost: seeded ? 0.08 : 0, context: seeded ? 84000 : 0, contextMax: 1000000, since: null };
    this.review = makeReview();
  }

  // ---------------------------------------------------------------- topic

  subscribe(emit: (e: Event) => void): () => void {
    this.subs.add(emit);
    emit({ type: 'AiTranscript', content: { from: 0, entries: this.entries, status: this.status } });
    emit({ type: 'AiInfo', content: { commands: COMMANDS, models: MODELS } });
    return () => this.subs.delete(emit);
  }

  private emitFrom(from: number): void {
    const content = { from, entries: this.entries.slice(from), status: { ...this.status } };
    for (const emit of this.subs) emit({ type: 'AiTranscript', content });
  }

  private push(entry: AiEntry): number {
    this.entries.push(entry);
    const i = this.entries.length - 1;
    this.emitFrom(i);
    return i;
  }

  private setActivity(a: AiActivity): void {
    this.ai.activity = a;
    this.status.state = a;
    this.status.since = a === 'idle' ? null : (this.status.since ?? now());
    this.changed();
    this.emitFrom(this.entries.length);
  }

  // ---------------------------------------------------------------- the script

  private startTurn(text: string, pictures: Picture[]): void {
    const names = pictures.map((p) => {
      const name = `pic-${hex(3)}.${p.mediaType.split('/')[1] ?? 'png'}`;
      this.pictures.set(name, p);
      return name;
    });
    this.makeSessionReal(text);
    this.push({ type: 'User', content: { text, at: now(), pictures: names, images: names.length } });
    const tok = { cancelled: false };
    this.turn = tok;
    this.status.since = now();
    this.setActivity('working');
    void this.play(text, tok);
  }

  private async play(text: string, tok: { cancelled: boolean }): Promise<void> {
    const reply = `Looking at that now. ${text.length > 60 ? 'That is a long request, so I will go step by step.' : ''}\n\nFirst I'll check what the code does today, then make the change.`;
    const i = this.push({ type: 'Assistant', content: { text: '', at: now() } });
    const words = reply.split(/(?<= )/);
    let shown = '';
    for (const w of words) {
      await sleep(150);
      if (tok.cancelled) return;
      shown += w;
      this.entries[i] = { type: 'Assistant', content: { text: shown, at: now() } };
      this.emitFrom(i);
    }
    await sleep(300);
    if (tok.cancelled) return;
    const t = this.push({ type: 'Tool', content: { id: uid('tool'), name: 'Grep', summary: 'Grep "parse_args" in src', detail: '{"pattern": "parse_args", "path": "src"}', result: null, error: false } });
    await sleep(900);
    if (tok.cancelled) return;
    const tool = this.entries[t]!;
    if (tool.type === 'Tool') tool.content.result = 'src/main.rs:15:    let args = parse_args(std::env::args().skip(1));\nsrc/main.rs:23:fn parse_args(mut it: impl Iterator<Item = String>) -> Args {';
    this.emitFrom(t);
    await sleep(400);
    if (tok.cancelled) return;
    const request = uid('req');
    if (/ask/i.test(text)) {
      const questions: AiQuestion[] = [
        {
          question: 'Which flag name should the new option use?',
          header: 'Flag',
          multi: false,
          options: [
            { label: '--new', description: 'Short, matches `shed new` (Recommended)' },
            { label: '--fresh', description: 'Says it starts without saved state' },
            { label: '--empty', description: 'Says the bench starts with no tabs' },
          ],
        },
        {
          question: 'Where should it be documented?',
          header: 'Docs',
          multi: true,
          options: [
            { label: 'User guide', description: 'docs/user-guide.md, under The shed command' },
            { label: '--help text', description: 'The clap help of the command' },
            { label: 'README', description: 'The quick start' },
          ],
        },
      ];
      const index = this.push({ type: 'Permission', content: { request, tool: 'AskUserQuestion', summary: 'Claude asks 2 questions', detail: '', answer: null, questions, answers: [] } });
      this.waiting = { request, index, started: now() };
    } else {
      const index = this.push({
        type: 'Permission',
        content: { request, tool: 'Bash', summary: '$ rm -rf target', detail: 'rm -rf target\n\nRemoves the build folder, so the next build starts clean.', answer: null, questions: [], answers: [] },
      });
      this.waiting = { request, index, started: now() };
    }
    this.setActivity('waiting');
  }

  private async finish(allowed: boolean, tok: { cancelled: boolean }, answers?: string[]): Promise<void> {
    this.setActivity('working');
    if (allowed && !answers) {
      await sleep(500);
      if (tok.cancelled) return;
      this.push({ type: 'Tool', content: { id: uid('tool'), name: 'Bash', summary: '$ rm -rf target', detail: 'rm -rf target', result: '', error: false } });
    }
    await sleep(400);
    if (tok.cancelled) return;
    const text = answers
      ? `Thanks. I'll use **${answers[0] || 'your choice'}** and document it in ${answers[1] || 'the guide'}.\n\n\`\`\`rust\n"${answers[0] || '--new'}" => args.new = true,\n\`\`\``
      : allowed
        ? 'Cleaned the build folder. Everything else is as it was.'
        : 'All right, I left `target` alone.';
    const i = this.push({ type: 'Assistant', content: { text: '', at: now() } });
    let shown = '';
    for (const w of text.split(/(?<= )/)) {
      await sleep(120);
      if (tok.cancelled) return;
      shown += w;
      this.entries[i] = { type: 'Assistant', content: { text: shown, at: now() } };
      this.emitFrom(i);
    }
    const secs = now() - (this.status.since ?? now());
    this.status.cost = Math.round((this.status.cost! + 0.03) * 100) / 100;
    this.status.context = Math.min(1000000, (this.status.context ?? 0) + 6200);
    this.push({ type: 'Notice', content: { text: `Turn done · ${secs} s · $0.03`, error: false } });
    this.endTurn();
  }

  private endTurn(): void {
    this.turn = null;
    this.waiting = null;
    this.setActivity('idle');
    const next = this.ai.queued?.shift();
    if (next !== undefined) {
      this.changed();
      setTimeout(() => this.startTurn(next, []), 300);
    }
  }

  private interrupt(): void {
    if (this.turn) this.turn.cancelled = true;
    this.turn = null;
    if (this.waiting) {
      const e = this.entries[this.waiting.index];
      if (e?.type === 'Permission') e.content.answer = false;
      this.waiting = null;
    }
    this.push({ type: 'Notice', content: { text: 'Interrupted', error: true } });
    this.setActivity('idle');
  }

  /** Sending from a new or draft session makes it a real one. */
  private makeSessionReal(text: string): void {
    if (this.ai.session) {
      const s = this.sessions.find((x) => x.id === this.ai.session);
      if (s) {
        s.modified = now();
        s.prompts += 1;
      }
      return;
    }
    const id = uid('s');
    const draft = this.ai.draftSession;
    if (draft) {
      for (const d of this.ai.drafts ?? []) if (d.session === draft) d.session = id;
      const i = this.sessions.findIndex((x) => x.id === draft);
      if (i >= 0) this.sessions.splice(i, 1);
    }
    this.sessions.unshift({ id, title: text.split('\n')[0]!.slice(0, 80), modified: now(), branch: 'main', prompts: 1, parent: null, depth: 0, draft: false, drafts: 0 });
    this.ai.session = id;
    this.ai.draftSession = null;
    this.status.session = id;
    this.changed();
  }

  // ---------------------------------------------------------------- calls

  async call(call: Call): Promise<Reply> {
    switch (call.type) {
      case 'AiStart':
        return { type: 'Done' };
      case 'AiSessions':
        return {
          type: 'Sessions',
          content: this.sessions
            .map((s) => ({ ...s, drafts: (this.ai.drafts ?? []).filter((d) => d.session === s.id).length }))
            .sort((a, b) => (a.draft === b.draft ? 0 : a.draft ? -1 : 1)),
        };
      case 'AiOpenSession': {
        if (this.turn) this.interrupt();
        if (this.ai.session) this.stored.set(this.ai.session, this.entries);
        const id = call.content.session ?? null;
        if (!id) {
          this.ai.session = null;
          this.ai.draftSession = null;
          this.entries = [];
        } else {
          const s = this.sessions.find((x) => x.id === id);
          if (!s) throw new Fail('notFound', `no session ${id}`);
          if (s.draft) {
            this.ai.session = null;
            this.ai.draftSession = id;
            this.entries = [];
          } else {
            this.ai.session = id;
            this.ai.draftSession = null;
            this.entries = this.stored.get(id) ?? [
              { type: 'User', content: { text: s.title, at: s.modified - 120, pictures: [], images: 0 } },
              { type: 'Assistant', content: { text: `This is the session **${s.title}**, read back from its file.`, at: s.modified - 100 } },
              { type: 'Notice', content: { text: 'Turn done · 20 s · $0.02', error: false } },
            ];
          }
        }
        this.status = { ...this.status, session: this.ai.session ?? null, state: 'idle', since: null };
        this.ai.activity = 'idle';
        this.changed();
        this.emitFrom(0);
        return { type: 'Done' };
      }
      case 'AiRenameSession': {
        const s = this.sessions.find((x) => x.id === call.content.session);
        if (!s) throw new Fail('notFound');
        s.title = call.content.name.trim() || 'Untitled session';
        return { type: 'Done' };
      }
      case 'AiSend': {
        const text = call.content.text;
        if (!text.trim() && !(call.content.pictures ?? []).length) throw new Fail('invalid', 'nothing to send');
        if (this.turn || this.ai.activity !== 'idle') {
          this.ai.queued = [...(this.ai.queued ?? []), text];
          this.changed();
        } else {
          this.ai.unsent = '';
          this.startTurn(text, call.content.pictures ?? []);
        }
        return { type: 'Done' };
      }
      case 'AiStop':
        if (this.turn || this.waiting) this.interrupt();
        return { type: 'Done' };
      case 'AiAnswer': {
        const w = this.answerable(call.content.request);
        const e = this.entries[w.index];
        if (e?.type === 'Permission') e.content.answer = call.content.allow;
        this.emitFrom(w.index);
        this.waiting = null;
        void this.finish(call.content.allow, this.turn ?? { cancelled: false });
        return { type: 'Done' };
      }
      case 'AiAnswerQuestions': {
        const w = this.answerable(call.content.request);
        const e = this.entries[w.index];
        if (e?.type === 'Permission') {
          e.content.answers = call.content.answers;
          e.content.answer = call.content.answers.some((a) => a !== '');
        }
        this.emitFrom(w.index);
        this.waiting = null;
        void this.finish(true, this.turn ?? { cancelled: false }, call.content.answers);
        return { type: 'Done' };
      }
      case 'AiEdit': {
        const at = this.userEntry(call.content.entry);
        if (this.turn) this.interrupt();
        this.entries = this.entries.slice(0, at);
        this.emitFrom(at);
        this.startTurn(call.content.text, call.content.pictures ?? []);
        return { type: 'Done' };
      }
      case 'AiRevert': {
        const at = this.userEntry(call.content.entry);
        if (this.turn) {
          this.turn.cancelled = true;
          this.turn = null;
          this.waiting = null;
          this.setActivity('idle');
        }
        this.entries = this.entries.slice(0, at);
        this.entries.push({ type: 'Notice', content: { text: call.content.files ? 'Reverted, files too' : 'Reverted the conversation', error: false } });
        this.emitFrom(at);
        return { type: 'Done' };
      }
      case 'AiQueued': {
        const q = this.ai.queued ?? [];
        const { index, text, send } = call.content;
        if (index >= q.length) throw new Fail('notFound', 'no such queued message');
        if (!text.trim()) q.splice(index, 1);
        else q[index] = text;
        if (send && text.trim()) {
          q.splice(index, 1);
          if (this.turn || this.waiting) this.interrupt();
          this.startTurn(text, []);
        }
        this.ai.queued = q;
        this.changed();
        return { type: 'Done' };
      }
      case 'AiSettings': {
        const s = call.content;
        if (s.model !== undefined && s.model !== null) this.ai.model = s.model === '' ? null : s.model;
        if (s.mode != null) this.ai.mode = s.mode;
        if (s.effort !== undefined && s.effort !== null) this.ai.effort = s.effort === '' ? null : s.effort;
        if (s.follow != null) this.ai.follow = s.follow;
        if (s.browser != null) this.ai.browser = s.browser;
        if (s.addDirs != null) this.ai.addDirs = s.addDirs;
        if (s.model != null) this.status.model = s.model === '' ? 'claude-opus-5-5' : `claude-${s.model}`;
        this.changed();
        this.emitFrom(this.entries.length);
        return { type: 'Done' };
      }
      case 'AiSetUnsent':
        this.ai.unsent = call.content.text;
        this.changed();
        return { type: 'Done' };
      case 'AiAddDraft': {
        let session = call.content.session ?? this.ai.session ?? this.ai.draftSession ?? null;
        if (!session) {
          draftCounter += 1;
          session = `draft-${draftCounter}`;
          this.sessions.unshift({ id: session, title: call.content.text.split('\n')[0]!.slice(0, 80), modified: now(), branch: null, prompts: 0, parent: null, depth: 0, draft: true, drafts: 0 });
          this.ai.draftSession = session;
        }
        this.ai.drafts = [...(this.ai.drafts ?? []), { session, text: call.content.text }];
        this.changed();
        return { type: 'Done' };
      }
      case 'AiRemoveDraft': {
        const d = this.ai.drafts ?? [];
        const { index, text } = call.content;
        if (d[index]?.text === text) {
          d.splice(index, 1);
          this.ai.drafts = d;
          this.changed();
        }
        return { type: 'Done' };
      }
      case 'AiPicture': {
        const p = this.pictures.get(call.content.name);
        if (!p) throw new Fail('notFound', `no picture ${call.content.name}`);
        return { type: 'Picture', content: p };
      }
      case 'AiReview':
        if (env.noReview) throw new Fail('unsupported', 'review is not offered yet');
        return { type: 'Review', content: this.review };
      case 'AiSetViewed': {
        const c = this.findChange(call.content.key);
        c.viewed = call.content.viewed;
        return { type: 'Done' };
      }
      case 'AiExplain': {
        await sleep(1000);
        const out = call.content.keys.map((key) => {
          const c = this.findChange(key);
          c.explanation = `In ${c.path}, this ${c.status === 'A' ? 'adds the file' : 'changes the code'} so that ${c.path.endsWith('.md') ? 'the guide says how the new flag works' : 'benches can say how many tabs they have'}, as you asked in the second prompt.`;
          return { key, text: c.explanation };
        });
        return { type: 'Explained', content: out };
      }
      default:
        throw new Fail('unsupported', `${call.type} is not an AI call`);
    }
  }

  private answerable(request: string): Waiting {
    const w = this.waiting;
    if (!w || w.request !== request) throw new Fail('notFound', 'that request is no longer waiting');
    return w;
  }

  private userEntry(entry: number): number {
    if (this.entries[entry]?.type !== 'User') throw new Fail('invalid', `entry ${entry} is not a prompt`);
    return entry;
  }

  private findChange(key: string): ReviewChange {
    for (const r of this.review.repos) for (const c of r.changes) if (c.key === key) return c;
    throw new Fail('notFound', `no change ${key}`);
  }

  dispose(): void {
    if (this.turn) this.turn.cancelled = true;
    this.subs.clear();
  }
}

function makeReview(): Review {
  const commit = hex(4).slice(0, 7);
  return {
    repos: [
      {
        root: '/home/you/shed',
        commits: [{ hash: commit + hex(16), short: commit, subject: 'Bench can say how many tabs it has', author: 'You', time: now() - 1800 }],
        changes: [
          {
            key: 'k-lib',
            path: 'src/lib.rs',
            layer: 'uncommitted',
            status: 'M',
            viewed: false,
            explanation: null,
            diff: {
              hunks: [
                {
                  header: '@@ -14,6 +14,11 @@ impl Bench {',
                  lines: [
                    { kind: 'context', text: '        Bench { name: name.to_string(), tabs: Vec::new() }', old: 14, new: 14 },
                    { kind: 'context', text: '    }', old: 15, new: 15 },
                    { kind: 'added', text: '', old: null, new: 16 },
                    { kind: 'added', text: '    /// How many tabs it has.', old: null, new: 17 },
                    { kind: 'added', text: '    pub fn len(&self) -> usize {', old: null, new: 18 },
                    { kind: 'added', text: '        self.tabs.len()', old: null, new: 19 },
                    { kind: 'added', text: '    }', old: null, new: 20 },
                    { kind: 'context', text: '}', old: 16, new: 21 },
                  ],
                },
              ],
            },
          },
          {
            key: 'k-main',
            path: 'src/main.rs',
            layer: commit,
            status: 'M',
            viewed: false,
            explanation: null,
            diff: {
              hunks: [
                {
                  header: '@@ -24,7 +24,7 @@ fn parse_args',
                  lines: [
                    { kind: 'context', text: '        match arg.as_str() {', old: 24, new: 24 },
                    { kind: 'removed', text: '            "-b" => args.bench = it.next(),', old: 25, new: null },
                    { kind: 'added', text: '            "--bench" => args.bench = it.next(),', old: null, new: 25 },
                    { kind: 'context', text: '            other => args.dir = Some(PathBuf::from(other)),', old: 26, new: 26 },
                  ],
                },
              ],
            },
          },
          {
            key: 'k-doc',
            path: 'docs/new.md',
            layer: 'uncommitted',
            status: 'A',
            viewed: false,
            explanation: null,
            diff: {
              hunks: [
                {
                  header: '@@ -0,0 +1,3 @@',
                  lines: [
                    { kind: 'added', text: '# Notes', old: null, new: 1 },
                    { kind: 'added', text: '', old: null, new: 2 },
                    { kind: 'added', text: 'What changed this week.', old: null, new: 3 },
                  ],
                },
              ],
            },
          },
        ],
      },
    ],
  };
}
