/**
 * A project's files as a tree, read folder by folder (`ListFolder`) as they
 * are opened, and read again when the disk changes under them.
 */

import {
  ChangeDetectionStrategy,
  Component,
  computed,
  effect,
  inject,
  input,
  output,
  signal,
  untracked,
  viewChild,
} from '@angular/core';
import type { FolderEntry } from '../../api/gen/api';
import { WorkspacesStore } from '../../api/workspaces.store';
import { errorText } from '../../api/errors';
import { TPipe, t } from '../../i18n/i18n';
import { Dialogs } from '../../ui/dialog';
import { Icon } from '../../ui/icon';
import { Menu, MenuTrigger } from '../../ui/menu';
import { Notices } from '../../ui/notices';
import { Tooltip } from '../../ui/tooltip';
import { parentOf } from './highlight';

type FolderState = { entries: FolderEntry[] } | { loading: true } | { error: string };

interface Row {
  path: string;
  name: string;
  dir: boolean;
  ignored: boolean;
  depth: number;
  open: boolean;
  loading: boolean;
  error: string | null;
}

/** Changes on disk, as the project tab passes them on. */
export interface DiskChange {
  seq: number;
  paths: string[];
}

/** Which folders were open, per tab, while the app runs. */
const expandedByTab = new Map<string, Set<string>>();

function join(folder: string, name: string): string {
  return folder ? `${folder}/${name}` : name;
}

@Component({
  selector: 'shed-file-tree',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [Icon, TPipe, Menu, MenuTrigger, Tooltip],
  template: `
    <div class="head">
      <span class="grow dim small ellipsis">{{ 'project.files' | t }}</span>
      <button class="icon-btn small" (click)="refreshAll()" [shedTip]="'common.refresh' | t">
        <shed-icon name="refresh" [size]="14" />
      </button>
      @if (canChange()) {
        <button class="icon-btn small" [shedMenu]="rootMenu" [shedTip]="'project.newEntry' | t">
          <shed-icon name="plus" [size]="14" />
        </button>
        <shed-menu #rootMenu>
          <button class="menu-item" (click)="create('', false)">
            <shed-icon name="file" />{{ 'project.newFile' | t }}
          </button>
          <button class="menu-item" (click)="create('', true)">
            <shed-icon name="folder" />{{ 'project.newFolder' | t }}
          </button>
        </shed-menu>
      }
    </div>
    <div class="rows" role="tree" [attr.aria-label]="'project.files' | t">
      @if (rootError(); as e) {
        <div class="error-box small">{{ e }}</div>
      }
      @for (r of rows(); track r.path) {
        <button
          class="row-btn"
          role="treeitem"
          [attr.aria-expanded]="r.dir ? r.open : null"
          [attr.aria-selected]="r.path === selected()"
          [class.selected]="r.path === selected()"
          [class.ignored]="r.ignored"
          [style.padding-left.px]="8 + r.depth * 14"
          (click)="activate(r)"
          (contextmenu)="onContext($event, r)"
          [title]="r.path"
        >
          @if (r.dir) {
            <shed-icon class="chev" [name]="r.open ? 'chevron-down' : 'chevron-right'" [size]="12" />
            <shed-icon class="kind" name="folder" [size]="15" />
          } @else {
            <span class="chev"></span>
            <shed-icon class="kind" name="file" [size]="15" />
          }
          <span class="ellipsis">{{ r.name }}</span>
          @if (r.loading) {
            <span class="dim small">…</span>
          }
        </button>
        @if (r.error) {
          <div class="error-box small" [style.margin-left.px]="22 + r.depth * 14">{{ r.error }}</div>
        }
      } @empty {
        @if (!rootError()) {
          <div class="empty small">{{ loadingRoot() ? ('common.loading' | t) : ('project.emptyFolder' | t) }}</div>
        }
      }
    </div>
    <shed-menu #rowMenu>
      @if (menuFolder() !== null) {
        <button class="menu-item" (click)="create(menuFolder()!, false)">
          <shed-icon name="file" />{{ 'project.newFileHere' | t }}
        </button>
        <button class="menu-item" (click)="create(menuFolder()!, true)">
          <shed-icon name="folder" />{{ 'project.newFolderHere' | t }}
        </button>
        <div class="menu-sep"></div>
      }
      <button class="menu-item" (click)="copyPath()"><shed-icon name="copy" />{{ 'project.copyPath' | t }}</button>
    </shed-menu>
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      min-height: 0;
    }
    .head {
      display: flex;
      align-items: center;
      gap: 2px;
      padding: 4px 4px 4px 10px;
      border-bottom: 1px solid var(--divider);
    }
    .rows {
      flex: 1 1 auto;
      overflow: auto;
      padding: 4px 0;
    }
    .row-btn {
      display: flex;
      align-items: center;
      gap: 4px;
      width: 100%;
      min-height: 26px;
      padding-right: 8px;
      border: 0;
      background: transparent;
      color: var(--text);
      font: inherit;
      font-size: 13px;
      text-align: left;
      cursor: pointer;
    }
    .row-btn:hover {
      background: var(--hover);
    }
    .row-btn.selected {
      background: color-mix(in srgb, var(--accent) 16%, transparent);
    }
    .row-btn.ignored {
      color: var(--text-dim);
    }
    .chev {
      width: 12px;
      flex: none;
      color: var(--text-dim);
    }
    .kind {
      color: var(--text-dim);
    }
    @media (pointer: coarse) {
      .row-btn {
        min-height: 44px;
      }
    }
  `,
})
export class FileTree {
  readonly workspace = input.required<string>();
  readonly tab = input.required<string>();
  readonly selected = input<string | null>(null);
  readonly changes = input<DiskChange | null>(null);
  readonly open = output<string>();

  private readonly store = inject(WorkspacesStore);
  private readonly dialogs = inject(Dialogs);
  private readonly notices = inject(Notices);
  private readonly rowMenu = viewChild.required<Menu>('rowMenu');

  private readonly folders = signal<Record<string, FolderState>>({});
  private readonly expanded = signal<Set<string>>(new Set());
  protected readonly menuFolder = signal<string | null>(null);
  private menuPath = '';

  protected readonly canChange = computed(() => this.store.canChange(this.workspace()));

  protected readonly loadingRoot = computed(() => {
    const root = this.folders()[''];
    return !root || 'loading' in root;
  });

  protected readonly rootError = computed(() => {
    const root = this.folders()[''];
    return root && 'error' in root ? root.error : null;
  });

  protected readonly rows = computed<Row[]>(() => {
    const folders = this.folders();
    const expanded = this.expanded();
    const out: Row[] = [];
    const walk = (folder: string, depth: number) => {
      const st = folders[folder];
      if (!st || !('entries' in st)) return;
      for (const e of st.entries) {
        const path = join(folder, e.name);
        const open = e.dir && expanded.has(path);
        const sub = folders[path];
        out.push({
          path,
          name: e.name,
          dir: e.dir,
          ignored: !!e.ignored,
          depth,
          open,
          loading: open && (!sub || 'loading' in sub),
          error: open && sub && 'error' in sub ? sub.error : null,
        });
        if (open) walk(path, depth + 1);
      }
    };
    walk('', 0);
    return out;
  });

  constructor() {
    // A new tab: start from its remembered open folders.
    effect(() => {
      const ws = this.workspace();
      const tab = this.tab();
      untracked(() => {
        let mem = expandedByTab.get(tab);
        if (!mem) {
          mem = new Set();
          expandedByTab.set(tab, mem);
        }
        this.folders.set({});
        this.expanded.set(new Set(mem));
        void this.load(ws, tab, '');
        for (const f of mem) void this.load(ws, tab, f);
      });
    });

    // The disk changed: read again the folders that hold what changed.
    effect(() => {
      const ch = this.changes();
      if (!ch) return;
      untracked(() => {
        const loaded = this.folders();
        const again = new Set<string>();
        for (const p of ch.paths) {
          const clean = p.endsWith('/') ? p.slice(0, -1) : p;
          const parent = parentOf(clean);
          if (parent in loaded) again.add(parent);
          if (clean in loaded) again.add(clean);
        }
        if (!ch.paths.length) for (const f of Object.keys(loaded)) again.add(f);
        for (const f of again) void this.load(this.workspace(), this.tab(), f, true);
      });
    });
  }

  protected refreshAll(): void {
    for (const f of Object.keys(this.folders())) void this.load(this.workspace(), this.tab(), f, true);
  }

  protected activate(r: Row): void {
    if (!r.dir) {
      this.open.emit(r.path);
      return;
    }
    const next = new Set(this.expanded());
    if (next.has(r.path)) next.delete(r.path);
    else {
      next.add(r.path);
      if (!(r.path in this.folders())) void this.load(this.workspace(), this.tab(), r.path);
    }
    this.expanded.set(next);
    expandedByTab.set(this.tab(), new Set(next));
  }

  protected onContext(e: MouseEvent, r: Row): void {
    e.preventDefault();
    this.menuPath = r.path;
    this.menuFolder.set(this.canChange() ? (r.dir ? r.path : parentOf(r.path)) : null);
    this.rowMenu().openAt(e.clientX, e.clientY, e.currentTarget as HTMLElement);
  }

  protected async copyPath(): Promise<void> {
    try {
      await navigator.clipboard.writeText(this.menuPath);
      this.notices.info(t('common.copied'));
    } catch (e) {
      this.notices.error(e);
    }
  }

  protected async create(folder: string, dir: boolean): Promise<void> {
    const name = await this.dialogs.prompt({
      title: t(dir ? 'project.newFolder.title' : 'project.newFile.title'),
      body: folder ? t('project.in', { folder }) : undefined,
      label: t('project.name'),
      ok: t('project.create'),
    });
    if (!name) return;
    const path = join(folder, name.replace(/^\/+|\/+$/g, ''));
    const ws = this.workspace();
    const tab = this.tab();
    const done = await this.notices.run(
      this.store.call(ws, { type: 'CreateEntry', content: { tab, path: dir ? `${path}/` : path } }, 'Done'),
    );
    if (done === undefined) return;
    if (folder) {
      const next = new Set(this.expanded());
      next.add(folder);
      this.expanded.set(next);
      expandedByTab.set(tab, new Set(next));
    }
    await this.load(ws, tab, folder, true);
    if (!dir) this.open.emit(path);
  }

  private async load(ws: string, tab: string, folder: string, quiet = false): Promise<void> {
    if (!quiet || !(folder in this.folders())) {
      this.folders.update((f) => ({ ...f, [folder]: { loading: true } }));
    }
    let st: FolderState;
    try {
      const entries = await this.store.call(ws, { type: 'ListFolder', content: { tab, path: folder } }, 'Folder');
      entries.sort((a, b) => (a.dir === b.dir ? a.name.localeCompare(b.name) : a.dir ? -1 : 1));
      st = { entries };
    } catch (e) {
      st = { error: errorText(e) };
    }
    if (ws !== this.workspace() || tab !== this.tab()) return;
    this.folders.update((f) => ({ ...f, [folder]: st }));
  }
}
