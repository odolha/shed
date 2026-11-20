/**
 * A project tab: its files (a tree and a viewer), search in files, and Git.
 * On a narrow screen one part shows at a time, with a way back.
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
  viewChild,
} from '@angular/core';
import type { Bench, Tab } from '../../api/gen/api';
import { WorkspacesStore } from '../../api/workspaces.store';
import { TPipe } from '../../i18n/i18n';
import { Dialogs } from '../../ui/dialog';
import { Icon } from '../../ui/icon';
import { Tooltip } from '../../ui/tooltip';
import { GitPanel } from '../git/git-panel';
import { tildify } from '../shared/tabs';
import { type DiskChange, FileTree } from './file-tree';
import { FileView } from './file-view';
import { splitPath } from './highlight';
import { QuickOpen, type QuickOpenData } from './quick-open';
import { type OpenAt, ProjectSearch } from './search';

type Mode = 'files' | 'search' | 'changes';

/** What each project tab showed, while the app runs. */
const memory = new Map<string, { mode: Mode; file: OpenAt | null }>();

@Component({
  selector: 'shed-project-tab',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [TPipe, Icon, Tooltip, FileTree, FileView, ProjectSearch, GitPanel],
  host: { '(document:keydown)': 'onKey($event)' },
  template: `
    <div class="head">
      <shed-icon name="project" [size]="16" class="kind" />
      <span class="root mono ellipsis" [title]="root()">{{ rootShown() }}</span>
      <span class="grow"></span>
      <div class="seg" role="tablist" [attr.aria-label]="'project.views' | t">
        <button role="tab" class="btn small" [class.on]="mode() === 'files'" [attr.aria-selected]="mode() === 'files'" (click)="setMode('files')">
          <shed-icon name="folder" [size]="14" /><span class="label">{{ 'project.files' | t }}</span>
        </button>
        <button role="tab" class="btn small" [class.on]="mode() === 'search'" [attr.aria-selected]="mode() === 'search'" (click)="setMode('search')">
          <shed-icon name="search" [size]="14" /><span class="label">{{ 'project.search' | t }}</span>
        </button>
        <button role="tab" class="btn small" [class.on]="mode() === 'changes'" [attr.aria-selected]="mode() === 'changes'" (click)="setMode('changes')">
          <shed-icon name="git" [size]="14" /><span class="label">{{ 'project.changes' | t }}</span>
          @if (changedCount()) {
            <span class="chip accent count">{{ changedCount() }}</span>
          }
        </button>
      </div>
      <button class="btn small ghost" (click)="quickOpen()" [shedTip]="'project.quickOpen.tip' | t">
        <shed-icon name="go" [size]="14" /><span class="label">{{ 'project.quickOpen' | t }}</span>
      </button>
    </div>

    @if (mode() === 'changes') {
      <shed-git [workspace]="workspace()" [tab]="tab().id" [refresh]="gitTick()" [follow]="false" />
    } @else {
      <div class="body" [class.narrow]="narrow()" [class.main-shown]="!!file()">
        <div class="side">
          <shed-file-tree
            [class.off]="mode() !== 'files'"
            [workspace]="workspace()"
            [tab]="tab().id"
            [selected]="file()?.path ?? null"
            [changes]="disk()"
            (open)="openFile({ path: $event, line: null })"
          />
          <shed-project-search
            [class.off]="mode() !== 'search'"
            [workspace]="workspace()"
            [tab]="tab().id"
            (open)="openFile($event)"
          />
        </div>
        <div class="main">
          @if (file(); as f) {
            <shed-file-view
              [workspace]="workspace()"
              [tab]="tab().id"
              [path]="f.path"
              [line]="f.line"
              [changed]="fileTick()"
              [showBack]="narrow()"
              (back)="file.set(null)"
            />
          } @else {
            <div class="placeholder">
              <p class="dim">{{ 'project.pick' | t }}</p>
              @if (openInShed().length) {
                <div class="menu-label">{{ 'project.openInShed' | t }}</div>
                @for (p of openInShed(); track p) {
                  <button class="menu-item" (click)="openFile({ path: p, line: null })">
                    <shed-icon name="file" [size]="14" />
                    <span class="mono ellipsis"><span>{{ split(p)[1] }}</span>&ngsp;<span class="dim small">{{ split(p)[0] }}</span></span>
                  </button>
                }
              }
            </div>
          }
        </div>
      </div>
    }
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      flex: 1 1 auto;
      min-height: 0;
    }
    .head {
      display: flex;
      align-items: center;
      gap: 6px;
      min-height: 40px;
      padding: 2px 8px 2px 12px;
      border-bottom: 1px solid var(--divider);
    }
    .kind {
      color: var(--text-dim);
    }
    .root {
      font-size: 12.5px;
      color: var(--text-dim);
      min-width: 3em;
    }
    .seg {
      display: inline-flex;
      gap: 2px;
    }
    .count {
      height: 18px;
      padding: 0 6px;
    }
    .body {
      display: grid;
      grid-template-columns: minmax(220px, 300px) 1fr;
      flex: 1 1 auto;
      min-height: 0;
    }
    .side,
    .main {
      display: flex;
      flex-direction: column;
      min-height: 0;
      min-width: 0;
    }
    .side {
      border-right: 1px solid var(--divider);
    }
    .side > * {
      flex: 1 1 auto;
    }
    .main > * {
      flex: 1 1 auto;
    }
    .off {
      display: none;
    }
    .body.narrow {
      grid-template-columns: 1fr;
    }
    .body.narrow .side {
      border-right: 0;
    }
    .body.narrow.main-shown .side,
    .body.narrow:not(.main-shown) .main {
      display: none;
    }
    .placeholder {
      padding: 24px 16px;
      max-width: 520px;
    }
    @media (max-width: 640px) {
      .head .label {
        display: none;
      }
      .root {
        flex: 1 1 auto;
      }
    }
  `,
})
export class ProjectTab {
  readonly workspace = input.required<string>();
  readonly bench = input.required<Bench>();
  readonly tab = input.required<Tab>();

  private readonly store = inject(WorkspacesStore);
  private readonly dialogs = inject(Dialogs);
  private readonly fileView = viewChild(FileView);

  protected readonly mode = signal<Mode>('files');
  protected readonly file = signal<OpenAt | null>(null);
  protected readonly disk = signal<DiskChange | null>(null);
  protected readonly fileTick = signal(0);
  protected readonly gitTick = signal(0);
  protected readonly changedCount = signal(0);
  protected readonly narrow = signal(false);

  protected readonly project = computed(() => {
    const k = this.tab().kind;
    return k.type === 'Project' ? k.content : null;
  });
  protected readonly root = computed(() => this.project()?.root ?? '');
  protected readonly rootShown = computed(() => tildify(this.root(), this.store.state(this.workspace())?.home ?? ''));
  protected readonly openInShed = computed(() => this.project()?.open ?? []);

  private diskSeq = 0;
  private countTimer: ReturnType<typeof setTimeout> | null = null;

  constructor() {
    const mq = matchMedia('(max-width: 760px)');
    this.narrow.set(mq.matches);
    const onMq = (e: MediaQueryListEvent) => this.narrow.set(e.matches);
    mq.addEventListener('change', onMq);
    inject(DestroyRef).onDestroy(() => {
      mq.removeEventListener('change', onMq);
      if (this.countTimer) clearTimeout(this.countTimer);
    });

    // A tab: what it showed last, and its changes on disk.
    effect((onCleanup) => {
      const ws = this.workspace();
      const tab = this.tab().id;
      untracked(() => {
        const mem = memory.get(tab);
        this.mode.set(mem?.mode ?? 'files');
        this.file.set(mem?.file ?? null);
        this.changedCount.set(0);
        void this.countChanges();
      });
      const sub = this.store.socket.subscribe(
        ws,
        { type: 'Project', content: { tab } },
        {
          event: (e) => {
            if (e.type === 'FilesChanged') {
              const paths = e.content.paths;
              this.disk.set({ seq: ++this.diskSeq, paths });
              const open = this.file()?.path;
              if (open && (!paths.length || paths.includes(open))) this.fileTick.update((n) => n + 1);
              this.gitTick.update((n) => n + 1);
              this.countSoon();
            } else if (e.type === 'GitChanged') {
              this.gitTick.update((n) => n + 1);
              this.countSoon();
            }
          },
        },
      );
      onCleanup(() => sub.close());
    });

    effect(() => {
      memory.set(this.tab().id, { mode: this.mode(), file: this.file() });
    });
  }

  protected split(p: string): [string, string] {
    return splitPath(p);
  }

  protected setMode(mode: Mode): void {
    this.mode.set(mode);
  }

  protected async openFile(at: OpenAt): Promise<void> {
    const view = this.fileView();
    if (view && this.file()?.path !== at.path && !(await view.mayLeave())) return;
    this.file.set({ ...at });
  }

  protected async quickOpen(): Promise<void> {
    const data: QuickOpenData = { workspace: this.workspace(), tab: this.tab().id, recent: this.openInShed() };
    const path = await this.dialogs.open<QuickOpenData, string>(QuickOpen, data);
    if (!path) return;
    if (this.mode() === 'changes') this.mode.set('files');
    await this.openFile({ path, line: null });
  }

  protected onKey(e: KeyboardEvent): void {
    if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === 'p') {
      if (this.dialogs.stack().length) return;
      e.preventDefault();
      void this.quickOpen();
    }
  }

  private countSoon(): void {
    if (this.countTimer) clearTimeout(this.countTimer);
    this.countTimer = setTimeout(() => {
      this.countTimer = null;
      void this.countChanges();
    }, 500);
  }

  private async countChanges(): Promise<void> {
    const ws = this.workspace();
    const tab = this.tab().id;
    try {
      const st = await this.store.call(ws, { type: 'GitStatus', content: { tab } }, 'GitStatus');
      if (ws === this.workspace() && tab === this.tab().id) this.changedCount.set(st.repo ? st.files.length : 0);
    } catch {
      // No badge then.
    }
  }
}
