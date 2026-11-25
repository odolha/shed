import { ChangeDetectionStrategy, Component, DestroyRef, computed, effect, inject, input, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { RouterLink } from '@angular/router';
import type { Bench, ClosedBench, Tab, WorkspaceInfo, WorkspaceState } from '../../api/gen/api';
import { WorkspacesStore } from '../../api/workspaces.store';
import { TPipe, t } from '../../i18n/i18n';
import { Icon } from '../../ui/icon';
import { LimitsBars } from '../../ui/limits';
import { Menu, MenuTrigger } from '../../ui/menu';
import { AgoPipe } from '../../ui/time';
import { Tooltip } from '../../ui/tooltip';
import { BENCH_COLORS, BenchActions } from '../bench/bench-actions';
import { activePane, closed, inkFor, running, tabActivity, tabDetail, tabIcon } from '../shared/tabs';

interface WsView {
  info: WorkspaceInfo;
  state: WorkspaceState | undefined;
  benches: Bench[];
  closed: ClosedBench[];
  canChange: boolean;
}

/** Every workspace with its benches: Shed's pegboard, for a browser. */
@Component({
  selector: 'shed-overview',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [RouterLink, FormsModule, TPipe, Icon, LimitsBars, Menu, MenuTrigger, AgoPipe, Tooltip],
  template: `
    <div class="page">
      <div class="row head">
        <h1 class="grow">{{ workspace() ? (single()?.info?.name ?? '') : ('overview.title' | t) }}</h1>
        <input
          class="input filter"
          type="search"
          [placeholder]="'overview.filter' | t"
          [attr.aria-label]="'overview.filter' | t"
          [ngModel]="filter()"
          (ngModelChange)="filter.set($event)"
        />
      </div>

      @if (!store.loaded()) {
        <div class="empty">{{ 'common.loading' | t }}</div>
      } @else {
        @for (w of views(); track w.info.id) {
          <section class="ws" [attr.aria-label]="w.info.name">
            <header class="ws-head">
              <a class="ws-name" [routerLink]="['/w', w.info.id]">
                <shed-icon name="server" [size]="18" />
                <h2>{{ w.info.name }}</h2>
              </a>
              @if (w.info.online) {
                <span class="chip ok">{{ 'common.online' | t }}</span>
              } @else {
                <span class="chip">{{ 'common.offline' | t }} · {{ 'common.lastSeen' | t: { ago: (w.info.seen | ago) } }}</span>
              }
              @if (w.info.policy === 'view') {
                <span class="chip warn" [shedTip]="'common.viewOnlyTip' | t">{{ 'common.viewOnly' | t }}</span>
              }
              <span class="grow"></span>
              <shed-limits [limits]="w.state?.limits" />
              <a class="btn ghost small" [routerLink]="['/w', w.info.id, 'logbook']">
                <shed-icon name="logbook" />{{ 'overview.logbook' | t }}
              </a>
              @if (w.canChange) {
                <button class="btn small" (click)="actions.newBench(w.info.id)">
                  <shed-icon name="bench-new" />{{ 'overview.newBench' | t }}
                </button>
              }
            </header>

            @if (!w.state) {
              <div class="empty">{{ 'overview.noState' | t }}</div>
            }

            <div class="benches">
              @for (b of w.benches; track b.id) {
                <article class="bench card" [style.--bench]="b.color">
                  <header class="bench-head">
                    <span class="dot" [style.background]="b.color"></span>
                    <a class="bench-name ellipsis" [routerLink]="['/w', w.info.id, 'b', b.id]">{{ b.name }}</a>
                    @if (b.badge) {
                      <span class="chip">{{ b.badge }}</span>
                    }
                    @if (b.closing) {
                      <span class="chip warn">{{ 'overview.closing' | t }}</span>
                    } @else if (!b.shown) {
                      <span class="chip warn" [shedTip]="'overview.hiddenTip' | t">{{ 'overview.hidden' | t }}</span>
                    } @else {
                      <span class="chip ok">{{ 'overview.open' | t }}</span>
                    }
                    <span class="grow"></span>
                    @if (w.canChange) {
                      <button class="icon-btn small" [shedMenu]="bm" [shedTip]="'common.more' | t">
                        <shed-icon name="more" />
                      </button>
                      <shed-menu #bm>
                        <button class="menu-item" (click)="actions.newTab(w.info.id, b, 'terminal')">
                          <shed-icon name="terminal" />{{ 'tab.new.terminal' | t }}
                        </button>
                        <button class="menu-item" (click)="actions.newTab(w.info.id, b, 'project')">
                          <shed-icon name="project" />{{ 'tab.new.project' | t }}
                        </button>
                        <button class="menu-item" (click)="actions.newTab(w.info.id, b, 'ai')">
                          <shed-icon name="ai" />{{ 'tab.new.ai' | t }}
                        </button>
                        <div class="menu-sep"></div>
                        <button class="menu-item" (click)="actions.rename(w.info.id, ref(b), b.name)">
                          <shed-icon name="pencil" />{{ 'bench.rename' | t }}
                        </button>
                        <button class="menu-item" (click)="actions.badge(w.info.id, ref(b), b.badge)">
                          <shed-icon name="tag" />{{ 'bench.badge' | t }}
                        </button>
                        <div class="menu-label">{{ 'bench.color' | t }}</div>
                        <div class="swatches">
                          @for (c of colors; track c.name) {
                            <button
                              class="menu-item swatch"
                              [style.background]="c.hex"
                              [attr.aria-label]="c.name"
                              (click)="actions.color(w.info.id, ref(b), c.name)"
                            ></button>
                          }
                        </div>
                        <div class="menu-sep"></div>
                        <button class="menu-item" (click)="actions.simple(w.info.id, 'RunStartup', b.id, t('bench.startupDone'))">
                          <shed-icon name="go" />{{ 'bench.runStartup' | t }}
                        </button>
                        <button class="menu-item" (click)="actions.simple(w.info.id, 'RunTeardown', b.id, t('bench.teardownDone'))">
                          <shed-icon name="power" />{{ 'bench.runTeardown' | t }}
                        </button>
                        <button class="menu-item" (click)="actions.simple(w.info.id, 'SaveBench', b.id, t('bench.saved'))">
                          <shed-icon name="bench-save" />{{ 'bench.save' | t }}
                        </button>
                        <div class="menu-sep"></div>
                        <button class="menu-item danger" (click)="actions.close(w.info.id, b)">
                          <shed-icon name="close" />{{ 'bench.close' | t }}
                        </button>
                      </shed-menu>
                    }
                  </header>
                  <ul class="tabs">
                    @for (tab of shownTabs(b.tabs); track tab.id) {
                      <li>
                        <a class="tab" [routerLink]="['/w', w.info.id, 'b', b.id, 't', tab.id]" [class.active]="tab.id === b.activeTab">
                          <span class="tab-icon" [class.busy]="activity(tab)" [style.color]="activity(tab) ? ink(b.color) : null">
                            <shed-icon [name]="icon(tab)" />
                          </span>
                          <span class="tab-text">
                            <span class="row" style="gap: 6px">
                              <span class="ellipsis tab-title">{{ tab.title || ('tab.privateTitle' | t) }}</span>
                              @for (tag of tab.tags ?? []; track tag.label) {
                                <span class="dot tag" [style.background]="tag.color" [shedTip]="tag.label"></span>
                              }
                              @if (activity(tab) === 'waiting') {
                                <span class="chip accent">{{ 'tab.waiting' | t }}</span>
                              } @else if (activity(tab) === 'working') {
                                <span class="chip">{{ 'tab.working' | t }}</span>
                              }
                            </span>
                            <span class="dim small ellipsis">{{ detail(tab, w.state?.home ?? '') }}</span>
                            @if (preview(w.info.id, tab); as lines) {
                              <pre class="preview">{{ lines }}</pre>
                            }
                          </span>
                        </a>
                      </li>
                    }
                  </ul>
                </article>
              }

              @for (c of w.closed; track c.name) {
                <article class="bench card closed">
                  <header class="bench-head">
                    <span class="dot ring" [style.border-color]="c.color"></span>
                    <span class="bench-name ellipsis">{{ c.name }}</span>
                    @if (c.badge) {
                      <span class="chip">{{ c.badge }}</span>
                    }
                    <span class="chip">{{ 'overview.closed' | t }} · {{ c.saved | ago }}</span>
                    <span class="grow"></span>
                    @if (w.canChange) {
                      <button class="btn small" (click)="actions.load(w.info.id, c.name, false)">
                        <shed-icon name="bench-load" />{{ 'overview.openBench' | t }}
                      </button>
                      <button class="icon-btn small" [shedMenu]="cm" [shedTip]="'common.more' | t">
                        <shed-icon name="more" />
                      </button>
                      <shed-menu #cm>
                        <button class="menu-item" (click)="actions.load(w.info.id, c.name, true)">
                          <shed-icon name="go" />{{ 'overview.openAndStart' | t }}
                        </button>
                        <button class="menu-item" (click)="actions.rename(w.info.id, closedRef(c), c.name)">
                          <shed-icon name="pencil" />{{ 'bench.rename' | t }}
                        </button>
                        <button class="menu-item" (click)="actions.badge(w.info.id, closedRef(c), c.badge)">
                          <shed-icon name="tag" />{{ 'bench.badge' | t }}
                        </button>
                        <div class="menu-label">{{ 'bench.color' | t }}</div>
                        <div class="swatches">
                          @for (col of colors; track col.name) {
                            <button
                              class="menu-item swatch"
                              [style.background]="col.hex"
                              [attr.aria-label]="col.name"
                              (click)="actions.color(w.info.id, closedRef(c), col.name)"
                            ></button>
                          }
                        </div>
                        <div class="menu-sep"></div>
                        <button class="menu-item danger" (click)="actions.remove(w.info.id, c.name)">
                          <shed-icon name="trash" />{{ 'bench.delete' | t }}
                        </button>
                      </shed-menu>
                    }
                  </header>
                  <ul class="tabs">
                    @for (tab of shownTabs(c.tabs); track tab.id) {
                      <li class="tab dim">
                        <span class="tab-icon"><shed-icon [name]="icon(tab)" /></span>
                        <span class="tab-text">
                          <span class="ellipsis tab-title">{{ tab.title || ('tab.privateTitle' | t) }}</span>
                          <span class="small ellipsis">{{ detail(tab, w.state?.home ?? '') }}</span>
                        </span>
                      </li>
                    }
                  </ul>
                </article>
              }
            </div>
            @if (w.state && !w.benches.length && !w.closed.length) {
              <div class="empty">{{ filter() ? ('overview.noMatch' | t) : ('overview.noBenches' | t) }}</div>
            }
          </section>
        } @empty {
          <div class="empty">
            <p>{{ 'overview.noWorkspaces' | t }}</p>
            <a class="btn" routerLink="/devices"><shed-icon name="device" />{{ 'nav.devices' | t }}</a>
          </div>
        }
      }
    </div>
  `,
  styles: `
    .head {
      margin-bottom: 12px;
      flex-wrap: wrap;
    }
    .filter {
      width: min(280px, 100%);
    }
    .ws {
      margin-bottom: 28px;
    }
    .ws-head {
      display: flex;
      align-items: center;
      flex-wrap: wrap;
      gap: 8px;
      padding: 8px 0;
      margin-bottom: 8px;
      border-bottom: 1px solid var(--divider);
    }
    .ws-name {
      display: flex;
      align-items: center;
      gap: 8px;
      color: var(--text);
      text-decoration: none;
    }
    .benches {
      display: grid;
      grid-template-columns: repeat(auto-fill, minmax(min(340px, 100%), 1fr));
      gap: 12px;
    }
    .bench {
      overflow: hidden;
      border-top: 3px solid var(--bench, var(--divider));
    }
    .bench.closed {
      border-top-color: var(--divider);
    }
    .bench-head {
      display: flex;
      align-items: center;
      gap: 8px;
      padding: 8px 8px 8px 12px;
      border-bottom: 1px solid var(--divider);
      min-height: 46px;
    }
    .bench-name {
      font-weight: 600;
      font-size: 15px;
      color: var(--text);
      text-decoration: none;
    }
    @media (pointer: coarse) {
      /* Names stay text-sized, their targets finger-sized. */
      .bench-name,
      .ws-name {
        display: inline-flex;
        align-items: center;
        min-height: 44px;
      }
      .tab {
        min-height: 44px;
      }
    }
    a.bench-name:hover {
      text-decoration: underline;
    }
    .dot.ring {
      background: transparent;
      border: 2px solid;
    }
    .dot.tag {
      width: 7px;
      height: 7px;
    }
    .tabs {
      list-style: none;
      margin: 0;
      padding: 4px;
    }
    .tab {
      display: flex;
      gap: 10px;
      padding: 6px 8px;
      border-radius: var(--radius-sm);
      color: var(--text);
      text-decoration: none;
      min-width: 0;
    }
    a.tab:hover {
      background: var(--hover);
    }
    a.tab.active .tab-title {
      font-weight: 600;
    }
    .tab-icon {
      padding-top: 2px;
      color: var(--text-dim);
    }
    .tab-text {
      display: flex;
      flex-direction: column;
      min-width: 0;
      flex: 1;
    }
    .preview {
      margin: 4px 0 0;
      padding: 4px 6px;
      max-height: 4.6em;
      overflow: hidden;
      border-radius: 4px;
      background: var(--term-bg);
      color: var(--text-dim);
      font: 11px/1.4 var(--mono);
      white-space: pre;
    }
    .swatches {
      display: grid;
      grid-template-columns: repeat(8, 1fr);
      gap: 4px;
      padding: 4px 6px;
    }
    .swatch {
      min-height: 20px;
      height: 20px;
      padding: 0;
      border-radius: 50%;
    }
    .swatch:hover,
    .swatch:focus-visible {
      outline: 2px solid var(--text);
    }
  `,
})
export class Overview {
  /** From the route: show only this workspace. */
  readonly workspace = input<string | undefined>();
  protected readonly store = inject(WorkspacesStore);
  protected readonly actions = inject(BenchActions);
  protected readonly filter = signal('');
  protected readonly colors = BENCH_COLORS;
  protected readonly t = t;
  private readonly previews = signal<Record<string, Record<string, string[]>>>({});

  protected readonly views = computed<WsView[]>(() => {
    const only = this.workspace();
    const q = this.filter().trim().toLowerCase();
    return this.store
      .workspaces()
      .filter((w) => !only || w.id === only)
      .map((info) => {
        const state = this.store.state(info.id);
        const match = (name: string, tabs: Tab[]) =>
          !q ||
          name.toLowerCase().includes(q) ||
          tabs.some(
            (t) =>
              t.title.toLowerCase().includes(q) ||
              (t.tags ?? []).some((g) => g.label.toLowerCase().includes(q)) ||
              tabDetail(t, '').toLowerCase().includes(q),
          );
        return {
          info,
          state,
          benches: (state?.benches ?? []).filter((b) => match(b.name, b.tabs)),
          closed: (state?.closed ?? []).filter((b) => match(b.name, b.tabs)),
          canChange: this.store.canChange(info.id),
        };
      });
  });

  protected readonly single = computed(() => this.views()[0]);

  constructor() {
    // Previews of the terminals, refreshed while the page is shown.
    effect((onCleanup) => {
      const ids = this.store
        .workspaces()
        .filter((w) => w.online && (!this.workspace() || w.id === this.workspace()))
        .map((w) => w.id);
      if (!this.store.connected() || !ids.length) return;
      const load = () => {
        if (document.visibilityState !== 'visible') return;
        for (const id of ids) {
          this.store
            .call(id, { type: 'Previews' }, 'Previews')
            .then((list) => {
              const byPane: Record<string, string[]> = {};
              for (const p of list) byPane[p.pane] = p.lines;
              this.previews.update((all) => ({ ...all, [id]: byPane }));
            })
            .catch(() => undefined);
        }
      };
      load();
      const timer = setInterval(load, 8000);
      onCleanup(() => clearInterval(timer));
    });
    inject(DestroyRef).onDestroy(() => this.previews.set({}));
  }

  protected ref(b: Bench) {
    return running(b.id);
  }

  protected closedRef(c: ClosedBench) {
    return closed(c.name);
  }

  protected shownTabs(tabs: Tab[]): Tab[] {
    return tabs;
  }

  protected icon(tab: Tab): string {
    return tabIcon(tab);
  }

  protected activity(tab: Tab) {
    return tabActivity(tab);
  }

  protected detail(tab: Tab, home: string): string {
    return tab.private ? '' : tabDetail(tab, home);
  }

  protected ink(color: string): string {
    return inkFor(color);
  }

  protected preview(ws: string, tab: Tab): string | null {
    if (tab.private || (tab.kind.type !== 'Terminal' && tab.kind.type !== 'Remote')) return null;
    const pane = activePane(tab);
    const lines = pane ? this.previews()[ws]?.[pane.id] : undefined;
    if (!lines?.length) return null;
    const trimmed = [...lines];
    while (trimmed.length && !trimmed[trimmed.length - 1]?.trim()) trimmed.pop();
    return trimmed.slice(-3).join('\n') || null;
  }
}
