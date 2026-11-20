/**
 * A file of the project, read only: text with line numbers, coloured by the
 * workspace's highlights; pictures; a word on binary or too large files. A
 * small text file can be changed whole (`WriteFile`).
 */

import {
  ChangeDetectionStrategy,
  Component,
  ElementRef,
  computed,
  effect,
  inject,
  input,
  output,
  signal,
  untracked,
  viewChild,
} from '@angular/core';
import { FormsModule } from '@angular/forms';
import type { FileContent } from '../../api/gen/api';
import { WorkspacesStore } from '../../api/workspaces.store';
import { errorText } from '../../api/errors';
import { TPipe, t } from '../../i18n/i18n';
import { Clipboard } from '../../ui/clipboard';
import { Dialogs } from '../../ui/dialog';
import { Icon } from '../../ui/icon';
import { Notices } from '../../ui/notices';
import { Tooltip } from '../../ui/tooltip';
import { pictureUrl } from '../shared/diff-view';
import { type Segment, fileSize, highlightLine, splitPath } from './highlight';

interface Line {
  n: number;
  segs: Segment[];
}

/** Files up to this size can be edited here. */
const EDITABLE = 256 * 1024;

@Component({
  selector: 'shed-file-view',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [Icon, TPipe, Tooltip, FormsModule],
  template: `
    <div class="head">
      @if (showBack()) {
        <button class="icon-btn small" (click)="back.emit()" [shedTip]="'common.back' | t">
          <shed-icon name="arrow-left" [size]="16" />
        </button>
      }
      <span class="name ellipsis" [title]="path()">
        <span class="dim">{{ parts()[0] }}</span>{{ parts()[1] }}
      </span>
      @if (file(); as f) {
        <span class="dim small nowrap">{{ size(f.size) }}</span>
        @if (f.lang) {
          <span class="dim small nowrap">{{ f.lang }}</span>
        }
      }
      <span class="grow"></span>
      @if (file()?.kind === 'text' && !editing()) {
        <button class="icon-btn small" [class.on]="wrap()" (click)="wrap.set(!wrap())" [shedTip]="'project.wrap' | t">
          <shed-icon name="view-list" [size]="15" />
        </button>
        <button class="icon-btn small" (click)="copyAll()" [shedTip]="'project.copyFile' | t">
          <shed-icon name="copy" [size]="15" />
        </button>
        @if (canEdit()) {
          <button class="btn small ghost" (click)="startEdit()">
            <shed-icon name="pencil" [size]="13" />{{ 'common.edit' | t }}
          </button>
        }
      }
      @if (file()?.kind === 'picture') {
        <button class="icon-btn small" [class.on]="actual()" (click)="actual.set(!actual())" [shedTip]="'project.actualSize' | t">
          <shed-icon name="resize" [size]="15" />
        </button>
      }
      <button class="icon-btn small" (click)="reload()" [shedTip]="'common.refresh' | t">
        <shed-icon name="refresh" [size]="15" />
      </button>
    </div>
    <div class="body" #body>
      @if (error(); as e) {
        <div class="error-box">{{ e }}</div>
      } @else if (!file()) {
        <div class="empty">{{ 'common.loading' | t }}</div>
      } @else {
        @let f = file()!;
        @switch (f.kind) {
          @case ('text') {
            @if (editing()) {
              <div class="edit">
                <textarea class="textarea mono" [(ngModel)]="draft" spellcheck="false" [attr.aria-label]="path()"></textarea>
                <div class="row">
                  <span class="grow"></span>
                  <button class="btn ghost" (click)="editing.set(false)" [disabled]="saving()">{{ 'common.cancel' | t }}</button>
                  <button class="btn primary" (click)="save()" [disabled]="saving()">{{ 'common.save' | t }}</button>
                </div>
              </div>
            } @else {
              <div class="code-view" [class.wrap]="wrap()">
                @for (l of lines(); track l.n) {
                  <div class="line" [attr.data-line]="l.n" [class.hit]="l.n === line()">
                    <span class="num">{{ l.n }}</span>
                    <span class="txt">@for (s of l.segs; track $index) {<span [class]="s.cls ?? ''">{{ s.text }}</span>}</span>
                  </div>
                }
              </div>
            }
          }
          @case ('picture') {
            @if (pic(); as u) {
              <div class="pic" [class.actual]="actual()">
                <img [src]="u" [alt]="parts()[1]" />
              </div>
            }
          }
          @case ('binary') {
            <div class="empty">{{ 'project.binary' | t: { size: size(f.size) } }}</div>
          }
          @case ('tooLarge') {
            <div class="empty">{{ 'project.tooLarge' | t: { size: size(f.size) } }}</div>
          }
        }
      }
    </div>
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      min-height: 0;
      min-width: 0;
    }
    .head {
      display: flex;
      align-items: center;
      gap: 6px;
      min-height: 37px;
      padding: 2px 6px 2px 10px;
      border-bottom: 1px solid var(--divider);
    }
    .name {
      font-family: var(--mono);
      font-size: 13px;
    }
    .nowrap {
      white-space: nowrap;
    }
    .body {
      flex: 1 1 auto;
      overflow: auto;
      min-height: 0;
    }
    .code-view {
      display: table;
      min-width: 100%;
      padding: 6px 0;
      font: 12.5px/1.55 var(--mono);
      tab-size: 4;
    }
    .line {
      display: table-row;
    }
    .line.hit {
      background: color-mix(in srgb, var(--accent) 16%, transparent);
    }
    .num {
      display: table-cell;
      padding: 0 12px 0 10px;
      text-align: right;
      color: var(--text-dim);
      opacity: 0.7;
      user-select: none;
      width: 1%;
      white-space: nowrap;
    }
    .txt {
      display: table-cell;
      padding-right: 16px;
      white-space: pre;
    }
    .wrap .txt {
      white-space: pre-wrap;
      word-break: break-word;
    }
    .pic {
      padding: 16px;
      text-align: center;
    }
    .pic img {
      max-width: 100%;
      max-height: calc(100dvh - 200px);
      background: repeating-conic-gradient(var(--hover) 0 25%, transparent 0 50%) 0 0 / 16px 16px;
      border: 1px solid var(--divider);
    }
    .pic.actual img {
      max-width: none;
      max-height: none;
    }
    .edit {
      display: flex;
      flex-direction: column;
      gap: 8px;
      height: 100%;
      padding: 8px;
    }
    .edit textarea {
      flex: 1 1 auto;
      min-height: 50dvh;
      font-size: 12.5px;
      white-space: pre;
    }
  `,
})
export class FileView {
  readonly workspace = input.required<string>();
  readonly tab = input.required<string>();
  readonly path = input.required<string>();
  /** A line to show (1-based), from a search hit. */
  readonly line = input<number | null>(null);
  /** Bumped when the file may have changed on disk. */
  readonly changed = input(0);
  readonly showBack = input(false);
  readonly back = output<void>();

  private readonly store = inject(WorkspacesStore);
  private readonly notices = inject(Notices);
  private readonly clipboard = inject(Clipboard);
  private readonly dialogs = inject(Dialogs);
  private readonly body = viewChild<ElementRef<HTMLElement>>('body');

  protected readonly file = signal<FileContent | null>(null);
  protected readonly error = signal<string | null>(null);
  protected readonly wrap = signal(false);
  protected readonly actual = signal(false);
  protected readonly editing = signal(false);
  protected readonly saving = signal(false);
  protected draft = '';
  private seq = 0;

  protected readonly parts = computed(() => splitPath(this.path()));
  protected readonly pic = computed(() => pictureUrl(this.file()?.picture));
  protected readonly canEdit = computed(() => {
    const f = this.file();
    return !!f && f.kind === 'text' && f.size <= EDITABLE && this.store.canChange(this.workspace());
  });

  protected readonly lines = computed<Line[]>(() => {
    const f = this.file();
    if (!f || f.kind !== 'text') return [];
    const texts = (f.text ?? '').split('\n');
    if (texts.length > 1 && texts[texts.length - 1] === '') texts.pop();
    return texts.map((text, i) => ({ n: i + 1, segs: highlightLine(text.replace(/\r$/, ''), f.highlights?.[i]) }));
  });

  constructor() {
    effect(() => {
      const ws = this.workspace();
      const tab = this.tab();
      const path = this.path();
      untracked(() => {
        this.file.set(null);
        this.editing.set(false);
        void this.load(ws, tab, path);
      });
    });

    effect(() => {
      if (this.changed() === 0) return;
      untracked(() => {
        if (!this.editing()) void this.load(this.workspace(), this.tab(), this.path(), true);
      });
    });

    // Scroll to the line asked for once it is drawn.
    effect(() => {
      const n = this.line();
      const lines = this.lines();
      if (!n || !lines.length) return;
      setTimeout(() => {
        const el = this.body()?.nativeElement.querySelector(`[data-line="${n}"]`);
        el?.scrollIntoView({ block: 'center' });
      });
    });
  }

  protected size(bytes: number): string {
    return fileSize(bytes);
  }

  protected reload(): void {
    void this.load(this.workspace(), this.tab(), this.path(), true);
  }

  protected copyAll(): void {
    const f = this.file();
    if (f) void this.clipboard.copy(f.text ?? '');
  }

  protected startEdit(): void {
    this.draft = this.file()?.text ?? '';
    this.editing.set(true);
  }

  protected async save(): Promise<void> {
    const ws = this.workspace();
    const tab = this.tab();
    const path = this.path();
    this.saving.set(true);
    const ok = await this.notices.run(
      this.store.call(ws, { type: 'WriteFile', content: { tab, path, text: this.draft } }, 'Done'),
    );
    this.saving.set(false);
    if (ok === undefined) return;
    this.editing.set(false);
    this.notices.info(t('project.saved'));
    await this.load(ws, tab, path, true);
  }

  private async load(ws: string, tab: string, path: string, quiet = false): Promise<void> {
    const seq = ++this.seq;
    if (!quiet) this.error.set(null);
    try {
      const f = await this.store.call(ws, { type: 'ReadFile', content: { tab, path } }, 'File');
      if (seq !== this.seq) return;
      this.file.set(f);
      this.error.set(null);
    } catch (e) {
      if (seq !== this.seq) return;
      this.error.set(errorText(e));
    }
  }

  /** Asks before leaving unsaved edits (the project tab calls it). */
  async mayLeave(): Promise<boolean> {
    if (!this.editing() || this.draft === this.file()?.text) return true;
    return this.dialogs.confirm({ title: t('project.unsaved'), ok: t('project.discardEdits'), danger: true });
  }
}
