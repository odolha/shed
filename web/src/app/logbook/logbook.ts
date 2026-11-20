/**
 * The logbook: commands run in a workspace's shells, searched in the tab,
 * its bench or everywhere (`LogSearch`). Read only, so it works on view-only
 * workspaces too.
 */

import {
  ChangeDetectionStrategy,
  Component,
  ElementRef,
  afterNextRender,
  computed,
  effect,
  inject,
  input,
  output,
  signal,
  untracked,
  viewChild,
} from '@angular/core';
import type { LogEntry, LogScope } from '../../api/gen/api';
import { errorText } from '../../api/errors';
import { WorkspacesStore } from '../../api/workspaces.store';
import { TPipe } from '../../i18n/i18n';
import { Clipboard } from '../../ui/clipboard';
import { Icon } from '../../ui/icon';
import { AgoPipe, ClockPipe } from '../../ui/time';
import { Tooltip } from '../../ui/tooltip';
import { tildify } from '../shared/tabs';

interface Row {
  entry: LogEntry;
  cwd: string;
  where: string;
  duration: string;
}

export function duration(secs: number): string {
  if (!secs || secs < 0) return '';
  if (secs < 1) return `${Math.round(secs * 1000)} ms`;
  if (secs < 60) return `${secs < 10 ? secs.toFixed(1) : Math.round(secs)} s`;
  const m = Math.floor(secs / 60);
  if (m < 60) return `${m}m ${Math.round(secs % 60)}s`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}

@Component({
  selector: 'shed-logbook',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [TPipe, Icon, Tooltip, AgoPipe, ClockPipe],
  template: `
    <div class="search">
      <label class="field grow">
        <span class="sr-only">{{ 'logbook.search' | t }}</span>
        <input
          #field
          class="input"
          type="search"
          [value]="query()"
          (input)="query.set($any($event.target).value)"
          [placeholder]="'logbook.placeholder' | t"
          autocomplete="off"
          autocapitalize="off"
          spellcheck="false"
        />
      </label>
      <div class="scopes" role="radiogroup" [attr.aria-label]="'logbook.scope' | t">
        @for (s of scopes(); track s) {
          <button
            class="btn small"
            role="radio"
            [class.on]="scope() === s"
            [attr.aria-checked]="scope() === s"
            (click)="scope.set(s)"
          >
            {{ 'logbook.scope.' + s | t }}
          </button>
        }
      </div>
    </div>
    <p class="hint dim small">{{ 'logbook.hint' | t }}</p>
    @if (!online()) {
      <div class="chip warn offline">{{ 'logbook.offline' | t }}</div>
    }
    @if (error(); as e) {
      <div class="error-box">{{ e }}</div>
    }
    <ul class="results" [attr.aria-busy]="busy()">
      @for (r of rows(); track $index) {
        <li class="entry" [class.pickable]="pickable()">
          <div class="main" [attr.role]="pickable() ? 'button' : null" [attr.tabindex]="pickable() ? 0 : null"
               (click)="choose(r.entry)" (keydown.enter)="choose(r.entry)">
            <div class="cmd mono">
              @if (r.entry.exit === null || r.entry.exit === undefined) {
                <span class="status dim" [shedTip]="'logbook.noExit' | t">·</span>
              } @else if (r.entry.exit === 0) {
                <span class="status ok" [shedTip]="'logbook.ok' | t">✓</span>
              } @else {
                <span class="status bad" [shedTip]="'logbook.failed' | t: { code: r.entry.exit }">✗ {{ r.entry.exit }}</span>
              }
              <span class="text">{{ r.entry.command }}</span>
            </div>
            <div class="meta dim small">
              <span class="ellipsis" [shedTip]="r.entry.cwd">{{ r.cwd }}</span>
              @if (r.where) {
                <span class="ellipsis">{{ r.where }}</span>
              }
              <span [shedTip]="r.entry.started | clock">{{ r.entry.started | ago }}</span>
              @if (r.duration) {
                <span>{{ r.duration }}</span>
              }
            </div>
          </div>
          <button class="icon-btn small" [shedTip]="'logbook.copy' | t" (click)="clipboard.copy(r.entry.command)">
            <shed-icon name="copy" [size]="15" />
          </button>
        </li>
      } @empty {
        @if (!busy() && !error()) {
          <li class="empty">{{ (query() ? 'logbook.noMatch' : 'logbook.none') | t }}</li>
        }
      }
    </ul>
    @if (busy() && !rows().length) {
      <p class="empty">{{ 'common.loading' | t }}</p>
    }
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      gap: 6px;
      min-height: 0;
    }
    .search {
      display: flex;
      flex-wrap: wrap;
      gap: 8px;
      align-items: center;
    }
    .search .field {
      min-width: 200px;
    }
    .scopes {
      display: flex;
      gap: 4px;
    }
    .hint {
      margin: 0;
    }
    .offline {
      align-self: flex-start;
    }
    .results {
      list-style: none;
      margin: 0;
      padding: 0;
      overflow: auto;
      min-height: 0;
    }
    .entry {
      display: flex;
      align-items: flex-start;
      gap: 6px;
      padding: 6px 4px 6px 8px;
      border-bottom: 1px solid var(--divider);
    }
    .main {
      flex: 1 1 auto;
      min-width: 0;
      border-radius: var(--radius-sm);
    }
    .pickable .main {
      cursor: pointer;
    }
    .pickable .main:hover {
      background: var(--hover);
    }
    .cmd {
      display: flex;
      gap: 8px;
      font-size: 13px;
    }
    .text {
      white-space: pre-wrap;
      word-break: break-all;
    }
    .status {
      flex: none;
      min-width: 1.2em;
    }
    .status.ok {
      color: var(--ok);
    }
    .status.bad {
      color: var(--danger);
    }
    .meta {
      display: flex;
      flex-wrap: wrap;
      gap: 4px 12px;
      margin-top: 2px;
    }
  `,
})
export class Logbook {
  readonly workspace = input.required<string>();
  readonly tab = input<string | null | undefined>(null);
  /** Results can be clicked to pick their command. */
  readonly pickable = input(false);
  readonly pick = output<string>();

  private readonly store = inject(WorkspacesStore);
  protected readonly clipboard = inject(Clipboard);
  private readonly field = viewChild.required<ElementRef<HTMLInputElement>>('field');

  protected readonly query = signal('');
  protected readonly scope = signal<LogScope>('all');
  protected readonly entries = signal<LogEntry[]>([]);
  protected readonly busy = signal(false);
  protected readonly error = signal<string | null>(null);

  protected readonly online = computed(() => this.store.online(this.workspace()));
  protected readonly scopes = computed<LogScope[]>(() => (this.tab() ? ['tab', 'bench', 'all'] : ['all']));

  protected readonly rows = computed<Row[]>(() => {
    const state = this.store.state(this.workspace());
    const home = state?.home ?? '';
    return this.entries().map((entry) => {
      const bench = state?.benches.find((b) => b.id === entry.bench);
      const tab = bench?.tabs.find((t) => t.id === entry.tab) ?? state?.benches.flatMap((b) => b.tabs).find((t) => t.id === entry.tab);
      const benchName = bench?.name ?? entry.bench;
      const tabName = tab?.title ?? (entry.tab && entry.tab !== entry.bench ? entry.tab : '');
      return {
        entry,
        cwd: tildify(entry.cwd, home),
        where: [benchName, tabName].filter(Boolean).join(' › '),
        duration: duration(entry.duration),
      };
    });
  });

  private seq = 0;

  constructor() {
    // A tab given: start with its own history.
    effect(() => {
      const tab = this.tab();
      untracked(() => this.scope.set(tab ? 'tab' : 'all'));
    });

    effect((onCleanup) => {
      const ws = this.workspace();
      const scope = this.scope();
      const query = this.query();
      const tab = this.tab() ?? null;
      const online = this.online();
      const timer = setTimeout(() => {
        if (online) void this.search(ws, scope, query, tab);
      }, 200);
      onCleanup(() => clearTimeout(timer));
    });

    afterNextRender(() => this.field().nativeElement.focus({ preventScroll: true }));
  }

  protected choose(entry: LogEntry): void {
    if (this.pickable()) this.pick.emit(entry.command);
  }

  private async search(ws: string, scope: LogScope, query: string, tab: string | null): Promise<void> {
    const seq = ++this.seq;
    this.busy.set(true);
    try {
      const log = await this.store.call(
        ws,
        { type: 'LogSearch', content: { scope, tab, query, limit: 200 } },
        'Log',
      );
      if (seq !== this.seq) return;
      this.entries.set(log);
      this.error.set(null);
    } catch (e) {
      if (seq !== this.seq) return;
      this.error.set(errorText(e));
    } finally {
      if (seq === this.seq) this.busy.set(false);
    }
  }
}
