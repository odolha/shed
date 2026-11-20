import { ChangeDetectionStrategy, Component, computed, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import type { PendingPairing } from '../../api/gen/api';
import { TPipe } from '../../i18n/i18n';
import { DialogRef } from '../../ui/dialog';

export interface ApproveResult {
  name: string;
  totp: string;
}

/** Approving a workspace's pairing: the name to give it and a fresh TOTP code. */
@Component({
  selector: 'shed-approve-dialog',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [FormsModule, TPipe],
  template: `
    <form class="dialog-body" (ngSubmit)="submit()">
      <h2>{{ 'devices.approveTitle' | t }}</h2>
      <p class="dim" style="margin: 0">{{ 'devices.approveBody' | t: { code: p.code } }}</p>
      <div class="code mono">{{ p.code }}</div>
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
          autofocus
          [ngModel]="totp()"
          (ngModelChange)="totp.set($event)"
        />
      </label>
      <div class="dialog-actions">
        <button type="button" class="btn ghost" (click)="ref.close(null)">{{ 'common.cancel' | t }}</button>
        <button type="submit" class="btn primary" [disabled]="!ok()">{{ 'devices.approve' | t }}</button>
      </div>
    </form>
  `,
  styles: `
    .code {
      align-self: center;
      font-size: 24px;
      letter-spacing: 0.08em;
      padding: 6px 14px;
      border-radius: var(--radius-sm);
      background: var(--bg);
      border: 1px solid var(--divider);
    }
  `,
})
export class ApproveDialog {
  protected readonly ref = inject<DialogRef<PendingPairing, ApproveResult>>(DialogRef);
  protected readonly p = this.ref.data;
  protected readonly name = signal(this.p.name);
  protected readonly totp = signal('');
  protected readonly ok = computed(() => !!this.name().trim() && this.totp().replace(/\s/g, '').length >= 6);

  protected submit(): void {
    if (this.ok()) this.ref.close({ name: this.name().trim(), totp: this.totp().replace(/\s/g, '') });
  }
}
