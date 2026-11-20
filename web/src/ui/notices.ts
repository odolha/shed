import { ChangeDetectionStrategy, Component, Injectable, inject, signal } from '@angular/core';
import { errorText } from '../api/errors';
import { Icon } from './icon';
import { TPipe } from '../i18n/i18n';

export interface Notice {
  id: number;
  text: string;
  kind: 'info' | 'error';
}

/** Short messages at the bottom of the screen: errors from calls, "Copied". */
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

  private push(text: string, kind: Notice['kind'], ms: number): void {
    const id = this.next++;
    this.list.update((l) => [...l.slice(-3), { id, text, kind }]);
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
        <div class="notice" [class.error]="n.kind === 'error'">
          <span class="grow">{{ n.text }}</span>
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
    .notice.error {
      border-color: color-mix(in srgb, var(--danger) 55%, var(--divider));
      color: var(--danger);
    }
  `,
})
export class NoticesHost {
  protected readonly notices = inject(Notices);
}
