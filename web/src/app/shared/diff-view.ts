import { ChangeDetectionStrategy, Component, computed, input, output } from '@angular/core';
import type { Diff, HunkAction, Picture } from '../../api/gen/api';
import { TPipe } from '../../i18n/i18n';
import { Icon } from '../../ui/icon';

export interface HunkEvent {
  index: number;
  header: string;
  action: HunkAction;
}

export function pictureUrl(p: Picture | null | undefined): string | null {
  return p ? `data:${p.mediaType};base64,${p.data}` : null;
}

/**
 * A file's changes, hunk by hunk, with line numbers; pictures side by side.
 * With `actions`, each hunk has buttons (stage, unstage, discard).
 */
@Component({
  selector: 'shed-diff',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [TPipe, Icon],
  template: `
    @let d = diff();
    @if (d.binary && !d.oldPicture && !d.newPicture) {
      <div class="empty">{{ 'diff.binary' | t }}</div>
    }
    @if (d.oldPicture || d.newPicture) {
      <div class="pictures">
        <figure>
          <figcaption class="dim small">{{ 'diff.before' | t }}</figcaption>
          @if (oldUrl(); as u) {
            <img [src]="u" alt="" />
          } @else {
            <div class="none dim small">{{ 'diff.none' | t }}</div>
          }
        </figure>
        <figure>
          <figcaption class="dim small">{{ 'diff.after' | t }}</figcaption>
          @if (newUrl(); as u) {
            <img [src]="u" alt="" />
          } @else {
            <div class="none dim small">{{ 'diff.none' | t }}</div>
          }
        </figure>
      </div>
    }
    @for (h of d.hunks; track $index; let i = $index) {
      <section class="hunk">
        <header>
          <span class="mono small dim grow ellipsis">{{ h.header }}</span>
          @for (a of actions(); track a) {
            <button
              class="btn small"
              [class.danger]="a === 'discard'"
              [class.ghost]="a !== 'discard'"
              [disabled]="disabled()"
              (click)="hunk.emit({ index: i, header: h.header, action: a })"
            >
              <shed-icon [name]="a === 'discard' ? 'discard' : a === 'stage' ? 'plus' : 'minus'" [size]="13" />
              {{ 'diff.hunk.' + a | t }}
            </button>
          }
        </header>
        @for (l of h.lines; track $index) {
          <div class="diff-line" [class.added]="l.kind === 'added'" [class.removed]="l.kind === 'removed'">
            <span class="num">{{ l.old ?? '' }}</span>
            <span class="num">{{ l.new ?? '' }}</span>
            <span class="sign">{{ l.kind === 'added' ? '+' : l.kind === 'removed' ? '−' : ' ' }}</span>
            <span>{{ l.text }}</span>
          </div>
        }
      </section>
    } @empty {
      @if (!d.binary && !d.oldPicture && !d.newPicture) {
        <div class="empty">{{ 'diff.empty' | t }}</div>
      }
    }
  `,
  styles: `
    :host {
      display: block;
    }
    .hunk {
      margin-bottom: 10px;
      border: 1px solid var(--divider);
      border-radius: var(--radius-sm);
      overflow: hidden;
      background: var(--surface);
    }
    .hunk header {
      display: flex;
      align-items: center;
      gap: 6px;
      padding: 2px 4px 2px 10px;
      background: color-mix(in srgb, var(--info) 8%, var(--surface));
      border-bottom: 1px solid var(--divider);
      min-height: 30px;
    }
    .pictures {
      display: grid;
      grid-template-columns: 1fr 1fr;
      gap: 12px;
      margin-bottom: 10px;
    }
    figure {
      margin: 0;
    }
    figure img {
      max-width: 100%;
      border: 1px solid var(--divider);
      border-radius: var(--radius-sm);
      background: repeating-conic-gradient(var(--hover) 0 25%, transparent 0 50%) 0 0 / 16px 16px;
    }
    .none {
      padding: 24px;
      border: 1px dashed var(--divider);
      border-radius: var(--radius-sm);
      text-align: center;
    }
  `,
})
export class DiffView {
  readonly diff = input.required<Diff>();
  readonly actions = input<HunkAction[]>([]);
  readonly disabled = input(false);
  readonly hunk = output<HunkEvent>();

  protected readonly oldUrl = computed(() => pictureUrl(this.diff().oldPicture));
  protected readonly newUrl = computed(() => pictureUrl(this.diff().newPicture));
}
