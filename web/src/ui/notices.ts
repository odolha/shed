import { ChangeDetectionStrategy, Component, Injectable, inject, signal } from '@angular/core';
import { Router } from '@angular/router';
import { errorText } from '../api/errors';
import { Icon } from './icon';
import { TPipe } from '../i18n/i18n';

export interface Notice {
  id: number;
  text: string;
  kind: 'info' | 'error' | 'notification';
  /** A second line, for notifications. */
  detail?: string;
  /** Where clicking it leads (an app route). */
  link?: string;
}

/**
 * Short messages at the bottom of the screen: errors from calls, "Copied",
 * and notifications from the nexus while the page is in view.
 */
@Injectable({ providedIn: 'root' })
export class Notices {
  readonly list = signal<Notice[]>([]);
  private next = 1;

  info(text: string): void {
    this.push(text, 'info', 3000);
  }

  error(e: unknown): void {
    this.push(errorText(e), 'error', 7000);
  }

  /** A notification: stays longer, and opens `link` when clicked. */
  notification(title: string, detail: string, link: string): void {
    this.push(title, 'notification', 12000, { detail, link });
  }

  dismiss(id: number): void {
    this.list.update((l) => l.filter((n) => n.id !== id));
  }

  /** Runs a call, telling the user if it fails; resolves to its result or undefined. */
  async run<T>(work: Promise<T>): Promise<T | undefined> {
    try {
      return await work;
    } catch (e) {
      this.error(e);
      return undefined;
    }
  }

  private push(text: string, kind: Notice['kind'], ms: number, extra: Partial<Notice> = {}): void {
    const id = this.next++;
    this.list.update((l) => [...l.slice(-3), { id, text, kind, ...extra }]);
    setTimeout(() => this.dismiss(id), ms);
  }
}

@Component({
  selector: 'shed-notices',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [Icon, TPipe],
  template: `
    <div class="stack" role="status" aria-live="polite">
      @for (n of notices.list(); track n.id) {
        <div class="notice" [class.error]="n.kind === 'error'" [class.notification]="n.kind === 'notification'">
          @if (n.link) {
            <shed-icon name="bell" [size]="16" />
            <button class="grow open" (click)="open(n)">
              <span class="title">{{ n.text }}</span>
              @if (n.detail) {
                <span class="detail dim">{{ n.detail }}</span>
              }
            </button>
            <button class="btn small ghost" (click)="open(n)">{{ 'common.open' | t }}</button>
          } @else {
            <span class="grow">{{ n.text }}</span>
          }
          <button class="icon-btn small" (click)="notices.dismiss(n.id)" [attr.aria-label]="'common.dismiss' | t">
            <shed-icon name="close" [size]="14" />
          </button>
        </div>
      }
    </div>
  `,
  styles: `
    .stack {
      position: fixed;
      left: 50%;
      bottom: calc(16px + env(safe-area-inset-bottom));
      transform: translateX(-50%);
      z-index: 50;
      display: flex;
      flex-direction: column;
      gap: 8px;
      width: min(480px, calc(100vw - 24px));
      pointer-events: none;
    }
    .notice {
      display: flex;
      align-items: center;
      gap: 8px;
      padding: 8px 8px 8px 14px;
      border: 1px solid var(--divider);
      border-radius: var(--radius);
      background: var(--raised);
      box-shadow: var(--shadow);
      pointer-events: auto;
    }
    .notice.notification {
      border-color: color-mix(in srgb, var(--accent) 55%, var(--divider));
    }
    .notice.notification > shed-icon {
      color: var(--accent);
      flex: none;
    }
    .open {
      display: flex;
      flex-direction: column;
      align-items: flex-start;
      gap: 2px;
      min-width: 0;
      padding: 0;
      border: 0;
      background: none;
      color: inherit;
      font: inherit;
      text-align: left;
      cursor: pointer;
    }
    .open .title {
      font-weight: 600;
    }
    .open .detail {
      font-size: 13px;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
      max-width: 100%;
    }
    .notice.error {
      border-color: color-mix(in srgb, var(--danger) 55%, var(--divider));
      color: var(--danger);
    }
  `,
})
export class NoticesHost {
  protected readonly notices = inject(Notices);
  private readonly router = inject(Router);

  protected open(n: Notice): void {
    this.notices.dismiss(n.id);
    if (n.link) void this.router.navigateByUrl(n.link);
  }
}
