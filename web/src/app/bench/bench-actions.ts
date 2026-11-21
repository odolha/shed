/**
 * What can be done to benches and tabs, shared by the overview and a bench's
 * page: each asks what it needs (a name, a folder, "are you sure"), calls the
 * workspace, and says if it failed.
 */

import { Injectable, inject } from '@angular/core';
import { Router } from '@angular/router';
import type { Bench, BenchRef, NewTabKind, Tab } from '../../api/gen/api';
import { WorkspacesStore } from '../../api/workspaces.store';
import { t } from '../../i18n/i18n';
import { Dialogs } from '../../ui/dialog';
import { Notices } from '../../ui/notices';
import { FolderPicker, type FolderPickerData } from './folder-picker';
import { closed, running } from '../shared/tabs';

/** The named bench colours and their bright stop (shed-model's `BenchColor`). */
export const BENCH_COLORS: { name: string; hex: string }[] = [
  { name: 'blue', hex: '#5CB2FF' },
  { name: 'violet', hex: '#B48BFF' },
  { name: 'pink', hex: '#FF8AC8' },
  { name: 'coral', hex: '#FF9A5A' },
  { name: 'amber', hex: '#FFD15C' },
  { name: 'green', hex: '#7BE38E' },
  { name: 'teal', hex: '#4BE0C4' },
  { name: 'slate', hex: '#B3BAC7' },
  { name: 'red', hex: '#FF6B7A' },
  { name: 'orange', hex: '#FFB066' },
  { name: 'lime', hex: '#C6EF5A' },
  { name: 'cyan', hex: '#5FE3FF' },
  { name: 'indigo', hex: '#8C9BFF' },
  { name: 'magenta', hex: '#F28CFF' },
  { name: 'brown', hex: '#D9A47A' },
  { name: 'olive', hex: '#D2CC6E' },
];

export type NewTabChoice = 'terminal' | 'project' | 'ai' | 'remote';

@Injectable({ providedIn: 'root' })
export class BenchActions {
  private readonly store = inject(WorkspacesStore);
  private readonly dialogs = inject(Dialogs);
  private readonly notices = inject(Notices);
  private readonly router = inject(Router);

  async newBench(ws: string): Promise<void> {
    const name = await this.dialogs.prompt({
      title: t('bench.new.title'),
      label: t('bench.new.name'),
      placeholder: t('bench.new.placeholder'),
      ok: t('bench.new.ok'),
      allowEmpty: true,
    });
    if (name === null) return;
    const id = await this.notices.run(
      this.store.call(ws, { type: 'NewBench', content: { name: name || null, cwd: null } }, 'Bench'),
    );
    if (id) void this.router.navigate(['/w', ws, 'b', id]);
  }

  async load(ws: string, name: string, start: boolean): Promise<void> {
    const id = await this.notices.run(this.store.call(ws, { type: 'LoadBench', content: { name, start } }, 'Bench'));
    if (id) void this.router.navigate(['/w', ws, 'b', id]);
  }

  async rename(ws: string, ref: BenchRef, current: string): Promise<void> {
    const name = await this.dialogs.prompt({ title: t('bench.rename'), value: current, ok: t('common.save') });
    if (!name || name === current) return;
    await this.notices.run(this.store.request(ws, { type: 'RenameBench', content: { bench: ref, name } }));
  }

  async color(ws: string, ref: BenchRef, color: string): Promise<void> {
    await this.notices.run(this.store.request(ws, { type: 'SetBenchColor', content: { bench: ref, color } }));
  }

  async badge(ws: string, ref: BenchRef, current: string | null | undefined): Promise<void> {
    const badge = await this.dialogs.prompt({
      title: t('bench.badge.title'),
      body: t('bench.badge.body'),
      value: current ?? '',
      ok: t('common.save'),
      allowEmpty: true,
    });
    if (badge === null) return;
    await this.notices.run(
      this.store.request(ws, { type: 'SetBadge', content: { bench: ref, badge: badge || null } }),
    );
  }

  async close(ws: string, bench: Bench): Promise<boolean> {
    const ok = await this.dialogs.confirm({
      title: t('bench.close.title', { name: bench.name }),
      body: t('bench.close.body'),
      ok: t('bench.close.ok'),
      danger: true,
    });
    if (!ok) return false;
    const r = await this.notices.run(this.store.request(ws, { type: 'CloseBench', content: { bench: bench.id } }));
    return r !== undefined;
  }

  async remove(ws: string, name: string): Promise<void> {
    const ok = await this.dialogs.confirm({
      title: t('bench.delete.title', { name }),
      body: t('bench.delete.body'),
      ok: t('bench.delete.ok'),
      danger: true,
    });
    if (ok) await this.notices.run(this.store.request(ws, { type: 'DeleteBench', content: { name } }));
  }

  async simple(ws: string, type: 'RunStartup' | 'RunTeardown' | 'SaveBench', bench: string, done: string): Promise<void> {
    const r = await this.notices.run(this.store.request(ws, { type, content: { bench } }));
    if (r) this.notices.info(done);
  }

  /** The folders a bench works in: its own, its project and AI tabs'. */
  folders(bench: { cwd: string; tabs: Tab[] }): string[] {
    const out = new Set<string>([bench.cwd]);
    for (const tab of bench.tabs) {
      if (tab.kind.type === 'Project') out.add(tab.kind.content.root);
      if (tab.kind.type === 'Ai') out.add(tab.kind.content.folder);
    }
    return [...out].filter(Boolean);
  }

  async pickFolder(ws: string, title: string, bench: { cwd: string; tabs: Tab[] }): Promise<string | null> {
    return this.dialogs.open<FolderPickerData, string>(FolderPicker, {
      workspace: ws,
      title,
      start: bench.cwd,
      suggestions: this.folders(bench),
    });
  }

  async newTab(ws: string, bench: Bench, choice: NewTabChoice): Promise<void> {
    let kind: NewTabKind;
    switch (choice) {
      case 'terminal':
        kind = { type: 'Terminal', content: { folder: null } };
        break;
      case 'project': {
        const folder = await this.pickFolder(ws, t('tab.new.projectFolder'), bench);
        if (!folder) return;
        kind = { type: 'Project', content: { folder } };
        break;
      }
      case 'ai': {
        const folder = await this.pickFolder(ws, t('tab.new.aiFolder'), bench);
        if (!folder) return;
        kind = { type: 'Ai', content: { folder } };
        break;
      }
      case 'remote': {
        const host = await this.dialogs.prompt({
          title: t('tab.new.remote'),
          label: t('tab.new.host'),
          placeholder: 'user@server',
          ok: t('tab.new.connect'),
        });
        if (!host) return;
        kind = { type: 'Remote', content: { host, join: null } };
        break;
      }
    }
    const id = await this.notices.run(
      this.store.call(ws, { type: 'NewTab', content: { bench: running(bench.id), kind } }, 'Tab'),
    );
    if (id) void this.router.navigate(['/w', ws, 'b', bench.id, 't', id]);
  }

  async renameTab(ws: string, ref: BenchRef, tab: Tab): Promise<void> {
    const name = await this.dialogs.prompt({
      title: t('tab.rename'),
      body: t('tab.rename.body'),
      value: tab.name ?? tab.title,
      ok: t('common.save'),
      allowEmpty: true,
    });
    if (name === null) return;
    await this.notices.run(
      this.store.request(ws, { type: 'RenameTab', content: { bench: ref, tab: tab.id, name: name || null } }),
    );
  }

  async setPrivate(ws: string, ref: BenchRef, tab: Tab, priv: boolean): Promise<void> {
    await this.notices.run(
      this.store.request(ws, { type: 'SetPrivate', content: { bench: ref, tab: tab.id, private: priv } }),
    );
  }

  /** A remote tab drops its connection and joins its shell again. */
  async reconnect(ws: string, tab: Tab): Promise<void> {
    const r = await this.notices.run(this.store.request(ws, { type: 'ReconnectRemote', content: { tab: tab.id } }));
    if (r) this.notices.info(t('tab.reconnected'));
  }

  async closeTab(ws: string, tab: Tab): Promise<boolean> {
    const ok = await this.dialogs.confirm({
      title: t('tab.close.title', { name: tab.title }),
      body: t('tab.close.body'),
      ok: t('tab.close.ok'),
      danger: true,
    });
    if (!ok) return false;
    return (await this.notices.run(this.store.request(ws, { type: 'CloseTab', content: { tab: tab.id } }))) !== undefined;
  }

  ref(bench: Bench | string): BenchRef {
    return typeof bench === 'string' ? closed(bench) : running(bench.id);
  }
}
