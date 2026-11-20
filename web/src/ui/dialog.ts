/**
 * Dialogs: native `<dialog>`s in the top layer, one stack, opened from code.
 *
 *   if (await dialogs.confirm({ title: t('tab.close.title'), ok: t('tab.close'), danger: true })) …
 *   const name = await dialogs.prompt({ title: …, value: tab.title });
 *   const code = await dialogs.totp({ title: … });
 *   const picked = await dialogs.open(FolderPicker, { workspace });
 *
 * A component opened with `open` injects `DialogRef` for its data and to close.
 */

import {
  AfterViewInit,
  ChangeDetectionStrategy,
  Component,
  ElementRef,
  Injectable,
  Injector,
  Type,
  computed,
  inject,
  input,
  output,
  signal,
  viewChild,
} from '@angular/core';
import { NgComponentOutlet } from '@angular/common';
import { FormsModule } from '@angular/forms';
import { TPipe, t } from '../i18n/i18n';

export class DialogRef<D = unknown, R = unknown> {
  constructor(
    readonly data: D,
    private readonly done: (result: R | null) => void,
  ) {}

  close(result: R | null = null): void {
    this.done(result);
  }
}

interface Entry {
  id: number;
  component: Type<unknown>;
  injector: Injector;
  wide: boolean;
  ref: DialogRef<unknown, unknown>;
}

export interface ConfirmOptions {
  title: string;
  body?: string;
  ok?: string;
  cancel?: string;
  /** Destructive: the button is in the danger colour. */
  danger?: boolean;
}

export interface PromptOptions {
  title: string;
  body?: string;
  label?: string;
  value?: string;
  placeholder?: string;
  ok?: string;
  /** Lets the field be emptied (to reset a name). */
  allowEmpty?: boolean;
  multiline?: boolean;
}

export interface TotpOptions {
  title: string;
  body?: string;
  ok?: string;
  danger?: boolean;
}

export interface Choice<V> {
  label: string;
  value: V;
  danger?: boolean;
  primary?: boolean;
}

export interface ChoiceOptions<V> {
  title: string;
  body?: string;
  choices: Choice<V>[];
}

@Injectable({ providedIn: 'root' })
export class Dialogs {
  private readonly injector = inject(Injector);
  private next = 1;
  readonly stack = signal<Entry[]>([]);

  open<D, R>(component: Type<unknown>, data: D, opts: { wide?: boolean } = {}): Promise<R | null> {
    return new Promise<R | null>((resolve) => {
      const id = this.next++;
      const ref = new DialogRef<D, R>(data, (result) => {
        this.stack.update((s) => s.filter((e) => e.id !== id));
        resolve(result);
      });
      const injector = Injector.create({
        providers: [{ provide: DialogRef, useValue: ref }],
        parent: this.injector,
      });
      this.stack.update((s) => [
        ...s,
        { id, component, injector, wide: !!opts.wide, ref: ref as DialogRef<unknown, unknown> },
      ]);
    });
  }

  async confirm(o: ConfirmOptions): Promise<boolean> {
    const r = await this.open<SimpleData, boolean>(SimpleDialog, { kind: 'confirm', ...o });
    return r === true;
  }

  prompt(o: PromptOptions): Promise<string | null> {
    return this.open<SimpleData, string>(SimpleDialog, { kind: 'prompt', ...o });
  }

  totp(o: TotpOptions): Promise<string | null> {
    return this.open<SimpleData, string>(SimpleDialog, { kind: 'totp', ...o });
  }

  choose<V>(o: ChoiceOptions<V>): Promise<V | null> {
    return this.open<SimpleData, V>(SimpleDialog, { kind: 'choice', ...o } as SimpleData);
  }
}

// ------------------------------------------------------------------ frame

@Component({
  selector: 'shed-dialog-frame',
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <dialog #dlg class="dialog" [class.wide]="wide()" (cancel)="onCancel($event)" (click)="onBackdrop($event)">
      <ng-content />
    </dialog>
  `,
})
export class DialogFrame implements AfterViewInit {
  readonly wide = input(false);
  readonly dismiss = output<void>();
  private readonly dlg = viewChild.required<ElementRef<HTMLDialogElement>>('dlg');

  ngAfterViewInit(): void {
    const d = this.dlg().nativeElement;
    if (typeof d.showModal === 'function' && !d.open) d.showModal();
    else d.setAttribute('open', '');
  }

  protected onCancel(e: Event): void {
    e.preventDefault();
    this.dismiss.emit();
  }

  protected onBackdrop(e: MouseEvent): void {
    // A click on the dialog element itself (not its content) is on the backdrop.
    if (e.target === this.dlg().nativeElement) {
      const r = this.dlg().nativeElement.getBoundingClientRect();
      const inside = e.clientX >= r.left && e.clientX <= r.right && e.clientY >= r.top && e.clientY <= r.bottom;
      if (!inside) this.dismiss.emit();
    }
  }
}

/** Where dialogs show; once, in the app's shell. */
@Component({
  selector: 'shed-dialog-host',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [NgComponentOutlet, DialogFrame],
  template: `
    @for (e of dialogs.stack(); track e.id) {
      <shed-dialog-frame [wide]="e.wide" (dismiss)="e.ref.close(null)">
        <ng-container *ngComponentOutlet="e.component; injector: e.injector" />
      </shed-dialog-frame>
    }
  `,
})
export class DialogHost {
  protected readonly dialogs = inject(Dialogs);
}

// ------------------------------------------------------------------ simple dialogs

type SimpleData =
  | ({ kind: 'confirm' } & ConfirmOptions)
  | ({ kind: 'prompt' } & PromptOptions)
  | ({ kind: 'totp' } & TotpOptions)
  | ({ kind: 'choice' } & ChoiceOptions<unknown>);

@Component({
  selector: 'shed-simple-dialog',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [FormsModule, TPipe],
  template: `
    <form class="dialog-body" (submit)="$event.preventDefault(); submit()">
      <h2>{{ d.title }}</h2>
      @if (d.body) {
        <p class="dim" style="margin: 0; white-space: pre-line">{{ d.body }}</p>
      }
      @switch (d.kind) {
        @case ('prompt') {
          <label class="field">
            @if (label()) {
              <span>{{ label() }}</span>
            }
            @if (multiline()) {
              <textarea class="textarea" rows="5" name="value" [(ngModel)]="value" #field></textarea>
            } @else {
              <input class="input" name="value" [(ngModel)]="value" [placeholder]="placeholder()" autocomplete="off" #field />
            }
          </label>
        }
        @case ('totp') {
          <label class="field">
            <span>{{ 'auth.totp' | t }}</span>
            <input
              class="input mono"
              name="totp"
              [(ngModel)]="value"
              inputmode="numeric"
              autocomplete="one-time-code"
              maxlength="32"
              placeholder="123456"
              #field
            />
          </label>
        }
      }
      <div class="dialog-actions">
        @if (d.kind === 'choice') {
          <button type="button" class="btn ghost" (click)="ref.close(null)">{{ 'common.cancel' | t }}</button>
          @for (c of choices(); track $index) {
            <button
              type="button"
              class="btn"
              [class.danger]="c.danger"
              [class.solid]="c.danger && c.primary"
              [class.primary]="c.primary && !c.danger"
              (click)="ref.close(c.value)"
            >
              {{ c.label }}
            </button>
          }
        } @else {
          <button type="button" class="btn ghost" (click)="ref.close(null)">{{ cancelText() }}</button>
          <button
            type="submit"
            class="btn"
            [class.primary]="!danger()"
            [class.danger]="danger()"
            [class.solid]="danger()"
            [disabled]="!canSubmit()"
          >
            {{ okText() }}
          </button>
        }
      </div>
    </form>
  `,
})
export class SimpleDialog implements AfterViewInit {
  protected readonly ref = inject<DialogRef<SimpleData, unknown>>(DialogRef);
  protected readonly d = this.ref.data;
  protected value = this.d.kind === 'prompt' ? (this.d.value ?? '') : '';
  private readonly field = viewChild<ElementRef<HTMLInputElement>>('field');

  protected readonly label = computed(() => (this.d.kind === 'prompt' ? (this.d.label ?? '') : ''));
  protected readonly placeholder = computed(() => (this.d.kind === 'prompt' ? (this.d.placeholder ?? '') : ''));
  protected readonly multiline = computed(() => this.d.kind === 'prompt' && !!this.d.multiline);
  protected readonly choices = computed(() => (this.d.kind === 'choice' ? this.d.choices : []));
  protected readonly danger = computed(() => (this.d.kind === 'confirm' || this.d.kind === 'totp') && !!this.d.danger);
  protected readonly okText = computed(() => ('ok' in this.d && this.d.ok) || t('common.ok'));
  protected readonly cancelText = computed(() => (this.d.kind === 'confirm' && this.d.cancel) || t('common.cancel'));

  ngAfterViewInit(): void {
    const f = this.field()?.nativeElement;
    if (f) {
      f.focus();
      if (this.d.kind === 'prompt') f.select();
    }
  }

  protected canSubmit(): boolean {
    if (this.d.kind === 'prompt') return this.d.allowEmpty || this.value.trim() !== '';
    if (this.d.kind === 'totp') return this.value.replace(/\s/g, '').length >= 6;
    return true;
  }

  protected submit(): void {
    if (!this.canSubmit()) return;
    switch (this.d.kind) {
      case 'confirm':
        this.ref.close(true);
        break;
      case 'prompt':
        this.ref.close(this.d.multiline ? this.value : this.value.trim());
        break;
      case 'totp':
        this.ref.close(this.value.replace(/\s/g, ''));
        break;
      default:
        break;
    }
  }
}
