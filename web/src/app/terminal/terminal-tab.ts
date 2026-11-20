/**
 * A terminal (or remote) tab: its panes laid out as split on wide screens,
 * one at a time on narrow ones, with the pane actions and the logbook.
 */

import {
  ChangeDetectionStrategy,
  Component,
  DestroyRef,
  computed,
  inject,
  input,
  signal,
  viewChildren,
} from '@angular/core';
import type { Bench, InputItem, Layout, Pane, SplitDir, Tab } from '../../api/gen/api';
import { WorkspacesStore } from '../../api/workspaces.store';
import { TPipe, t } from '../../i18n/i18n';
import { Dialogs } from '../../ui/dialog';
import { Icon } from '../../ui/icon';
import { Notices } from '../../ui/notices';
import { Tooltip } from '../../ui/tooltip';
import { LogbookDialog, type LogbookDialogData } from '../logbook/logbook-dialog';
import { tildify } from '../shared/tabs';
import { TerminalView } from './terminal-view';

/** A pane's place in the tab, in fractions of its size. */
export interface Placed {
  id: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Where each pane of a layout goes. */
export function place(layout: Layout, x = 0, y = 0, w = 1, h = 1): Placed[] {
  if (layout.type === 'Pane') return [{ id: layout.content, x, y, w, h }];
  const { dir, ratio, first, second } = layout.content;
  const r = Math.min(0.95, Math.max(0.05, ratio || 0.5));
  if (dir === 'right') {
    return [...place(first, x, y, w * r, h), ...place(second, x + w * r, y, w * (1 - r), h)];
  }
  return [...place(first, x, y, w, h * r), ...place(second, x, y + h * r, w, h * (1 - r))];
}

const ENTER: InputItem = { type: 'Key', content: { key: 'Enter', ctrl: false, alt: false, shift: false } };

@Component({
  selector: 'shed-terminal-tab',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [TerminalView, Icon, TPipe, Tooltip],
  template: `
    <div class="bar">
      @if (narrow() && panes().length > 1) {
        <div class="switcher" role="tablist" [attr.aria-label]="'term.panes' | t">
          @for (p of panes(); track p.id; let i = $index) {
            <button
              class="btn small"
              role="tab"
              [class.on]="p.id === activeId()"
              [attr.aria-selected]="p.id === activeId()"
              (click)="focusedId.set(p.id)"
            >
              {{ i + 1 }} · <span class="ellipsis label">{{ paneLabel(p) }}</span>
            </button>
          }
        </div>
      }
      <span class="grow"></span>
      <button class="btn small ghost" (click)="openLogbook()">
        <shed-icon name="logbook" [size]="15" /><span class="wide-only">{{ 'term.logbook' | t }}</span>
      </button>
      @if (canChange() && active(); as a) {
        <button class="icon-btn small" [shedTip]="'term.splitRight' | t" (click)="split(a, 'right')">
          <shed-icon name="split-right" [size]="15" />
        </button>
        <button class="icon-btn small" [shedTip]="'term.splitDown' | t" (click)="split(a, 'down')">
          <shed-icon name="split-down" [size]="15" />
        </button>
        <button class="icon-btn small close" [shedTip]="'term.closePane' | t" (click)="closePane(a)">
          <shed-icon name="close" [size]="15" />
        </button>
      }
    </div>
    <div class="panes">
      @for (p of placed(); track p.id) {
        @let pane = paneById().get(p.id);
        @if (pane) {
          <div
            class="box"
            [class.active]="!narrow() && placed().length > 1 && p.id === activeId()"
            [style.left.%]="p.x * 100"
            [style.top.%]="p.y * 100"
            [style.width.%]="p.w * 100"
            [style.height.%]="p.h * 100"
            [style.--bench]="bench().color"
          >
            @if (pane.held) {
              <div class="banner">
                <span class="grow">{{ 'term.held' | t }}</span>
                @if (canChange()) {
                  <button class="btn small primary" (click)="call('StartNow', pane.id)">
                    <shed-icon name="go" [size]="14" />{{ 'term.startNow' | t }}
                  </button>
                }
              </div>
            }
            @if (pane.waiting) {
              <div class="banner">
                <span class="grow ellipsis">{{ 'term.waiting' | t }} <code>{{ pane.waiting }}</code></span>
                @if (canChange()) {
                  <button class="btn small primary" (click)="run(pane.id)">
                    <shed-icon name="go" [size]="14" />{{ 'term.run' | t }}
                  </button>
                }
              </div>
            }
            @if (pane.asking) {
              <div class="banner">
                <shed-icon name="ai" [size]="14" />
                <span class="grow ellipsis">{{ pane.asking }}</span>
                @if (canChange()) {
                  <button class="btn small" (click)="call('CancelAsk', pane.id)">
                    <shed-icon name="stop" [size]="14" />{{ 'term.cancelAsk' | t }}
                  </button>
                }
              </div>
            }
            <shed-terminal-view [workspace]="workspace()" [pane]="pane" (focused)="focusedId.set(p.id)" />
          </div>
        }
      } @empty {
        <p class="empty">{{ 'term.noPanes' | t }}</p>
      }
    </div>
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      flex: 1 1 auto;
      min-height: 0;
    }
    .bar {
      display: flex;
      align-items: center;
      gap: 4px;
      min-height: 36px;
      padding: 2px 6px;
      border-bottom: 1px solid var(--divider);
      background: var(--surface);
    }
    .switcher {
      display: flex;
      gap: 4px;
      overflow-x: auto;
      min-width: 0;
    }
    .switcher .label {
      max-width: 9em;
    }
    .close:hover {
      color: var(--danger);
    }
    .panes {
      position: relative;
      flex: 1 1 auto;
      min-height: 0;
      background: var(--divider);
    }
    .box {
      position: absolute;
      display: flex;
      flex-direction: column;
      padding: 1px;
      min-width: 0;
      min-height: 0;
    }
    .box.active {
      box-shadow: inset 0 0 0 1px var(--bench, var(--accent));
    }
    .banner {
      display: flex;
      align-items: center;
      gap: 8px;
      padding: 4px 6px 4px 10px;
      font-size: 12px;
      background: color-mix(in srgb, var(--warn) 14%, var(--surface));
      border-bottom: 1px solid var(--divider);
    }
    .banner code {
      font-size: 12px;
    }
    @media (max-width: 699px) {
      .wide-only {
        display: none;
      }
    }
  `,
})
export class TerminalTab {
  readonly workspace = input.required<string>();
  readonly bench = input.required<Bench>();
  readonly tab = input.required<Tab>();

  private readonly store = inject(WorkspacesStore);
  private readonly dialogs = inject(Dialogs);
  private readonly notices = inject(Notices);
  private readonly views = viewChildren(TerminalView);

  /** The pane clicked last in this view. */
  protected readonly focusedId = signal<string | null>(null);
  protected readonly narrow = signal(false);

  protected readonly canChange = computed(() => this.store.canChange(this.workspace()));
  protected readonly panes = computed(() => this.tab().panes ?? []);
  protected readonly paneById = computed(() => new Map(this.panes().map((p) => [p.id, p])));

  protected readonly activeId = computed(() => {
    const ids = this.paneById();
    const f = this.focusedId();
    if (f && ids.has(f)) return f;
    const a = this.tab().activePane;
    if (a && ids.has(a)) return a;
    return this.panes()[0]?.id ?? null;
  });

  protected readonly active = computed(() => {
    const id = this.activeId();
    return id ? (this.paneById().get(id) ?? null) : null;
  });

  protected readonly placed = computed<Placed[]>(() => {
    if (this.narrow()) {
      const id = this.activeId();
      return id ? [{ id, x: 0, y: 0, w: 1, h: 1 }] : [];
    }
    const layout = this.tab().layout;
    const panes = this.panes();
    if (layout) {
      const out = place(layout).filter((p) => this.paneById().has(p.id));
      if (out.length) return out;
    }
    // No layout: stacked, evenly.
    return panes.map((p, i) => ({ id: p.id, x: 0, y: i / panes.length, w: 1, h: 1 / panes.length }));
  });

  constructor() {
    const mq = matchMedia('(max-width: 699px)');
    this.narrow.set(mq.matches);
    const onChange = (e: MediaQueryListEvent) => this.narrow.set(e.matches);
    mq.addEventListener('change', onChange);
    inject(DestroyRef).onDestroy(() => mq.removeEventListener('change', onChange));
  }

  protected paneLabel(p: Pane): string {
    return p.tool || p.title || tildify(p.cwd, this.store.state(this.workspace())?.home ?? '');
  }

  protected async split(pane: Pane, dir: SplitDir): Promise<void> {
    const id = await this.notices.run(
      this.store.call(this.workspace(), { type: 'Split', content: { pane: pane.id, dir, folder: null } }, 'Pane'),
    );
    if (id) this.focusedId.set(id);
  }

  protected async closePane(pane: Pane): Promise<void> {
    const only = this.panes().length <= 1;
    const ok = await this.dialogs.confirm({
      title: t(only ? 'term.closeLast.title' : 'term.closePane.title'),
      body: t('term.closePane.body', { what: this.paneLabel(pane) }),
      ok: t('term.closePane.ok'),
      danger: true,
    });
    if (!ok) return;
    await this.notices.run(this.store.request(this.workspace(), { type: 'ClosePane', content: { pane: pane.id } }));
  }

  protected call(type: 'StartNow' | 'CancelAsk', pane: string): void {
    void this.notices.run(this.store.request(this.workspace(), { type, content: { pane } }));
  }

  protected run(pane: string): void {
    void this.notices.run(this.store.request(this.workspace(), { type: 'Input', content: { pane, input: [ENTER] } }));
  }

  protected async openLogbook(): Promise<void> {
    const command = await this.dialogs.open<LogbookDialogData, string>(
      LogbookDialog,
      { workspace: this.workspace(), tab: this.tab().id },
      { wide: true },
    );
    const id = this.activeId();
    if (!command || !id || !this.canChange()) return;
    const view = this.views().find((v) => v.pane().id === id);
    const text: InputItem = { type: 'Text', content: command };
    if (view) {
      view.send([text]);
      view.focus();
    } else {
      void this.notices.run(this.store.request(this.workspace(), { type: 'Input', content: { pane: id, input: [text] } }));
    }
  }
}
