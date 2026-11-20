/**
 * Git in a project's (or AI tab's) folder: what changed, staged or not, the
 * diffs with hunk actions, committing, branches, history, fetch/pull/push.
 */

import {
  ChangeDetectionStrategy,
  Component,
  DestroyRef,
  computed,
  effect,
  inject,
  input,
  signal,
  untracked,
} from '@angular/core';
import { NgTemplateOutlet } from '@angular/common';
import { FormsModule } from '@angular/forms';
import type {
  Branches,
  Call,
  Commit,
  CommitFile,
  Diff,
  DiffSource,
  GitFile,
  GitStatus,
  HunkAction,
} from '../../api/gen/api';
import { WorkspacesStore } from '../../api/workspaces.store';
import { errorText } from '../../api/errors';
import { TPipe, t } from '../../i18n/i18n';
import { Dialogs } from '../../ui/dialog';
import { Icon } from '../../ui/icon';
import { Menu, MenuTrigger } from '../../ui/menu';
import { Notices } from '../../ui/notices';
import { AgoPipe } from '../../ui/time';
import { Tooltip } from '../../ui/tooltip';
import { DiffView, type HunkEvent } from '../shared/diff-view';
import { splitPath } from '../project/highlight';

type Side = 'staged' | 'unstaged';

interface Row {
  path: string;
  dir: string;
  name: string;
  letter: string;
  from: string | null;
}

interface Selected {
  path: string;
  side: Side;
}

/** The colour class for a status letter. */
export function letterClass(letter: string): string {
  switch (letter) {
    case 'M':
    case 'T':
      return 'l-warn';
    case 'A':
    case '?':
      return 'l-ok';
    case 'D':
    case 'U':
      return 'l-danger';
    case 'R':
    case 'C':
      return 'l-info';
    default:
      return '';
  }
}

function row(f: GitFile, letter: string): Row {
  const [dir, name] = splitPath(f.path);
  return { path: f.path, dir, name, letter: letter.trim() || '·', from: f.from ?? null };
}

function source(side: Side): DiffSource {
  return side === 'staged' ? { type: 'Staged' } : { type: 'Unstaged' };
}

@Component({
  selector: 'shed-git',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [NgTemplateOutlet, FormsModule, TPipe, Icon, Menu, MenuTrigger, Tooltip, DiffView, AgoPipe],
  templateUrl: './git-panel.html',
  styleUrl: './git-panel.css',
})
export class GitPanel {
  readonly workspace = input.required<string>();
  readonly tab = input.required<string>();
  /** Bumped by the host when the disk or Git changed (with `follow` off). */
  readonly refresh = input(0);
  /** Follow changes on disk by itself (a `Project` subscription). */
  readonly follow = input(true);

  private readonly store = inject(WorkspacesStore);
  private readonly dialogs = inject(Dialogs);
  private readonly notices = inject(Notices);

  protected readonly status = signal<GitStatus | null>(null);
  protected readonly statusError = signal<string | null>(null);
  protected readonly mode = signal<'changes' | 'history'>('changes');
  protected readonly selected = signal<Selected | null>(null);
  protected readonly diff = signal<Diff | null>(null);
  protected readonly diffError = signal<string | null>(null);
  protected readonly busy = signal<string | null>(null);
  protected readonly output = signal<{ title: string; text: string } | null>(null);
  protected readonly outputOpen = signal(true);
  protected readonly branches = signal<Branches | null>(null);
  protected message = '';
  protected amend = false;

  protected readonly commits = signal<Commit[] | null>(null);
  protected readonly commitsError = signal<string | null>(null);
  protected readonly commit = signal<Commit | null>(null);
  protected readonly commitFiles = signal<CommitFile[] | null>(null);
  protected readonly commitFile = signal<string | null>(null);

  protected readonly narrow = signal(false);

  protected readonly canChange = computed(() => this.store.canChange(this.workspace()));

  protected readonly conflicts = computed(() =>
    (this.status()?.files ?? []).filter((f) => f.conflict).map((f) => row(f, 'U')),
  );
  protected readonly staged = computed(() =>
    (this.status()?.files ?? [])
      .filter((f) => !f.conflict && f.staged.trim() !== '' && f.staged !== '?')
      .map((f) => row(f, f.staged)),
  );
  protected readonly unstaged = computed(() =>
    (this.status()?.files ?? [])
      .filter((f) => !f.conflict && (f.unstaged.trim() !== '' || f.staged === '?'))
      .map((f) => row(f, f.staged === '?' ? '?' : f.unstaged)),
  );

  /** In a narrow layout, the detail (diff) is shown instead of the lists. */
  protected readonly showDetail = computed(() =>
    this.mode() === 'changes' ? !!this.selected() : !!this.commit(),
  );

  protected readonly canCommit = computed(() => this.staged().length > 0);

  private seq = 0;
  private diffSeq = 0;
  private followTimer: ReturnType<typeof setTimeout> | null = null;

  constructor() {
    const mq = matchMedia('(max-width: 760px)');
    this.narrow.set(mq.matches);
    const onMq = (e: MediaQueryListEvent) => this.narrow.set(e.matches);
    mq.addEventListener('change', onMq);
    inject(DestroyRef).onDestroy(() => {
      mq.removeEventListener('change', onMq);
      if (this.followTimer) clearTimeout(this.followTimer);
    });

    effect(() => {
      this.workspace();
      this.tab();
      untracked(() => {
        this.status.set(null);
        this.selected.set(null);
        this.diff.set(null);
        this.commits.set(null);
        this.commit.set(null);
        void this.load();
      });
    });

    effect(() => {
      if (this.refresh() === 0) return;
      untracked(() => this.reloadSoon());
    });

    effect((onCleanup) => {
      if (!this.follow()) return;
      const ws = this.workspace();
      const tab = this.tab();
      const sub = this.store.socket.subscribe(
        ws,
        { type: 'Project', content: { tab } },
        { event: (e) => (e.type === 'GitChanged' || e.type === 'FilesChanged' ? this.reloadSoon() : undefined) },
      );
      onCleanup(() => sub.close());
    });
  }

  // ------------------------------------------------------------ loading

  protected async load(): Promise<void> {
    const seq = ++this.seq;
    const ws = this.workspace();
    const tab = this.tab();
    try {
      const st = await this.store.call(ws, { type: 'GitStatus', content: { tab } }, 'GitStatus');
      if (seq !== this.seq) return;
      this.status.set(st);
      this.statusError.set(null);
    } catch (e) {
      if (seq === this.seq) this.statusError.set(errorText(e));
      return;
    }
    if (this.mode() === 'changes' && this.selected()) void this.loadDiff();
    if (this.mode() === 'history' && this.commits()) void this.loadLog();
  }

  private reloadSoon(): void {
    if (this.followTimer) clearTimeout(this.followTimer);
    this.followTimer = setTimeout(() => {
      this.followTimer = null;
      void this.load();
    }, 300);
  }

  private async loadDiff(): Promise<void> {
    const seq = ++this.diffSeq;
    const ws = this.workspace();
    const tab = this.tab();
    let src: DiffSource;
    let path: string;
    if (this.mode() === 'changes') {
      const sel = this.selected();
      if (!sel) return;
      src = source(sel.side);
      path = sel.path;
    } else {
      const c = this.commit();
      const f = this.commitFile();
      if (!c || !f) return;
      src = { type: 'Commit', content: c.hash };
      path = f;
    }
    try {
      const d = await this.store.call(ws, { type: 'GitDiff', content: { tab, path, source: src } }, 'Diff');
      if (seq !== this.diffSeq) return;
      this.diff.set(d);
      this.diffError.set(null);
    } catch (e) {
      if (seq === this.diffSeq) this.diffError.set(errorText(e));
    }
  }

  // ------------------------------------------------------------ changes

  protected select(path: string, side: Side): void {
    this.selected.set({ path, side });
    this.diff.set(null);
    this.diffError.set(null);
    void this.loadDiff();
  }

  protected closeDetail(): void {
    if (this.mode() === 'changes') this.selected.set(null);
    else if (this.commitFile()) this.commitFile.set(null);
    else this.commit.set(null);
    this.diff.set(null);
  }

  protected isSelected(path: string, side: Side): boolean {
    const s = this.selected();
    return !!s && s.path === path && s.side === side;
  }

  protected stage(paths: string[]): Promise<void> {
    return this.op({ type: 'GitStage', content: { tab: this.tab(), paths } });
  }

  protected unstage(paths: string[]): Promise<void> {
    return this.op({ type: 'GitUnstage', content: { tab: this.tab(), paths } });
  }

  protected async discard(paths: string[]): Promise<void> {
    const untracked = new Set(this.unstaged().filter((r) => r.letter === '?').map((r) => r.path));
    const deletes = paths.some((p) => untracked.has(p));
    const ok = await this.dialogs.confirm({
      title: t('git.discard.title', { n: paths.length }),
      body: paths.join('\n') + (deletes ? '\n\n' + t('git.discard.untracked') : ''),
      ok: t('git.discard.ok'),
      danger: true,
    });
    if (!ok) return;
    await this.op({ type: 'GitDiscard', content: { tab: this.tab(), paths } });
  }

  protected async onHunk(e: HunkEvent): Promise<void> {
    const sel = this.selected();
    if (!sel) return;
    if (e.action === 'discard') {
      const ok = await this.dialogs.confirm({
        title: t('git.discardHunk.title'),
        body: `${sel.path}\n${e.header}`,
        ok: t('git.discard.ok'),
        danger: true,
      });
      if (!ok) return;
    }
    await this.op({
      type: 'GitHunk',
      content: { tab: this.tab(), path: sel.path, source: source(sel.side), hunk: e.index, header: e.header, action: e.action },
    });
  }

  protected hunkActions(): HunkAction[] {
    if (!this.canChange() || this.mode() !== 'changes') return [];
    const sel = this.selected();
    if (!sel) return [];
    return sel.side === 'staged' ? ['unstage'] : ['stage', 'discard'];
  }

  protected async doCommit(): Promise<void> {
    const message = this.message.trim();
    if (!message || (!this.canCommit() && !this.amend)) return;
    this.busy.set('commit');
    try {
      const hash = await this.store.call(
        this.workspace(),
        { type: 'GitCommit', content: { tab: this.tab(), message, amend: this.amend } },
        'Text',
      );
      this.notices.info(t('git.committed', { hash: hash.slice(0, 8) }));
      this.message = '';
      this.amend = false;
    } catch (e) {
      this.notices.error(e);
    } finally {
      this.busy.set(null);
    }
    await this.load();
  }

  // ------------------------------------------------------------ remote

  protected async remote(kind: 'fetch' | 'pull' | 'push'): Promise<void> {
    const tab = this.tab();
    if (kind === 'push') {
      const st = this.status();
      const out = st?.outgoing ?? [];
      const body = [
        st?.upstream ? t('git.push.to', { upstream: st.upstream }) : t('git.push.noUpstream'),
        '',
        ...(out.length ? out.map((c) => `${c.short}  ${c.subject}`) : [t('git.push.nothing')]),
      ].join('\n');
      const ok = await this.dialogs.confirm({ title: t('git.push.title', { n: out.length }), body, ok: t('git.push') });
      if (!ok) return;
    }
    const call: Call =
      kind === 'fetch'
        ? { type: 'GitFetch', content: { tab } }
        : kind === 'pull'
          ? { type: 'GitPull', content: { tab } }
          : { type: 'GitPush', content: { tab, force: false } };
    this.busy.set(kind);
    try {
      const text = await this.store.call(this.workspace(), call, 'Text', 120_000);
      this.output.set({ title: t(`git.${kind}.done`), text: text.trim() || t('git.noOutput') });
      this.outputOpen.set(true);
    } catch (e) {
      this.output.set({ title: t(`git.${kind}.failed`), text: errorText(e) });
      this.outputOpen.set(true);
    } finally {
      this.busy.set(null);
    }
    await this.load();
  }

  // ------------------------------------------------------------ branches

  protected async loadBranches(): Promise<void> {
    try {
      this.branches.set(
        await this.store.call(this.workspace(), { type: 'GitBranches', content: { tab: this.tab() } }, 'Branches'),
      );
    } catch (e) {
      this.notices.error(e);
    }
  }

  protected async switchTo(branch: string): Promise<void> {
    await this.op({ type: 'GitSwitch', content: { tab: this.tab(), branch } }, t('git.switched', { branch }));
  }

  protected async newBranch(): Promise<void> {
    const branch = await this.dialogs.prompt({
      title: t('git.newBranch.title'),
      label: t('git.newBranch.label'),
      body: this.status()?.branch ? t('git.newBranch.from', { branch: this.status()!.branch! }) : undefined,
      ok: t('git.newBranch.ok'),
    });
    if (!branch) return;
    await this.op({ type: 'GitCreateBranch', content: { tab: this.tab(), branch } }, t('git.switched', { branch }));
  }

  // ------------------------------------------------------------ history

  protected setMode(mode: 'changes' | 'history'): void {
    this.mode.set(mode);
    this.diff.set(null);
    if (mode === 'history' && !this.commits()) void this.loadLog();
    if (mode === 'changes' && this.selected()) void this.loadDiff();
    if (mode === 'history' && this.commitFile()) void this.loadDiff();
  }

  private async loadLog(): Promise<void> {
    try {
      this.commits.set(
        await this.store.call(this.workspace(), { type: 'GitLog', content: { tab: this.tab(), limit: 100 } }, 'Commits'),
      );
      this.commitsError.set(null);
    } catch (e) {
      this.commitsError.set(errorText(e));
    }
  }

  protected async openCommit(c: Commit): Promise<void> {
    this.commit.set(c);
    this.commitFile.set(null);
    this.commitFiles.set(null);
    this.diff.set(null);
    try {
      const files = await this.store.call(
        this.workspace(),
        { type: 'GitCommitFiles', content: { tab: this.tab(), hash: c.hash } },
        'CommitFiles',
      );
      if (this.commit()?.hash === c.hash) this.commitFiles.set(files);
    } catch (e) {
      this.notices.error(e);
    }
  }

  protected openCommitFile(path: string): void {
    this.commitFile.set(path);
    this.diff.set(null);
    this.diffError.set(null);
    void this.loadDiff();
  }

  // ------------------------------------------------------------ helpers

  protected letterClass(l: string): string {
    return letterClass(l);
  }

  protected paths(rows: Row[]): string[] {
    return rows.map((r) => r.path);
  }

  protected splitPath(p: string): [string, string] {
    return splitPath(p);
  }

  private async op(call: Call, done?: string): Promise<void> {
    this.busy.set(call.type);
    try {
      await this.store.request(this.workspace(), call, 60_000);
      if (done) this.notices.info(done);
    } catch (e) {
      this.notices.error(e);
    } finally {
      this.busy.set(null);
    }
    await this.load();
  }
}
