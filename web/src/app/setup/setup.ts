import { ChangeDetectionStrategy, Component, OnInit, computed, inject, input, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { Router } from '@angular/router';
import { AuthService } from '../../api/auth';
import { errorText, isFailure } from '../../api/errors';
import type { SetupTotp } from '../../api/gen/api';
import { NexusHttp, browserName } from '../../api/http';
import { TPipe, t } from '../../i18n/i18n';
import { Clipboard } from '../../ui/clipboard';
import { Icon } from '../../ui/icon';

const MIN_PASSWORD = 10;

/** The secret in groups of four, to type into an authenticator. */
export function groupSecret(secret: string): string {
  return secret.replace(/\s/g, '').replace(/(.{4})(?=.)/g, '$1 ');
}

/**
 * Setting up the account on a new nexus: the setup code from its log, a user
 * name and password; then an authenticator (TOTP); then recovery codes.
 */
@Component({
  selector: 'shed-setup-page',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [FormsModule, TPipe, Icon],
  template: `
    <div class="auth-page">
      <div class="brand">
        <img src="icons/shed.svg" alt="" width="56" height="56" />
        <h1>{{ 'setup.title' | t }}</h1>
      </div>
      <ol class="steps" [attr.aria-label]="'setup.steps' | t">
        @for (s of [1, 2, 3]; track s) {
          <li [class.on]="step() === s" [class.done]="step() > s" [attr.aria-current]="step() === s ? 'step' : null">
            <span class="num">
              @if (step() > s) {
                <shed-icon name="check" [size]="14" />
              } @else {
                {{ s }}
              }
            </span>
            <span class="label">{{ 'setup.step' + s | t }}</span>
          </li>
        }
      </ol>

      <div class="card form">
        @switch (step()) {
          @case (1) {
            <form class="col gap" (ngSubmit)="start()">
              <p class="dim intro">{{ 'setup.intro' | t }}</p>
              @if (!codeFromUrl()) {
                <label class="field">
                  <span>{{ 'setup.code' | t }}</span>
                  <input
                    class="input mono"
                    name="code"
                    autocomplete="off"
                    spellcheck="false"
                    autocapitalize="none"
                    required
                    [ngModel]="code()"
                    (ngModelChange)="code.set($event)"
                  />
                  <small class="dim">{{ 'setup.codeHint' | t }}</small>
                </label>
              }
              <label class="field">
                <span>{{ 'setup.username' | t }}</span>
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
                <span>{{ 'setup.password' | t }}</span>
                <input
                  class="input"
                  type="password"
                  name="password"
                  autocomplete="new-password"
                  required
                  [attr.minlength]="min"
                  [ngModel]="password()"
                  (ngModelChange)="password.set($event)"
                />
                <small class="dim" [class.danger-text]="password() && password().length < min">
                  {{ 'setup.passwordHint' | t: { n: min } }}
                </small>
              </label>
              <label class="field">
                <span>{{ 'setup.password2' | t }}</span>
                <input
                  class="input"
                  type="password"
                  name="password2"
                  autocomplete="new-password"
                  required
                  [ngModel]="password2()"
                  (ngModelChange)="password2.set($event)"
                />
                @if (password2() && password2() !== password()) {
                  <small class="danger-text">{{ 'setup.mismatch' | t }}</small>
                }
              </label>
              @if (error(); as e) {
                <div class="error-box" role="alert">{{ e }}</div>
              }
              <button type="submit" class="btn primary" [disabled]="!canStart()">
                {{ (busy() ? 'common.loading' : 'setup.next') | t }}
              </button>
            </form>
          }
          @case (2) {
            <form class="col gap" (ngSubmit)="confirm()">
              <p class="dim intro">{{ 'setup.totpIntro' | t }}</p>
              @if (qr(); as q) {
                <img class="qr" [src]="q" [alt]="'setup.qrAlt' | t" width="220" height="220" />
              }
              <div class="field">
                <span>{{ 'setup.secret' | t }}</span>
                <div class="row">
                  <code class="secret grow">{{ secretGrouped() }}</code>
                  <button type="button" class="btn small" (click)="copySecret()">
                    <shed-icon name="copy" [size]="14" />{{ 'common.copy' | t }}
                  </button>
                </div>
              </div>
              <label class="field">
                <span>{{ 'setup.totp' | t }}</span>
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
              @if (error(); as e) {
                <div class="error-box" role="alert">{{ e }}</div>
              }
              <div class="row">
                <button type="button" class="btn ghost" (click)="back()">{{ 'common.back' | t }}</button>
                <span class="grow"></span>
                <button type="submit" class="btn primary" [disabled]="!canConfirm()">
                  {{ (busy() ? 'common.loading' : 'setup.confirm') | t }}
                </button>
              </div>
            </form>
          }
          @case (3) {
            <div class="col gap">
              <p class="dim intro">{{ 'setup.recoveryIntro' | t }}</p>
              @if (recovery().length) {
                <ul class="codes" [attr.aria-label]="'setup.recoveryCodes' | t">
                  @for (c of recovery(); track c) {
                    <li>{{ c }}</li>
                  }
                </ul>
                <div class="row wrap">
                  <button type="button" class="btn" (click)="copyCodes()">
                    <shed-icon name="copy" [size]="14" />{{ 'setup.copyAll' | t }}
                  </button>
                  <a class="btn" [href]="downloadUrl()" download="shed-recovery-codes.txt">
                    <shed-icon name="arrow-down" [size]="14" />{{ 'setup.download' | t }}
                  </a>
                </div>
                <label class="check">
                  <input type="checkbox" [checked]="kept()" (change)="kept.set($any($event.target).checked)" />
                  {{ 'setup.kept' | t }}
                </label>
              } @else {
                <p class="dim">{{ 'setup.noCodes' | t }}</p>
              }
              <button type="button" class="btn primary" [disabled]="recovery().length > 0 && !kept()" (click)="finish()">
                {{ 'setup.continue' | t }}
              </button>
            </div>
          }
        }
      </div>
    </div>
  `,
  styles: `
    .auth-page {
      min-height: 100dvh;
      display: flex;
      flex-direction: column;
      align-items: center;
      justify-content: center;
      gap: 18px;
      padding: 24px 16px calc(24px + env(safe-area-inset-bottom));
    }
    .brand {
      display: flex;
      flex-direction: column;
      align-items: center;
      gap: 8px;
      text-align: center;
    }
    .steps {
      display: flex;
      gap: 6px;
      margin: 0;
      padding: 0;
      list-style: none;
      flex-wrap: wrap;
      justify-content: center;
    }
    .steps li {
      display: flex;
      align-items: center;
      gap: 6px;
      padding: 4px 10px 4px 4px;
      border-radius: 14px;
      color: var(--text-dim);
      font-size: 13px;
    }
    .steps li.on {
      background: color-mix(in srgb, var(--accent) 14%, transparent);
      color: var(--text);
    }
    .num {
      display: inline-flex;
      align-items: center;
      justify-content: center;
      width: 22px;
      height: 22px;
      border-radius: 50%;
      border: 1px solid var(--divider);
      font-size: 12px;
    }
    .on .num {
      background: var(--accent);
      border-color: var(--accent);
      color: var(--accent-ink);
    }
    .done .num {
      color: var(--ok);
      border-color: var(--ok);
    }
    .form {
      width: min(440px, 100%);
      padding: 20px;
    }
    .gap {
      gap: 14px;
    }
    .intro {
      margin: 0;
    }
    .qr {
      align-self: center;
      width: 220px;
      height: 220px;
      padding: 10px;
      border-radius: var(--radius);
      background: #fff;
    }
    .secret {
      padding: 6px 10px;
      border-radius: var(--radius-sm);
      background: var(--bg);
      border: 1px solid var(--divider);
      word-break: break-all;
      user-select: all;
    }
    .codes {
      display: grid;
      grid-template-columns: repeat(2, 1fr);
      gap: 6px 16px;
      margin: 0;
      padding: 12px 16px;
      list-style: none;
      border-radius: var(--radius-sm);
      background: var(--bg);
      border: 1px solid var(--divider);
      font-family: var(--mono);
      font-size: 14px;
      user-select: all;
    }
  `,
})
export class SetupPage implements OnInit {
  private readonly http = inject(NexusHttp);
  private readonly auth = inject(AuthService);
  private readonly router = inject(Router);
  private readonly clipboard = inject(Clipboard);

  /** `?code=` from the setup link the nexus logged (`#code=` is read too). */
  readonly codeParam = input<string | undefined>(undefined, { alias: 'code' });

  protected readonly min = MIN_PASSWORD;
  protected readonly step = signal<1 | 2 | 3>(1);
  protected readonly code = signal('');
  protected readonly codeFromUrl = signal(false);
  protected readonly username = signal('');
  protected readonly password = signal('');
  protected readonly password2 = signal('');
  protected readonly totp = signal('');
  protected readonly busy = signal(false);
  protected readonly error = signal<string | null>(null);
  protected readonly secret = signal<SetupTotp | null>(null);
  protected readonly qr = signal<string | null>(null);
  protected readonly recovery = signal<string[]>([]);
  protected readonly kept = signal(false);

  protected readonly secretGrouped = computed(() => groupSecret(this.secret()?.secret ?? ''));
  protected readonly canStart = computed(
    () =>
      !this.busy() &&
      !!this.code().trim() &&
      !!this.username().trim() &&
      this.password().length >= MIN_PASSWORD &&
      this.password() === this.password2(),
  );
  protected readonly canConfirm = computed(() => !this.busy() && this.totp().replace(/\s/g, '').length >= 6);
  protected readonly downloadUrl = computed(() => {
    const text = `${t('setup.fileHeader')}\n\n${this.recovery().join('\n')}\n`;
    return URL.createObjectURL(new Blob([text], { type: 'text/plain' }));
  });

  ngOnInit(): void {
    // The setup link carries the code in its fragment (`#code=…`, never sent
    // to a server or kept in logs), or in the query (`?code=…`).
    const fragment = new URLSearchParams(location.hash.replace(/^#/, '')).get('code')?.trim();
    if (fragment) history.replaceState(history.state, '', location.pathname + location.search);
    const c = fragment || this.codeParam()?.trim();
    if (c) {
      this.code.set(c);
      this.codeFromUrl.set(true);
    }
  }

  protected async start(): Promise<void> {
    if (!this.canStart()) return;
    this.busy.set(true);
    this.error.set(null);
    try {
      const totp = await this.http.setup({
        code: this.code().trim(),
        username: this.username().trim(),
        password: this.password(),
      });
      this.secret.set(totp);
      this.qr.set(await qrDataUrl(totp.url));
      this.step.set(2);
    } catch (e) {
      // A wrong setup code: let it be typed again.
      if (isFailure(e, 'unauthorized') || isFailure(e, 'wrongCode') || isFailure(e, 'forbidden')) {
        this.codeFromUrl.set(false);
        this.error.set(t('setup.badCode'));
      } else this.error.set(errorText(e));
    } finally {
      this.busy.set(false);
    }
  }

  protected back(): void {
    this.error.set(null);
    this.totp.set('');
    this.step.set(1);
  }

  protected async confirm(): Promise<void> {
    if (!this.canConfirm()) return;
    this.busy.set(true);
    this.error.set(null);
    try {
      const signed = await this.http.setupConfirm({
        code: this.code().trim(),
        totp: this.totp().replace(/\s/g, ''),
        device: { kind: 'browser', name: browserName() },
      });
      this.auth.signedIn(signed);
      this.recovery.set(signed.recoveryCodes ?? []);
      this.step.set(3);
    } catch (e) {
      this.error.set(isFailure(e, 'wrongCode') ? t('setup.wrongTotp') : errorText(e));
      this.totp.set('');
    } finally {
      this.busy.set(false);
    }
  }

  protected copySecret(): void {
    void this.clipboard.copy(this.secret()?.secret ?? '');
  }

  protected copyCodes(): void {
    void this.clipboard.copy(this.recovery().join('\n'));
  }

  protected finish(): void {
    void this.router.navigateByUrl('/');
  }
}

/** The `otpauth://` URL as a QR code, as an SVG data URL (drawn here, never sent anywhere). */
async function qrDataUrl(url: string): Promise<string | null> {
  try {
    const QR = await import('qrcode');
    const svg = await QR.toString(url, {
      type: 'svg',
      margin: 0,
      errorCorrectionLevel: 'M',
      color: { dark: '#000000', light: '#ffffff' },
    });
    return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
  } catch {
    return null;
  }
}
