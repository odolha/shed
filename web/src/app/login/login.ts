import { ChangeDetectionStrategy, Component, computed, inject, input, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { Router } from '@angular/router';
import { AuthService } from '../../api/auth';
import { errorText, isFailure } from '../../api/errors';
import { NexusHttp, browserName } from '../../api/http';
import { TPipe, t } from '../../i18n/i18n';

/** Signing in: user name, password and a code from the authenticator. */
@Component({
  selector: 'shed-login-page',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [FormsModule, TPipe],
  template: `
    <div class="auth-page">
      <div class="brand">
        <img src="icons/shed.svg" alt="" width="56" height="56" />
        <h1>Shed</h1>
      </div>
      <form class="card form" (ngSubmit)="submit()" [attr.aria-busy]="busy()">
        <h2>{{ 'login.title' | t }}</h2>
        @if (expired()) {
          <p class="note" role="status">{{ 'login.expired' | t }}</p>
        }
        <label class="field">
          <span>{{ 'login.username' | t }}</span>
          <input
            class="input"
            name="username"
            autocomplete="username"
            autocapitalize="none"
            spellcheck="false"
            required
            [ngModel]="username()"
            (ngModelChange)="username.set($event)"
          />
        </label>
        <label class="field">
          <span>{{ 'login.password' | t }}</span>
          <input
            class="input"
            type="password"
            name="password"
            autocomplete="current-password"
            required
            [ngModel]="password()"
            (ngModelChange)="password.set($event)"
          />
        </label>
        <label class="field">
          <span>{{ (recovery() ? 'login.recoveryCode' : 'login.totp') | t }}</span>
          <input
            class="input mono"
            name="totp"
            required
            spellcheck="false"
            autocapitalize="none"
            [attr.inputmode]="recovery() ? 'text' : 'numeric'"
            [attr.autocomplete]="recovery() ? 'off' : 'one-time-code'"
            [attr.maxlength]="recovery() ? 64 : 8"
            [placeholder]="recovery() ? 'xxxx-xxxx' : '123456'"
            [ngModel]="totp()"
            (ngModelChange)="totp.set($event)"
          />
        </label>
        <button type="button" class="link-btn" (click)="toggleRecovery()">
          {{ (recovery() ? 'login.useTotp' : 'login.useRecovery') | t }}
        </button>
        @if (error(); as e) {
          <div class="error-box" role="alert">{{ e }}</div>
        }
        <button type="submit" class="btn primary" [disabled]="!canSubmit()">
          {{ (busy() ? 'login.signingIn' : 'login.signIn') | t }}
        </button>
      </form>
    </div>
  `,
  styles: `
    .auth-page {
      min-height: 100dvh;
      display: flex;
      flex-direction: column;
      align-items: center;
      justify-content: center;
      gap: 20px;
      padding: 24px 16px calc(24px + env(safe-area-inset-bottom));
    }
    .brand {
      display: flex;
      flex-direction: column;
      align-items: center;
      gap: 8px;
    }
    .form {
      width: min(400px, 100%);
      display: flex;
      flex-direction: column;
      gap: 14px;
      padding: 20px;
    }
    .note {
      margin: 0;
      padding: 8px 12px;
      border-radius: var(--radius-sm);
      background: color-mix(in srgb, var(--warn) 14%, transparent);
      color: var(--warn);
    }
    .link-btn {
      align-self: flex-start;
      padding: 2px 0;
      border: 0;
      background: none;
      color: var(--accent);
      font: inherit;
      font-size: 13px;
      cursor: pointer;
    }
  `,
})
export class LoginPage {
  private readonly http = inject(NexusHttp);
  private readonly auth = inject(AuthService);
  private readonly router = inject(Router);

  /** Query parameters (component input binding). */
  readonly returnUrl = input<string | undefined>();
  readonly expired = input<string | undefined>();

  protected readonly username = signal(this.auth.username());
  protected readonly password = signal('');
  protected readonly totp = signal('');
  protected readonly recovery = signal(false);
  protected readonly busy = signal(false);
  protected readonly error = signal<string | null>(null);

  protected readonly canSubmit = computed(
    () => !this.busy() && !!this.username().trim() && !!this.password() && this.totp().trim().length >= 6,
  );

  protected toggleRecovery(): void {
    this.recovery.update((r) => !r);
    this.totp.set('');
  }

  protected async submit(): Promise<void> {
    if (!this.canSubmit()) return;
    this.busy.set(true);
    this.error.set(null);
    try {
      const signed = await this.http.login({
        username: this.username().trim(),
        password: this.password(),
        totp: this.recovery() ? this.totp().trim() : this.totp().replace(/\s/g, ''),
        device: { kind: 'browser', name: browserName() },
      });
      this.auth.signedIn(signed);
      const back = this.returnUrl();
      await this.router.navigateByUrl(back && back.startsWith('/') && !back.startsWith('//') ? back : '/');
    } catch (e) {
      if (isFailure(e, 'unauthorized')) this.error.set(t('login.wrong'));
      else if (isFailure(e, 'wrongCode')) this.error.set(t('login.wrongCode'));
      else this.error.set(errorText(e));
      this.totp.set('');
    } finally {
      this.busy.set(false);
    }
  }
}
