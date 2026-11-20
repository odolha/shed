/** Quick open: a file of the project by a few letters of its path. */

import { AfterViewInit, ChangeDetectionStrategy, Component, ElementRef, inject, signal, viewChild } from '@angular/core';
import { WorkspacesStore } from '../../api/workspaces.store';
import { errorText } from '../../api/errors';
import { TPipe } from '../../i18n/i18n';
import { DialogRef } from '../../ui/dialog';
import { Icon } from '../../ui/icon';
import { type Segment, positionMarks, segment } from './highlight';

export interface QuickOpenData {
  workspace: string;
  tab: string;
  /** Files open in Shed, shown before anything is typed. */
  recent: string[];
}

interface Item {
  path: string;
  segs: Segment[];
}

@Component({
  selector: 'shed-quick-open',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [TPipe, Icon],
  template: `
    <div class="dialog-body">
      <h2>{{ 'project.quickOpen' | t }}</h2>
      <input
        #field
        class="input mono"
        [value]="query()"
        (input)="onInput($any($event.target).value)"
        (keydown)="onKey($event)"
        [placeholder]="'project.quickOpen.placeholder' | t"
        [attr.aria-label]="'project.quickOpen' | t"
        role="combobox"
        aria-controls="quick-open-list"
        [attr.aria-activedescendant]="items().length ? 'qo-' + active() : null"
        autocomplete="off"
        spellcheck="false"
      />
      @if (error(); as e) {
        <div class="error-box small">{{ e }}</div>
      }
      <div class="list" id="quick-open-list" role="listbox">
        @if (!query() && items().length) {
          <div class="menu-label">{{ 'project.openInShed' | t }}</div>
        }
        @for (it of items(); track it.path; let i = $index) {
          <button
            class="menu-item"
            role="option"
            [id]="'qo-' + i"
            [attr.aria-selected]="i === active()"
            [class.active]="i === active()"
            (click)="ref.close(it.path)"
            (mousemove)="active.set(i)"
          >
            <shed-icon name="file" [size]="14" />
            <span class="mono ellipsis">@for (s of it.segs; track $index) {<span [class]="s.cls ?? ''">{{ s.text }}</span>}</span>
          </button>
        } @empty {
          @if (query() && !busy()) {
            <div class="empty small">{{ 'project.noMatch' | t }}</div>
          }
        }
      </div>
    </div>
  `,
  styles: `
    .list {
      max-height: min(55dvh, 420px);
      overflow: auto;
      margin: 0 -8px;
    }
    .menu-item.active {
      background: var(--hover);
    }
    .menu-item .mono {
      font-size: 13px;
    }
    .match {
      color: var(--accent);
      font-weight: 700;
    }
  `,
})
export class QuickOpen implements AfterViewInit {
  protected readonly ref = inject<DialogRef<QuickOpenData, string>>(DialogRef);
  private readonly store = inject(WorkspacesStore);
  private readonly field = viewChild.required<ElementRef<HTMLInputElement>>('field');

  protected readonly query = signal('');
  protected readonly items = signal<Item[]>(this.ref.data.recent.map((path) => ({ path, segs: [{ text: path, cls: null }] })));
  protected readonly active = signal(0);
  protected readonly busy = signal(false);
  protected readonly error = signal<string | null>(null);
  private timer: ReturnType<typeof setTimeout> | null = null;
  private seq = 0;

  ngAfterViewInit(): void {
    this.field().nativeElement.focus();
  }

  protected onInput(value: string): void {
    this.query.set(value);
    if (this.timer) clearTimeout(this.timer);
    this.timer = setTimeout(() => void this.search(value), 120);
  }

  protected onKey(e: KeyboardEvent): void {
    const n = this.items().length;
    if (e.key === 'ArrowDown' && n) {
      e.preventDefault();
      this.active.set((this.active() + 1) % n);
      this.scrollActive();
    } else if (e.key === 'ArrowUp' && n) {
      e.preventDefault();
      this.active.set((this.active() - 1 + n) % n);
      this.scrollActive();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const it = this.items()[this.active()];
      if (it) this.ref.close(it.path);
    }
  }

  private scrollActive(): void {
    queueMicrotask(() => document.getElementById(`qo-${this.active()}`)?.scrollIntoView({ block: 'nearest' }));
  }

  private async search(query: string): Promise<void> {
    const seq = ++this.seq;
    const { workspace, tab, recent } = this.ref.data;
    if (!query.trim()) {
      this.items.set(recent.map((path) => ({ path, segs: [{ text: path, cls: null }] })));
      this.active.set(0);
      this.error.set(null);
      return;
    }
    this.busy.set(true);
    try {
      const files = await this.store.call(workspace, { type: 'QuickOpen', content: { tab, query, limit: 50 } }, 'Files');
      if (seq !== this.seq) return;
      this.items.set(files.map((f) => ({ path: f.path, segs: segment(f.path, positionMarks(f.hits, 'match')) })));
      this.active.set(0);
      this.error.set(null);
    } catch (e) {
      if (seq === this.seq) this.error.set(errorText(e));
    } finally {
      if (seq === this.seq) this.busy.set(false);
    }
  }
}
