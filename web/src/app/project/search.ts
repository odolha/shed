/**
 * Search in the project's files: hits stream in by file (`Topic Search`); a
 * new query ends the old search (unsubscribing stops it).
 */

import {
  ChangeDetectionStrategy,
  Component,
  OnDestroy,
  computed,
  inject,
  input,
  output,
  signal,
  type WritableSignal,
} from '@angular/core';
import type { SearchHits } from '../../api/gen/api';
import type { Sub } from '../../api/socket';
import { WorkspacesStore } from '../../api/workspaces.store';
import { ApiFailure, errorText } from '../../api/errors';
import { TPipe, t } from '../../i18n/i18n';
import { Icon } from '../../ui/icon';
import { Tooltip } from '../../ui/tooltip';
import { type Segment, rangeMarks, segment, splitPath } from './highlight';

export interface OpenAt {
  path: string;
  line: number | null;
}

interface FileHits {
  path: string;
  dir: string;
  name: string;
  open: boolean;
  hits: { line: number; segs: Segment[] }[];
}

/** Long lines are cut around the first match. */
function trimHit(text: string, ranges: number[]): { text: string; ranges: number[] } {
  const chars = Array.from(text);
  const first = ranges[0] ?? 0;
  if (chars.length <= 200 || first < 60) return { text: chars.slice(0, 400).join(''), ranges };
  const cut = first - 40;
  return { text: '…' + chars.slice(cut, cut + 360).join(''), ranges: ranges.map((r) => r - cut + 1) };
}

@Component({
  selector: 'shed-project-search',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [TPipe, Icon, Tooltip],
  template: `
    <form class="form" (submit)="$event.preventDefault(); start()">
      <div class="row">
        <input
          class="input"
          [value]="query()"
          (input)="onQuery($any($event.target).value)"
          [placeholder]="'project.search.placeholder' | t"
          [attr.aria-label]="'project.search' | t"
          spellcheck="false"
        />
      </div>
      <div class="row toggles">
        <button type="button" class="icon-btn small mono" [class.on]="regex()" (click)="toggle(regex)" [shedTip]="'project.search.regex' | t" [attr.aria-pressed]="regex()">.*</button>
        <button type="button" class="icon-btn small mono" [class.on]="caseSensitive()" (click)="toggle(caseSensitive)" [shedTip]="'project.search.case' | t" [attr.aria-pressed]="caseSensitive()">Aa</button>
        <button type="button" class="icon-btn small mono" [class.on]="wholeWord()" (click)="toggle(wholeWord)" [shedTip]="'project.search.word' | t" [attr.aria-pressed]="wholeWord()">ab</button>
        <button type="button" class="icon-btn small" [class.on]="showGlobs()" (click)="showGlobs.set(!showGlobs())" [shedTip]="'project.search.globs' | t" [attr.aria-pressed]="showGlobs()">
          <shed-icon name="more" [size]="14" />
        </button>
        <span class="grow"></span>
        @if (running()) {
          <span class="dim small">{{ 'project.search.running' | t }}</span>
          <button type="button" class="btn small ghost" (click)="stop()">
            <shed-icon name="stop" [size]="12" />{{ 'project.search.stop' | t }}
          </button>
        }
      </div>
      @if (showGlobs()) {
        <input class="input small-input" [value]="include()" (change)="include.set($any($event.target).value); start()" [placeholder]="'project.search.include' | t" [attr.aria-label]="'project.search.include' | t" />
        <input class="input small-input" [value]="exclude()" (change)="exclude.set($any($event.target).value); start()" [placeholder]="'project.search.exclude' | t" [attr.aria-label]="'project.search.exclude' | t" />
      }
    </form>
    <div class="summary dim small" role="status">
      @if (error(); as e) {
        <span class="danger-text">{{ e }}</span>
      } @else if (started()) {
        {{ 'project.search.count' | t: { hits: hitCount(), files: files().length } }}
        @if (truncated()) {
          · {{ 'project.search.truncated' | t }}
        }
      }
    </div>
    <div class="results">
      @for (f of files(); track f.path) {
        <button class="file" (click)="toggleFile(f.path)" [attr.aria-expanded]="f.open">
          <shed-icon [name]="f.open ? 'chevron-down' : 'chevron-right'" [size]="12" />
          <span class="ellipsis"><span>{{ f.name }}</span>&ngsp;<span class="dim small">{{ f.dir }}</span></span>
          <span class="chip">{{ f.hits.length }}</span>
        </button>
        @if (f.open) {
          @for (h of f.hits; track $index) {
            <button class="hit" (click)="open.emit({ path: f.path, line: h.line })">
              <span class="ln dim">{{ h.line }}</span>
              <span class="mono ellipsis">@for (s of h.segs; track $index) {<span [class]="s.cls ?? ''">{{ s.text }}</span>}</span>
            </button>
          }
        }
      } @empty {
        @if (started() && !running() && !error()) {
          <div class="empty small">{{ 'project.noMatch' | t }}</div>
        }
      }
    </div>
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      min-height: 0;
    }
    .form {
      display: flex;
      flex-direction: column;
      gap: 6px;
      padding: 8px;
      border-bottom: 1px solid var(--divider);
    }
    .toggles {
      gap: 2px;
    }
    .toggles .mono {
      font-size: 12px;
      font-weight: 600;
    }
    .small-input {
      min-height: 28px;
      font-size: 12px;
    }
    .summary {
      padding: 4px 10px;
      min-height: 24px;
    }
    .results {
      flex: 1 1 auto;
      overflow: auto;
      padding-bottom: 8px;
    }
    .file,
    .hit {
      display: flex;
      align-items: center;
      gap: 6px;
      width: 100%;
      border: 0;
      background: transparent;
      color: var(--text);
      font: inherit;
      text-align: left;
      cursor: pointer;
    }
    .file {
      min-height: 28px;
      padding: 0 8px;
      font-size: 13px;
    }
    .file .ellipsis {
      flex: 1 1 auto;
    }
    .hit {
      min-height: 24px;
      padding: 0 8px 0 26px;
      font-size: 12px;
    }
    .file:hover,
    .hit:hover {
      background: var(--hover);
    }
    .ln {
      min-width: 2.5em;
      text-align: right;
      font-family: var(--mono);
    }
    .match {
      background: color-mix(in srgb, var(--accent) 30%, transparent);
      color: var(--text);
      border-radius: 2px;
    }
    @media (pointer: coarse) {
      .hit {
        min-height: 34px;
      }
    }
  `,
})
export class ProjectSearch implements OnDestroy {
  readonly workspace = input.required<string>();
  readonly tab = input.required<string>();
  readonly open = output<OpenAt>();

  private readonly store = inject(WorkspacesStore);

  protected readonly query = signal('');
  protected readonly regex = signal(false);
  protected readonly caseSensitive = signal(false);
  protected readonly wholeWord = signal(false);
  protected readonly showGlobs = signal(false);
  protected readonly include = signal('');
  protected readonly exclude = signal('');

  protected readonly files = signal<FileHits[]>([]);
  protected readonly running = signal(false);
  protected readonly started = signal(false);
  protected readonly truncated = signal(false);
  protected readonly error = signal<string | null>(null);
  protected readonly hitCount = computed(() => this.files().reduce((n, f) => n + f.hits.length, 0));

  private sub: Sub | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;

  protected onQuery(value: string): void {
    this.query.set(value);
    if (this.timer) clearTimeout(this.timer);
    this.timer = setTimeout(() => this.start(), 350);
  }

  protected toggle(s: WritableSignal<boolean>): void {
    s.set(!s());
    this.start();
  }

  protected toggleFile(path: string): void {
    this.files.update((fs) => fs.map((f) => (f.path === path ? { ...f, open: !f.open } : f)));
  }

  protected stop(): void {
    this.sub?.close();
    this.sub = null;
    this.running.set(false);
  }

  start(): void {
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    this.stop();
    const query = this.query();
    this.files.set([]);
    this.truncated.set(false);
    this.error.set(null);
    if (query.trim().length < 2) {
      this.started.set(false);
      return;
    }
    this.started.set(true);
    this.running.set(true);
    const include = this.include().trim() || null;
    const exclude = this.exclude().trim() || null;
    const sub = this.store.socket.subscribe(
      this.workspace(),
      {
        type: 'Search',
        content: {
          tab: this.tab(),
          query,
          regex: this.regex(),
          caseSensitive: this.caseSensitive(),
          wholeWord: this.wholeWord(),
          include,
          exclude,
        },
      },
      {
        event: (e) => {
          if (this.sub !== sub) return;
          if (e.type === 'SearchHits') this.add(e.content);
          else if (e.type === 'SearchDone') {
            this.truncated.set(!!e.content.truncated);
            if (e.content.error) this.error.set(e.content.error);
            this.running.set(false);
            sub.close();
            this.sub = null;
          }
        },
        ended: (err) => {
          if (this.sub !== sub) return;
          this.running.set(false);
          this.sub = null;
          if (err) this.error.set(errorText(ApiFailure.from(err)));
        },
        paused: () => {
          if (this.sub !== sub) return;
          this.error.set(t('error.offline'));
        },
      },
    );
    this.sub = sub;
  }

  private add(found: SearchHits): void {
    const hits = found.hits.map((h) => {
      const cut = trimHit(h.text, h.ranges);
      return { line: h.line, segs: segment(cut.text, rangeMarks(cut.ranges, 'match')) };
    });
    this.files.update((fs) => {
      const i = fs.findIndex((f) => f.path === found.path);
      if (i >= 0) {
        const next = [...fs];
        next[i] = { ...fs[i]!, hits: [...fs[i]!.hits, ...hits] };
        return next;
      }
      const [dir, name] = splitPath(found.path);
      return [...fs, { path: found.path, dir, name, open: true, hits }];
    });
  }

  ngOnDestroy(): void {
    if (this.timer) clearTimeout(this.timer);
    this.stop();
  }
}
