import { ChangeDetectionStrategy, Component, computed, effect, inject, input } from '@angular/core';
import { Router, RouterLink } from '@angular/router';
import type { Bench, Tab } from '../../api/gen/api';
import { WorkspacesStore } from '../../api/workspaces.store';
import { TPipe, t } from '../../i18n/i18n';
import { Icon } from '../../ui/icon';
import { LimitsBars } from '../../ui/limits';
import { ContextMenuTrigger, Menu, MenuTrigger } from '../../ui/menu';
import { AgoPipe } from '../../ui/time';
import { Tooltip } from '../../ui/tooltip';
import { AiTab } from '../ai/ai-tab';
import { ProjectTab } from '../project/project-tab';
import { TerminalTab } from '../terminal/terminal-tab';
import { inkFor, running, tabActivity, tabIcon } from '../shared/tabs';
import { BENCH_COLORS, BenchActions } from './bench-actions';

/** A bench: its tab bar, and the tab shown. URLs: `/w/:workspace/b/:bench/t/:tab`. */
@Component({
  selector: 'shed-bench-page',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [RouterLink, TPipe, Icon, LimitsBars, Menu, MenuTrigger, ContextMenuTrigger, AgoPipe, Tooltip, AiTab, ProjectTab, TerminalTab],
  template: `
    @if (benchSig(); as b) {
      <header class="head" [style.--bench]="b.color">
        <a class="icon-btn" [routerLink]="['/w', workspace()]" [shedTip]="'bench.back' | t">
          <shed-icon name="arrow-left" [size]="18" />
        </a>
        <span class="dot" [style.background]="b.color"></span>
        <button class="name ellipsis" [shedMenu]="benchMenu">{{ b.name }}</button>
        @if (b.badge) {
          <span class="chip">{{ b.badge }}</span>
        }
        @if (!online()) {
          <span class="chip">{{ 'common.offline' | t }} · {{ 'common.lastSeen' | t: { ago: (info()?.seen | ago) } }}</span>
        } @else if (info()?.policy === 'view') {
          <span class="chip warn" [shedTip]="'common.viewOnlyTip' | t">{{ 'common.viewOnly' | t }}</span>
        }
        <span class="grow"></span>
        @for (l of b.links ?? []; track l.url) {
          <a class="btn ghost small wide-only" [href]="l.url" target="_blank" rel="noopener noreferrer">
            <shed-icon name="link" />{{ l.name }}
          </a>
        }
        @if (hasAi()) {
          <shed-limits class="wide-only" [limits]="state()?.limits" />
          <shed-limits class="narrow-only" [compact]="true" [limits]="state()?.limits" />
        }
        <button class="icon-btn" [shedMenu]="benchMenu" [shedTip]="'bench.menu' | t">
          <shed-icon name="more" [size]="18" />
        </button>
      </header>

      <shed-menu #benchMenu>
        <div class="menu-label">{{ b.name }}</div>
        @if (canChange()) {
          <button class="menu-item" (click)="actions.rename(workspace(), ref(), b.name)">
            <shed-icon name="pencil" />{{ 'bench.rename' | t }}
          </button>
          <button class="menu-item" (click)="actions.badge(workspace(), ref(), b.badge)">
            <shed-icon name="tag" />{{ 'bench.badge' | t }}
          </button>
          <div class="menu-label">{{ 'bench.color' | t }}</div>
          <div class="swatches">
            @for (c of colors; track c.name) {
              <button
                class="menu-item swatch"
                [style.background]="c.hex"
                [attr.aria-label]="c.name"
                (click)="actions.color(workspace(), ref(), c.name)"
              ></button>
            }
          </div>
          <div class="menu-sep"></div>
          <button class="menu-item" (click)="actions.simple(workspace(), 'RunStartup', b.id, t('bench.startupDone'))">
            <shed-icon name="go" />{{ 'bench.runStartup' | t }}
          </button>
          <button class="menu-item" (click)="actions.simple(workspace(), 'RunTeardown', b.id, t('bench.teardownDone'))">
            <shed-icon name="power" />{{ 'bench.runTeardown' | t }}
          </button>
        }
        <a class="menu-item" [routerLink]="['/w', workspace(), 'logbook']" [queryParams]="{ tab: current()?.id }">
          <shed-icon name="logbook" />{{ 'bench.logbook' | t }}
        </a>
        @for (l of b.links ?? []; track l.url) {
          <a class="menu-item" [href]="l.url" target="_blank" rel="noopener noreferrer">
            <shed-icon name="link" />{{ l.name }}
          </a>
        }
        @if (canChange()) {
          <div class="menu-sep"></div>
          <button class="menu-item danger" (click)="closeBench(b)">
            <shed-icon name="close" />{{ 'bench.close' | t }}
          </button>
        }
      </shed-menu>

      <nav class="tabbar" [style.--bench]="b.color" role="tablist" [attr.aria-label]="'bench.tabs' | t">
        @for (tab of b.tabs; track tab.id) {
          <a
            class="tab"
            role="tab"
            [class.active]="tab.id === current()?.id"
            [class.gap]="tab.divider && tab.divider !== 'none' && !$first"
            [attr.aria-selected]="tab.id === current()?.id"
            [routerLink]="['/w', workspace(), 'b', b.id, 't', tab.id]"
            replaceUrl
            [shedContextMenu]="canChange() ? tabMenu : null"
            (contextmenu)="menuTab = tab"
            (touchstart)="menuTab = tab"
          >
            <span [style.color]="activity(tab) ? ink(b.color) : null" [class.pulse]="activity(tab) === 'working'">
              <shed-icon [name]="icon(tab)" />
            </span>
            <span class="ellipsis">{{ tab.title || ('tab.privateTitle' | t) }}</span>
            @for (tag of tab.tags ?? []; track tag.label) {
              <span class="dot tag" [style.background]="tag.color"></span>
            }
          </a>
        }
        @if (canChange()) {
          <button class="icon-btn small add" [shedMenu]="newTab" [shedTip]="'tab.new' | t">
            <shed-icon name="plus" />
          </button>
          @if (current(); as cur) {
            <button class="icon-btn small" [shedMenu]="tabMenu" (click)="menuTab = cur" [shedTip]="'tab.menu' | t">
              <shed-icon name="chevron-down" />
            </button>
          }
        }
      </nav>

      <shed-menu #newTab>
        <button class="menu-item" (click)="actions.newTab(workspace(), b, 'terminal')">
          <shed-icon name="terminal" />{{ 'tab.new.terminal' | t }}
        </button>
        <button class="menu-item" (click)="actions.newTab(workspace(), b, 'project')">
          <shed-icon name="project" />{{ 'tab.new.project' | t }}
        </button>
        <button class="menu-item" (click)="actions.newTab(workspace(), b, 'ai')">
          <shed-icon name="ai" />{{ 'tab.new.ai' | t }}
        </button>
        <button class="menu-item" (click)="actions.newTab(workspace(), b, 'remote')">
          <shed-icon name="server" />{{ 'tab.new.remoteTab' | t }}
        </button>
      </shed-menu>

      <shed-menu #tabMenu>
        @if (menuTab; as mt) {
          <div class="menu-label">{{ mt.title }}</div>
          <button class="menu-item" (click)="actions.renameTab(workspace(), ref(), mt)">
            <shed-icon name="pencil" />{{ 'tab.rename' | t }}
          </button>
          @if (mt.kind.type === 'Remote') {
            <button class="menu-item" (click)="actions.reconnect(workspace(), mt)">
              <shed-icon name="refresh" />{{ 'tab.reconnect' | t }}
            </button>
          }
          <button class="menu-item" (click)="actions.setPrivate(workspace(), ref(), mt, !mt.private)">
            <shed-icon name="private" />{{ (mt.private ? 'tab.unprivate' : 'tab.private') | t }}
          </button>
          <div class="menu-sep"></div>
          <button class="menu-item danger" (click)="closeTab(mt)">
            <shed-icon name="close" />{{ 'tab.close' | t }}
          </button>
        }
      </shed-menu>

      <div class="body">
        @if (current(); as tab) {
          @if (tab.private) {
            <div class="empty">
              <shed-icon name="private" [size]="28" />
              <p>{{ 'tab.privateBody' | t }}</p>
            </div>
          } @else if (!online()) {
            <div class="empty">
              <shed-icon name="warning" [size]="28" />
              <p>{{ 'bench.offline' | t: { name: info()?.name ?? '' } }}</p>
            </div>
          } @else {
            @switch (tab.kind.type) {
              @case ('Ai') {
                @defer (on immediate) {
                  <shed-ai-tab [workspace]="workspace()" [bench]="b" [tab]="tab" />
                } @loading (after 150ms) {
                  <div class="empty">{{ 'common.loading' | t }}</div>
                }
              }
              @case ('Project') {
                @defer (on immediate) {
                  <shed-project-tab [workspace]="workspace()" [bench]="b" [tab]="tab" />
                } @loading (after 150ms) {
                  <div class="empty">{{ 'common.loading' | t }}</div>
                }
              }
              @default {
                @defer (on immediate) {
                  <shed-terminal-tab [workspace]="workspace()" [bench]="b" [tab]="tab" />
                } @loading (after 150ms) {
                  <div class="empty">{{ 'common.loading' | t }}</div>
                }
              }
            }
          }
        } @else {
          <div class="empty">
            <p>{{ 'bench.noTabs' | t }}</p>
            @if (canChange()) {
              <button class="btn" [shedMenu]="newTab"><shed-icon name="tab-new" />{{ 'tab.new' | t }}</button>
            }
          </div>
        }
      </div>
    } @else if (!store.loaded()) {
      <div class="empty">{{ 'common.loading' | t }}</div>
    } @else {
      <div class="empty">
        <p>{{ 'bench.gone' | t }}</p>
        <a class="btn" [routerLink]="['/w', workspace()]"><shed-icon name="arrow-left" />{{ 'bench.back' | t }}</a>
      </div>
    }
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      min-height: 0;
      height: 100%;
    }
    .head {
      display: flex;
      align-items: center;
      gap: 8px;
      padding: 4px 8px;
      border-top: 3px solid var(--bench);
      background: color-mix(in srgb, var(--bench) 6%, var(--bg));
      min-height: 46px;
    }
    .name {
      border: 0;
      background: none;
      color: var(--text);
      font: inherit;
      font-weight: 600;
      font-size: 16px;
      padding: 4px 6px;
      border-radius: var(--radius-sm);
      cursor: pointer;
    }
    .name:hover {
      background: var(--hover);
    }
    .tabbar {
      display: flex;
      align-items: center;
      gap: 2px;
      padding: 0 6px;
      overflow-x: auto;
      scrollbar-width: thin;
      flex: none;
      border-bottom: 1px solid color-mix(in srgb, var(--bench) 35%, var(--divider));
      background: color-mix(in srgb, var(--bench) 6%, var(--bg));
    }
    .tab {
      display: flex;
      align-items: center;
      gap: 6px;
      max-width: 220px;
      min-width: 0;
      flex: none;
      padding: 8px 12px 6px;
      border-bottom: 2px solid transparent;
      color: var(--text-dim);
      text-decoration: none;
      white-space: nowrap;
    }
    .tab:hover {
      color: var(--text);
      background: var(--hover);
    }
    .tab.active {
      color: var(--text);
      font-weight: 600;
      border-bottom-color: var(--bench);
      background: color-mix(in srgb, var(--bench) 10%, var(--surface));
    }
    .tab.gap {
      margin-left: 14px;
    }
    .dot.tag {
      width: 7px;
      height: 7px;
    }
    .add {
      margin-left: 4px;
    }
    @media (pointer: coarse) {
      .tab {
        min-height: 44px;
      }
      .tabbar {
        scrollbar-width: none;
      }
      .name {
        min-height: 40px;
      }
    }
    .pulse {
      animation: pulse 1.4s ease-in-out infinite;
    }
    @keyframes pulse {
      50% {
        color: var(--text-dim);
      }
    }
    .body {
      flex: 1 1 auto;
      min-height: 0;
      display: flex;
      flex-direction: column;
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
    .narrow-only {
      display: none;
    }
    @media (max-width: 699px) {
      .wide-only {
        display: none;
      }
      .narrow-only {
        display: flex;
      }
      /* The keyboard up: the tab bar stays, the bench's head goes. */
      :host-context(.kb) .head {
        display: none;
      }
    }
  `,
})
export class BenchPage {
  readonly workspace = input.required<string>();
  readonly bench = input.required<string>();
  readonly tab = input<string | undefined>();

  protected readonly store = inject(WorkspacesStore);
  protected readonly actions = inject(BenchActions);
  private readonly router = inject(Router);
  protected readonly colors = BENCH_COLORS;
  protected readonly t = t;
  protected menuTab: Tab | null = null;

  protected readonly info = computed(() => this.store.info(this.workspace()));
  protected readonly state = computed(() => this.store.state(this.workspace()));
  protected readonly benchSig = computed(() => this.state()?.benches.find((b) => b.id === this.bench()));
  protected readonly online = computed(() => this.store.online(this.workspace()));
  protected readonly canChange = computed(() => this.store.canChange(this.workspace()));
  protected readonly ref = computed(() => running(this.bench()));
  protected readonly hasAi = computed(() => !!this.benchSig()?.tabs.some((x) => x.kind.type === 'Ai'));

  protected readonly current = computed<Tab | undefined>(() => {
    const b = this.benchSig();
    if (!b) return undefined;
    const want = this.tab();
    return b.tabs.find((x) => x.id === want) ?? b.tabs.find((x) => x.id === b.activeTab) ?? b.tabs[0];
  });

  constructor() {
    // The page's title says where we are.
    effect(() => {
      const b = this.benchSig();
      const cur = this.current();
      document.title = b ? `${cur ? cur.title + ' · ' : ''}${b.name} · Shed` : 'Shed';
    });
    // A tab that went away (closed here or elsewhere): show the bench's.
    effect(() => {
      const b = this.benchSig();
      const want = this.tab();
      if (b && want && !b.tabs.some((x) => x.id === want)) {
        void this.router.navigate(['/w', this.workspace(), 'b', b.id], { replaceUrl: true });
      }
    });
  }

  protected icon(tab: Tab): string {
    return tabIcon(tab);
  }

  protected activity(tab: Tab) {
    return tabActivity(tab);
  }

  protected ink(color: string): string {
    return inkFor(color);
  }

  protected async closeTab(tab: Tab): Promise<void> {
    await this.actions.closeTab(this.workspace(), tab);
  }

  protected async closeBench(b: Bench): Promise<void> {
    if (await this.actions.close(this.workspace(), b)) {
      void this.router.navigate(['/w', this.workspace()]);
    }
  }
}
