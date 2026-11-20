import { ChangeDetectionStrategy, Component, computed, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { TPipe } from '../../i18n/i18n';
import { DialogRef } from '../../ui/dialog';

export interface ApproveData {
  /** The name the workspace asked for, if one request is pending. */
  name: string;
}

export interface ApproveResult {
  action: 'approve' | 'reject';
  code: string;
  name: string;
  totp: string;
}

/** `k7qf 2m9d` → `K7QF-2M9D`; anything else as typed, upper case. */
export function normalizePairCode(code: string): string {
  const bare = code.replace(/[\s-]/g, '').toUpperCase();
  return /^[A-Z0-9]{8}$/.test(bare) ? `${bare.slice(0, 4)}-${bare.slice(4)}` : bare;
}

/**
 * Approving a workspace: the code `shed nexus join` printed on it, typed here
 * (the list of requests does not carry it, so only someone who sees the
 * workspace can approve it), the name to give it and a fresh TOTP code.
 */
@Component({
  selector: 'shed-approve-dialog',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [FormsModule, TPipe],
  template: `
    <form class="dialog-body" (ngSubmit)="submit('approve')">
      <h2>{{ 'devices.approveTitle' | t }}</h2>
      <p class="dim" style="margin: 0">{{ 'devices.approveBody' | t }}</p>
      <label class="field">
        <span>{{ 'devices.pairCode' | t }}</span>
        <input
          class="input mono code"
          name="code"
          autocomplete="off"
          autocapitalize="characters"
          spellcheck="false"
          placeholder="K7QF-2M9D"
          required
          autofocus
          [ngModel]="code()"
          (ngModelChange)="code.set($event)"
        />
      </label>
      <label class="field">
        <span>{{ 'devices.workspaceName' | t }}</span>
        <input class="input" name="name" autocomplete="off" required [ngModel]="name()" (ngModelChange)="name.set($event)" />
      </label>
      <label class="field">
        <span>{{ 'auth.totp' | t }}</span>
        <input
          class="input mono"
          name="totp"
          inputmode="numeric"
          autocomplete="one-time-code"
          maxlength="8"
          placeholder="123456"
          required
          [ngModel]="totp()"
          (ngModelChange)="totp.set($event)"
        />
      </label>
      <div class="dialog-actions">
        <button type="button" class="btn danger" [disabled]="!codeOk()" (click)="submit('reject')">
          {{ 'devices.reject' | t }}
        </button>
        <span class="grow"></span>
        <button type="button" class="btn ghost" (click)="ref.close(null)">{{ 'common.cancel' | t }}</button>
        <button type="submit" class="btn primary" [disabled]="!ok()">{{ 'devices.approve' | t }}</button>
      </div>
    </form>
  `,
  styles: `
    .code {
      font-size: 20px;
      letter-spacing: 0.08em;
      text-transform: uppercase;
    }
  `,
})
export class ApproveDialog {
  protected readonly ref = inject<DialogRef<ApproveData, ApproveResult>>(DialogRef);
  protected readonly code = signal('');
  protected readonly name = signal(this.ref.data.name);
  protected readonly totp = signal('');
  protected readonly codeOk = computed(() => this.code().replace(/[\s-]/g, '').length >= 6);
  protected readonly ok = computed(
    () => this.codeOk() && !!this.name().trim() && this.totp().replace(/\s/g, '').length >= 6,
  );

  protected submit(action: ApproveResult['action']): void {
    if (action === 'approve' ? !this.ok() : !this.codeOk()) return;
    this.ref.close({
      action,
      code: normalizePairCode(this.code()),
      name: this.name().trim(),
      totp: this.totp().replace(/\s/g, ''),
    });
  }
}
