import { ChangeDetectionStrategy, Component, computed, effect, inject, input, output, signal } from '@angular/core';
import type { Review, ReviewChange } from '../../api/gen/api';
import { errorText, isFailure } from '../../api/errors';
import { WorkspacesStore } from '../../api/workspaces.store';
import { TPipe } from '../../i18n/i18n';
import { Icon } from '../../ui/icon';
import { Notices } from '../../ui/notices';
import { DiffView } from '../shared/diff-view';
import { tildify } from '../shared/tabs';

/** What Claude changed, for review: each change with Viewed, Why and its diff. */
@Component({
  selector: 'shed-ai-review',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [TPipe, Icon, DiffView],
  template: `
    <header class="head">
      <button class="btn ghost" (click)="back.emit()"><shed-icon name="arrow-left" />{{ 'ai.review.back' | t }}</button>
      <h2 class="grow">{{ 'ai.review.title' | t }}</h2>
      @if (all().length) {
        <span class="dim small">{{ 'ai.review.viewed' | t: { n: viewedCount(), of: all().length } }}</span>
        @if (canChange()) {
          <button class="btn small" [disabled]="explaining()" (click)="explain(keys())">
            <shed-icon name="ai" />{{ 'ai.review.explainAll' | t }}
          </button>
        }
      }
      <button class="icon-btn" (click)="load()" [attr.aria-label]="'common.refresh' | t"><shed-icon name="refresh" /></button>
    </header>
    <div class="body">
      @if (loading()) {
        <div class="empty">{{ 'common.loading' | t }}</div>
      } @else if (unsupported()) {
        <div class="empty">{{ 'ai.review.unsupported' | t }}</div>
      } @else if (error(); as e) {
        <div class="error-box">{{ e }}</div>
      } @else {
        @for (repo of review()?.repos ?? []; track repo.root) {
          <section>
            <h3 class="mono small dim">{{ tilde(repo.root) }}</h3>
            @for (c of repo.changes; track c.key) {
              <article class="change card" [class.viewed]="c.viewed">
                <header class="row">
                  <button class="icon-btn small" (click)="toggle(c.key)" [attr.aria-expanded]="isOpen(c)">
                    <shed-icon [name]="isOpen(c) ? 'chevron-down' : 'chevron-right'" />
                  </button>
                  <span class="status mono" [attr.data-s]="c.status">{{ c.status }}</span>
                  <span class="mono ellipsis grow">{{ c.path }}</span>
                  <span class="chip">{{ c.layer === 'uncommitted' ? ('ai.review.uncommitted' | t) : c.layer }}</span>
                  @if (canChange()) {
                    <button class="btn ghost small" [disabled]="explaining()" (click)="explain([c.key])">
                      {{ 'ai.review.explain' | t }}
                    </button>
                  }
                  <label class="check small">
                    <input type="checkbox" [checked]="c.viewed" [disabled]="!canChange()" (change)="setViewed(c, !c.viewed)" />
                    {{ 'ai.review.viewedBox' | t }}
                  </label>
                </header>
                @if (c.explanation) {
                  <div class="why"><strong>{{ 'ai.review.why' | t }}</strong> {{ c.explanation }}</div>
                }
                @if (isOpen(c)) {
                  <shed-diff [diff]="c.diff" />
                }
              </article>
            }
          </section>
        } @empty {
          <div class="empty">{{ 'ai.review.none' | t }}</div>
        }
      }
    </div>
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      min-height: 0;
      flex: 1;
    }
    .head {
      display: flex;
      align-items: center;
      gap: 8px;
      flex-wrap: wrap;
      padding: 8px 12px;
      border-bottom: 1px solid var(--divider);
    }
    .body {
      flex: 1;
      overflow: auto;
      padding: 12px;
    }
    section {
      margin-bottom: 16px;
    }
    h3 {
      margin: 0 0 6px;
    }
    .change {
      margin-bottom: 8px;
      padding: 6px 8px;
    }
    .change.viewed > header .mono {
      color: var(--text-dim);
    }
    .change header {
      flex-wrap: wrap;
      gap: 6px;
    }
    .status {
      width: 1.4em;
      text-align: center;
      font-weight: 600;
      color: var(--warn);
    }
    .status[data-s='A'],
    .status[data-s='?'] {
      color: var(--ok);
    }
    .status[data-s='D'] {
      color: var(--danger);
    }
    .why {
      margin: 6px 4px 8px;
      padding: 8px 10px;
      border-left: 3px solid var(--accent);
      background: var(--hover);
      border-radius: 4px;
    }
    shed-diff {
      margin-top: 6px;
    }
  `,
})
export class AiReviewView {
  readonly workspace = input.required<string>();
  readonly tab = input.required<string>();
  readonly home = input('');
  readonly canChange = input(false);
  readonly back = output<void>();

  private readonly store = inject(WorkspacesStore);
  private readonly notices = inject(Notices);
  protected readonly review = signal<Review | null>(null);
  protected readonly loading = signal(false);
  protected readonly error = signal<string | null>(null);
  protected readonly unsupported = signal(false);
  protected readonly explaining = signal(false);
  private readonly opened = signal<Map<string, boolean>>(new Map());

  protected readonly all = computed<ReviewChange[]>(() => (this.review()?.repos ?? []).flatMap((r) => r.changes));
  protected readonly keys = computed(() => this.all().map((c) => c.key));
  protected readonly viewedCount = computed(() => this.all().filter((c) => c.viewed).length);

  constructor() {
    effect(() => {
      this.workspace();
      this.tab();
      void this.load();
    });
  }

  protected tilde(p: string): string {
    return tildify(p, this.home());
  }

  protected isOpen(c: ReviewChange): boolean {
    // Changes not viewed yet are open, viewed ones folded, unless toggled.
    return this.opened().get(c.key) ?? !c.viewed;
  }

  protected toggle(key: string): void {
    const c = this.all().find((x) => x.key === key);
    if (!c) return;
    const open = this.isOpen(c);
    this.opened.update((m) => new Map(m).set(key, !open));
  }

  async load(): Promise<void> {
    this.loading.set(true);
    this.error.set(null);
    this.unsupported.set(false);
    try {
      this.review.set(await this.store.call(this.workspace(), { type: 'AiReview', content: { tab: this.tab() } }, 'Review', 60_000));
    } catch (e) {
      if (isFailure(e, 'unsupported')) this.unsupported.set(true);
      else this.error.set(errorText(e));
    } finally {
      this.loading.set(false);
    }
  }

  protected async setViewed(c: ReviewChange, viewed: boolean): Promise<void> {
    this.patch(c.key, { viewed });
    const r = await this.notices.run(
      this.store.request(this.workspace(), { type: 'AiSetViewed', content: { tab: this.tab(), key: c.key, viewed } }),
    );
    if (r === undefined) this.patch(c.key, { viewed: !viewed });
  }

  protected async explain(keys: string[]): Promise<void> {
    if (!keys.length) return;
    this.explaining.set(true);
    try {
      const list = await this.notices.run(
        this.store.call(this.workspace(), { type: 'AiExplain', content: { tab: this.tab(), keys } }, 'Explained', 180_000),
      );
      for (const x of list ?? []) this.patch(x.key, { explanation: x.text });
    } finally {
      this.explaining.set(false);
    }
  }

  private patch(key: string, change: Partial<ReviewChange>): void {
    const r = this.review();
    if (!r) return;
    this.review.set({
      repos: r.repos.map((repo) => ({
        ...repo,
        changes: repo.changes.map((c) => (c.key === key ? { ...c, ...change } : c)),
      })),
    });
  }
}
