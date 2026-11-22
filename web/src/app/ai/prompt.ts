import {
  ChangeDetectionStrategy,
  Component,
  ElementRef,
  OnDestroy,
  computed,
  effect,
  inject,
  input,
  model,
  signal,
  untracked,
  viewChild,
} from '@angular/core';
import { MAX_UPLOAD, type AiCommand, type AiInfo, type AiTab, type Picture } from '../../api/gen/api';
import { WorkspacesStore } from '../../api/workspaces.store';
import { TPipe, t } from '../../i18n/i18n';
import { Icon } from '../../ui/icon';
import { Notices } from '../../ui/notices';
import { Tooltip } from '../../ui/tooltip';
import { dataUrl, readBase64, readPicture } from './pictures';
import { withAttachedFiles } from './runs';

interface Attached {
  id: number;
  name: string;
  picture: Picture;
  url: string;
}

/** A file kept on the workspace for the prompt (`AiUpload`): its path once there. */
interface AttachedFile {
  id: number;
  name: string;
  path: string | null;
}

/** Pictures Claude reads as pictures; other files go as files. */
const PICTURES = new Set(['image/png', 'image/jpeg', 'image/gif', 'image/webp']);

/** The prompt with a draft merged into it: the draft at the end, after a
 * blank line (alone, in a prompt with nothing in it), as the desktop does. */
export function mergedDraft(prompt: string, draft: string): string {
  const kept = prompt.trimEnd();
  return kept.trim() ? `${kept}\n\n${draft}` : draft;
}

export function isPicture(file: { type: string }): boolean {
  return PICTURES.has(file.type);
}

/** The commands matching what is typed after `/`, best first. */
export function matchCommands(commands: AiCommand[], text: string): AiCommand[] {
  if (!text.startsWith('/') || /\s/.test(text)) return [];
  const q = text.slice(1).toLowerCase();
  const name = (c: AiCommand) => c.name.replace(/^\//, '').toLowerCase();
  const starts = commands.filter((c) => name(c).startsWith(q));
  const contains = commands.filter((c) => !name(c).startsWith(q) && name(c).includes(q));
  return [...starts, ...contains].slice(0, 12);
}

/** The prompt: text, pictures, `/` commands; Send (queued while Claude works), Draft, Stop. */
@Component({
  selector: 'shed-ai-prompt',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [TPipe, Icon, Tooltip],
  host: {
    '(dragover)': 'onDragOver($event)',
    '(dragleave)': 'dragging.set(false)',
    '(drop)': 'onDrop($event)',
    '[class.dragging]': 'dragging()',
  },
  template: `
    @if (editing() !== null) {
      <div class="editing small">
        <shed-icon name="pencil" [size]="13" />
        <span class="grow">{{ 'ai.prompt.editing' | t }}</span>
        <button class="btn ghost small" (click)="cancelEdit()">{{ 'common.cancel' | t }}</button>
      </div>
    }
    @if (matches().length) {
      <ul class="complete menu-like" role="listbox" [attr.aria-label]="'ai.prompt.commands' | t">
        @for (c of matches(); track c.name; let i = $index) {
          <li>
            <button
              class="menu-item"
              role="option"
              [class.sel]="i === selected()"
              [attr.aria-selected]="i === selected()"
              (mousedown)="$event.preventDefault(); pick(c)"
            >
              <span class="mono">/{{ bare(c.name) }}</span>
              @if (c.hint) {
                <span class="dim small mono">{{ c.hint }}</span>
              }
              <span class="dim small ellipsis grow">{{ c.description }}</span>
            </button>
          </li>
        }
      </ul>
    }
    @if (files().length) {
      <div class="files">
        @for (f of files(); track f.id) {
          <span class="file" [class.pending]="f.path === null" [shedTip]="f.path ?? ('ai.prompt.uploading' | t: { name: f.name })">
            @if (f.path === null) {
              <span class="spinner" [attr.aria-label]="'ai.prompt.uploading' | t: { name: f.name }"></span>
            } @else {
              <shed-icon name="file" [size]="13" />
            }
            <span class="ellipsis">{{ f.name }}</span>
            <button class="x" (click)="unattachFile(f.id)" [attr.aria-label]="'ai.prompt.removeFile' | t: { name: f.name }">
              <shed-icon name="close" [size]="12" />
            </button>
          </span>
        }
      </div>
    }
    @if (attached().length) {
      <div class="pics">
        @for (a of attached(); track a.id) {
          <span class="pic">
            <img [src]="a.url" [alt]="a.name" />
            <button class="x" (click)="unattach(a.id)" [attr.aria-label]="'ai.prompt.removePicture' | t: { name: a.name }">
              <shed-icon name="close" [size]="12" />
            </button>
          </span>
        }
      </div>
    }
    <div class="box">
      <textarea
        #area
        class="area"
        rows="1"
        [value]="text()"
        (input)="onInput($event)"
        (keydown)="onKey($event)"
        (paste)="onPaste($event)"
        [placeholder]="(working() ? 'ai.prompt.placeholderQueue' : 'ai.prompt.placeholder') | t"
        [attr.aria-label]="'ai.prompt.label' | t"
        [disabled]="!canChange()"
        enterkeyhint="enter"
      ></textarea>
      <div class="buttons">
        <input #file type="file" multiple hidden (change)="onFiles($event)" />
        <input #camera type="file" accept="image/*" capture="environment" hidden (change)="onFiles($event)" />
        <button class="btn ghost" (click)="file.click()" [disabled]="!canChange()" [shedTip]="'ai.prompt.attachTip' | t">
          <shed-icon name="paperclip" />
          <span class="label">{{ 'ai.prompt.attach' | t }}</span>
        </button>
        @if (coarse) {
          <button class="icon-btn" (click)="camera.click()" [disabled]="!canChange()" [shedTip]="'ai.prompt.camera' | t">
            <shed-icon name="camera" [size]="18" />
          </button>
        }
        <span class="grow"></span>
        @if (editing() === null) {
          <button class="btn ghost" (click)="draft()" [disabled]="!canChange() || !text().trim()" [shedTip]="'ai.prompt.draftTip' | t">
            <shed-icon name="pencil" />
            <span class="label">{{ 'ai.prompt.draft' | t }}</span>
          </button>
        }
        @if (working() && !text().trim() && !attached().length && !files().length) {
          <button class="btn" (click)="stop()" [disabled]="!canChange()">
            <shed-icon name="stop" />{{ 'ai.prompt.stop' | t }}
          </button>
        } @else {
          <button class="btn primary" (click)="send()" [disabled]="!canSend()">
            <shed-icon [name]="working() && editing() === null ? 'queue' : 'send'" />
            {{ (working() && editing() === null ? 'ai.prompt.queue' : 'common.send') | t }}
          </button>
        }
      </div>
    </div>
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      gap: 6px;
      position: relative;
    }
    :host(.dragging) .box {
      border-color: var(--accent);
      box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent) 40%, transparent);
    }
    .box {
      border: 1px solid var(--divider);
      border-radius: 10px;
      background: var(--surface);
      padding: 6px;
    }
    .box:focus-within {
      border-color: color-mix(in srgb, var(--accent) 70%, var(--divider));
    }
    .area {
      display: block;
      width: 100%;
      min-height: 40px;
      max-height: 40vh;
      padding: 6px 8px;
      border: 0;
      outline: none;
      resize: none;
      background: transparent;
      color: var(--text);
      font: inherit;
      line-height: 1.45;
    }
    .buttons {
      display: flex;
      align-items: center;
      gap: 4px;
    }
    .editing {
      display: flex;
      align-items: center;
      gap: 6px;
      padding: 2px 4px 2px 10px;
      border-radius: var(--radius-sm);
      background: color-mix(in srgb, var(--accent) 12%, transparent);
      color: var(--accent);
    }
    .complete {
      list-style: none;
      margin: 0;
      padding: 4px;
      max-height: 260px;
      overflow: auto;
      border: 1px solid var(--divider);
      border-radius: var(--radius);
      background: var(--raised);
      box-shadow: var(--shadow);
    }
    .complete .sel {
      background: var(--hover);
    }
    .files {
      display: flex;
      flex-wrap: wrap;
      gap: 6px;
    }
    .file {
      display: inline-flex;
      align-items: center;
      gap: 6px;
      max-width: 260px;
      height: 26px;
      padding: 0 2px 0 8px;
      border: 1px solid var(--divider);
      border-radius: 13px;
      background: var(--surface);
      font-size: 12.5px;
    }
    .file.pending {
      color: var(--text-dim);
    }
    .file .x {
      width: 20px;
      height: 20px;
      flex: none;
      display: inline-flex;
      align-items: center;
      justify-content: center;
      border: 0;
      border-radius: 50%;
      background: none;
      color: var(--text-dim);
      cursor: pointer;
    }
    .file .x:hover {
      background: var(--hover);
      color: var(--text);
    }
    .spinner {
      width: 10px;
      height: 10px;
      flex: none;
      border: 2px solid var(--divider);
      border-top-color: var(--accent);
      border-radius: 50%;
      animation: spin 0.8s linear infinite;
    }
    @keyframes spin {
      to {
        transform: rotate(360deg);
      }
    }
    .pics {
      display: flex;
      flex-wrap: wrap;
      gap: 6px;
    }
    .pic {
      position: relative;
      width: 64px;
      height: 48px;
      border-radius: var(--radius-sm);
      overflow: hidden;
      border: 1px solid var(--divider);
    }
    .pic img {
      width: 100%;
      height: 100%;
      object-fit: cover;
    }
    .pic .x {
      position: absolute;
      top: 2px;
      right: 2px;
      width: 20px;
      height: 20px;
      display: inline-flex;
      align-items: center;
      justify-content: center;
      border: 0;
      border-radius: 50%;
      background: rgb(0 0 0 / 60%);
      color: #fff;
      cursor: pointer;
    }
    @media (max-width: 480px) {
      .label {
        display: none;
      }
    }
  `,
})
export class AiPrompt implements OnDestroy {
  readonly workspace = input.required<string>();
  readonly tab = input.required<string>();
  readonly ai = input.required<AiTab>();
  readonly info = input<AiInfo | null>(null);
  readonly working = input(false);
  readonly canChange = input(false);
  /** The entry index of a message being edited, or null. */
  readonly editing = model<number | null>(null);

  private readonly store = inject(WorkspacesStore);
  private readonly notices = inject(Notices);
  private readonly area = viewChild.required<ElementRef<HTMLTextAreaElement>>('area');

  protected readonly text = signal('');
  protected readonly attached = signal<Attached[]>([]);
  protected readonly files = signal<AttachedFile[]>([]);
  protected readonly dragging = signal(false);
  protected readonly selected = signal(0);
  protected readonly coarse = typeof matchMedia === 'function' && matchMedia('(pointer: coarse)').matches;
  private nextId = 1;
  private unsentTimer: ReturnType<typeof setTimeout> | null = null;
  private loadedFor: string | null = null;
  private readonly dismissed = signal('');

  protected readonly matches = computed(() => {
    const text = this.text();
    if (text === this.dismissed()) return [];
    return matchCommands(this.info()?.commands ?? [], text);
  });

  /** Files still on their way to the workspace: sending waits for them. */
  protected readonly uploading = computed(() => this.files().some((f) => f.path === null));

  protected readonly canSend = computed(
    () =>
      this.canChange() &&
      !this.uploading() &&
      (this.text().trim() !== '' || this.attached().length > 0 || this.files().length > 0),
  );

  constructor() {
    // What was written and not sent comes back with the tab.
    effect(() => {
      const tab = this.tab();
      const unsent = this.ai().unsent ?? '';
      untracked(() => {
        if (this.loadedFor !== tab) {
          this.loadedFor = tab;
          this.setText(unsent);
        }
      });
    });
  }

  ngOnDestroy(): void {
    if (this.unsentTimer) {
      clearTimeout(this.unsentTimer);
      this.saveUnsent();
    }
  }

  /** Puts text in the prompt (an edit, a draft back), focused. */
  fill(text: string): void {
    this.setText(text);
    this.scheduleUnsent();
    queueMicrotask(() => {
      const el = this.area().nativeElement;
      el.focus();
      el.setSelectionRange(el.value.length, el.value.length);
    });
  }

  /** Adds a draft's text at the end of the prompt, focused. */
  merge(text: string): void {
    this.fill(mergedDraft(this.text(), text));
  }

  focus(): void {
    this.area().nativeElement.focus();
  }

  protected bare(name: string): string {
    return name.replace(/^\//, '');
  }

  private setText(text: string): void {
    this.text.set(text);
    queueMicrotask(() => this.autosize());
  }

  private autosize(): void {
    const el = this.area()?.nativeElement;
    if (!el) return;
    if (el.value !== this.text()) el.value = this.text();
    el.style.height = 'auto';
    el.style.height = `${el.scrollHeight}px`;
  }

  protected onInput(e: Event): void {
    this.text.set((e.target as HTMLTextAreaElement).value);
    this.selected.set(0);
    this.autosize();
    this.scheduleUnsent();
  }

  private scheduleUnsent(): void {
    if (this.unsentTimer) clearTimeout(this.unsentTimer);
    this.unsentTimer = setTimeout(() => this.saveUnsent(), 1000);
  }

  private saveUnsent(): void {
    this.unsentTimer = null;
    if (!this.canChange() || this.editing() !== null) return;
    const text = this.text();
    if (text === (this.ai().unsent ?? '')) return;
    this.store
      .request(this.workspace(), { type: 'AiSetUnsent', content: { tab: this.tab(), text } })
      .catch(() => undefined);
  }

  protected onKey(e: KeyboardEvent): void {
    const m = this.matches();
    if (m.length) {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        const n = m.length;
        this.selected.update((i) => (e.key === 'ArrowDown' ? (i + 1) % n : (i - 1 + n) % n));
        return;
      }
      if (e.key === 'Tab' || (e.key === 'Enter' && !e.shiftKey)) {
        e.preventDefault();
        const c = m[this.selected()] ?? m[0];
        if (c) this.pick(c);
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        this.dismissed.set(this.text());
        this.selected.set(0);
        return;
      }
    }
    if (e.key === 'Escape' && this.editing() !== null) {
      e.preventDefault();
      this.cancelEdit();
      return;
    }
    if (e.key === 'Enter' && !e.isComposing) {
      // Desktop: Enter sends, Shift+Enter breaks the line. Ctrl/Cmd+Enter sends
      // everywhere. Phones: Enter breaks the line; the button sends.
      const send = e.ctrlKey || e.metaKey || (!this.coarse && !e.shiftKey && !e.altKey);
      if (send) {
        e.preventDefault();
        void this.send();
      }
    }
  }

  protected pick(c: AiCommand): void {
    const text = `/${this.bare(c.name)} `;
    this.dismissed.set('');
    this.setText(text);
    this.selected.set(0);
    this.focus();
  }

  protected cancelEdit(): void {
    this.editing.set(null);
    this.setText('');
  }

  protected onPaste(e: ClipboardEvent): void {
    const files = Array.from(e.clipboardData?.files ?? []);
    if (!files.length) return;
    e.preventDefault();
    void this.attach(files);
  }

  protected onDragOver(e: DragEvent): void {
    if (!e.dataTransfer?.types.includes('Files')) return;
    e.preventDefault();
    this.dragging.set(true);
  }

  protected onDrop(e: DragEvent): void {
    this.dragging.set(false);
    const files = Array.from(e.dataTransfer?.files ?? []);
    if (!files.length) return;
    e.preventDefault();
    void this.attach(files);
  }

  protected onFiles(e: Event): void {
    const input = e.target as HTMLInputElement;
    const files = Array.from(input.files ?? []);
    input.value = '';
    void this.attach(files);
  }

  /** Pictures as pictures; other files kept on the workspace, by their path. */
  private async attach(files: File[]): Promise<void> {
    if (!this.canChange()) return;
    for (const f of files) {
      if (!isPicture(f)) {
        void this.upload(f);
        continue;
      }
      if (f.size > 20_000_000) {
        this.notices.info(t('ai.prompt.tooLarge', { name: f.name }));
        continue;
      }
      try {
        const picture = await readPicture(f);
        this.attached.update((a) => [...a, { id: this.nextId++, name: f.name || 'picture', picture, url: dataUrl(picture) }]);
      } catch (e) {
        this.notices.error(e);
      }
    }
  }

  protected unattach(id: number): void {
    this.attached.update((a) => a.filter((x) => x.id !== id));
  }

  private async upload(f: File): Promise<void> {
    const name = f.name || 'file';
    // Refused before it is read: the workspace keeps no more than this.
    if (f.size > MAX_UPLOAD) {
      this.notices.info(t('ai.prompt.fileTooLarge', { name, max: `${Math.round(MAX_UPLOAD / (1024 * 1024))} MB` }));
      return;
    }
    const id = this.nextId++;
    this.files.update((a) => [...a, { id, name, path: null }]);
    try {
      const data = await readBase64(f);
      const path = await this.store.call(
        this.workspace(),
        { type: 'AiUpload', content: { tab: this.tab(), name, data } },
        'Text',
        120_000,
      );
      // Taken out while it went: left out.
      this.files.update((a) => a.map((x) => (x.id === id ? { ...x, path } : x)));
    } catch (e) {
      this.files.update((a) => a.filter((x) => x.id !== id));
      this.notices.error(e);
    }
  }

  protected unattachFile(id: number): void {
    this.files.update((a) => a.filter((x) => x.id !== id));
  }

  async send(): Promise<void> {
    if (this.uploading()) {
      this.notices.info(t('ai.prompt.waitUploads'));
      return;
    }
    if (!this.canSend()) return;
    const typed = this.text().trim();
    const attached = this.attached();
    const files = this.files();
    // Files go as their paths on the workspace, for Claude to read.
    const text = withAttachedFiles(typed, files.flatMap((f) => (f.path ? [f.path] : [])));
    const pictures = attached.map((a) => a.picture);
    const editing = this.editing();
    const work =
      editing !== null
        ? this.store.request(this.workspace(), { type: 'AiEdit', content: { tab: this.tab(), entry: editing, text, pictures } })
        : this.store.request(this.workspace(), { type: 'AiSend', content: { tab: this.tab(), text, pictures } }, 60_000);
    // Cleared at once; put back if it did not go.
    this.setText('');
    this.attached.set([]);
    this.files.set([]);
    this.editing.set(null);
    const r = await this.notices.run(work);
    if (r === undefined) {
      this.setText(typed);
      this.attached.set(attached);
      this.files.set(files);
      return;
    }
    if (this.unsentTimer) clearTimeout(this.unsentTimer);
    if (this.ai().unsent) this.saveUnsent();
  }

  protected async draft(): Promise<void> {
    const text = this.text().trim();
    if (!text) return;
    this.setText('');
    const r = await this.notices.run(
      this.store.request(this.workspace(), { type: 'AiAddDraft', content: { tab: this.tab(), text, session: null } }),
    );
    if (r === undefined) this.setText(text);
    else {
      this.notices.info(t('ai.prompt.drafted'));
      this.saveUnsent();
    }
  }

  protected stop(): void {
    void this.notices.run(this.store.request(this.workspace(), { type: 'AiStop', content: { tab: this.tab() } }));
  }
}
