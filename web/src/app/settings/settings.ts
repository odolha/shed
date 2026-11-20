import { ChangeDetectionStrategy, Component, OnInit, computed, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { AuthService } from '../../api/auth';
import { errorText, isFailure } from '../../api/errors';
import type { NotificationKind, PushInfo } from '../../api/gen/api';
import { NexusHttp } from '../../api/http';
import { TPipe, t } from '../../i18n/i18n';
import { Icon } from '../../ui/icon';
import { Notices } from '../../ui/notices';
import { KINDS, currentSubscription, pushOn, pushSupport, subscribe, unsubscribe } from './push';

const MIN_PASSWORD = 10;
const WEB_VERSION = '0.1.0';

/** Notifications on this device, the account, and versions. */
@Component({
  selector: 'shed-settings-page',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [FormsModule, TPipe, Icon],
  template: `
    <div class="page col big-gap">
      <h1>{{ 'settings.title' | t }}</h1>

      <section class="card section" aria-labelledby="push-h">
        <h2 id="push-h"><shed-icon name="bell" [size]="18" />{{ 'push.title' | t }}</h2>
        <p class="dim">{{ 'push.intro' | t }}</p>
        @if (support() !== 'ok') {
          <p class="note">{{ 'push.support.' + support() | t }}</p>
        } @else if (info() === null) {
          @if (pushError(); as e) {
            <div class="error-box" role="alert">{{ e }}</div>
          } @else {
            <p class="dim">{{ 'common.loading' | t }}</p>
          }
        } @else if (!info()!.vapidKey) {
          <p class="note">{{ 'push.off' | t }}</p>
        } @else {
          <div class="row wrap">
            @if (subscribed()) {
              <span class="chip ok"><shed-icon name="check" [size]="12" />{{ 'push.on' | t }}</span>
            } @else if (denied()) {
              <span class="chip danger">{{ 'push.denied' | t }}</span>
            } @else {
              <span class="chip">{{ 'push.offHere' | t }}</span>
            }
            <span class="grow"></span>
            @if (subscribed()) {
              <button class="btn" [disabled]="busy()" (click)="disable()">
                <shed-icon name="bell-off" [size]="14" />{{ 'push.disable' | t }}
              </button>
            } @else {
              <button class="btn primary" [disabled]="busy()" (click)="enable()">
                <shed-icon name="bell" [size]="14" />{{ 'push.enable' | t }}
              </button>
            }
          </div>
          @if (pushError(); as e) {
            <div class="error-box" role="alert">{{ e }}</div>
          }
          <fieldset class="kinds" [disabled]="busy()">
            <legend>{{ 'push.which' | t }}</legend>
            @for (k of kinds; track k) {
              <label class="check">
                <input type="checkbox" [checked]="chosen().includes(k)" (change)="toggleKind(k, $any($event.target).checked)" />
                <span class="col tight">
                  <span>{{ 'push.kind.' + k | t }}</span>
                  <span class="dim small">{{ 'push.kindHint.' + k | t }}</span>
                </span>
              </label>
            }
          </fieldset>
        }
      </section>

      <section class="card section" aria-labelledby="acct-h">
        <h2 id="acct-h"><shed-icon name="key" [size]="18" />{{ 'settings.account' | t }}</h2>
        <div class="row wrap">
          <span class="grow">{{ 'settings.signedInAs' | t: { name: auth.username() } }}</span>
          <button class="btn" (click)="auth.signOut()"><shed-icon name="logout" [size]="14" />{{ 'nav.signOut' | t }}</button>
        </div>

        <form class="col pw" (ngSubmit)="changePassword()">
          <h3>{{ 'settings.changePassword' | t }}</h3>
          <input type="text" name="username" autocomplete="username" [value]="auth.username()" hidden />
          <label class="field">
            <span>{{ 'settings.oldPassword' | t }}</span>
            <input class="input" type="password" name="old" autocomplete="current-password" required
              [ngModel]="oldPw()" (ngModelChange)="oldPw.set($event)" />
          </label>
          <label class="field">
            <span>{{ 'settings.newPassword' | t }}</span>
            <input class="input" type="password" name="new" autocomplete="new-password" required
              [ngModel]="newPw()" (ngModelChange)="newPw.set($event)" />
            <small class="dim" [class.danger-text]="newPw() && newPw().length < min">{{ 'setup.passwordHint' | t: { n: min } }}</small>
          </label>
          <label class="field">
            <span>{{ 'settings.newPassword2' | t }}</span>
            <input class="input" type="password" name="new2" autocomplete="new-password" required
              [ngModel]="newPw2()" (ngModelChange)="newPw2.set($event)" />
            @if (newPw2() && newPw2() !== newPw()) {
              <small class="danger-text">{{ 'setup.mismatch' | t }}</small>
            }
          </label>
          <label class="field">
            <span>{{ 'auth.totp' | t }}</span>
            <input class="input mono" name="totp" inputmode="numeric" autocomplete="one-time-code" maxlength="8" placeholder="123456" required
              [ngModel]="pwTotp()" (ngModelChange)="pwTotp.set($event)" />
          </label>
          <p class="dim small hint">{{ 'settings.passwordNote' | t }}</p>
          @if (pwError(); as e) {
            <div class="error-box" role="alert">{{ e }}</div>
          }
          <div class="row">
            <span class="grow"></span>
            <button type="submit" class="btn primary" [disabled]="!canChange()">{{ 'settings.changePasswordGo' | t }}</button>
          </div>
        </form>
      </section>

      <section class="card section" aria-labelledby="about-h">
        <h2 id="about-h"><shed-icon name="shed" [size]="18" />{{ 'settings.about' | t }}</h2>
        <dl>
          <dt class="dim">{{ 'settings.webVersion' | t }}</dt>
          <dd class="mono">{{ webVersion }}</dd>
          <dt class="dim">{{ 'settings.nexusVersion' | t }}</dt>
          <dd class="mono">{{ auth.status()?.version || '—' }} (API {{ auth.status()?.api ?? '—' }})</dd>
        </dl>
      </section>
    </div>
  `,
  styles: `
    .page {
      max-width: 640px;
    }
    .big-gap {
      gap: 18px;
    }
    .section {
      display: flex;
      flex-direction: column;
      gap: 12px;
      padding: 16px;
    }
    h2 {
      display: flex;
      align-items: center;
      gap: 8px;
    }
    p {
      margin: 0;
    }
    .note {
      padding: 8px 12px;
      border-radius: var(--radius-sm);
      background: var(--hover);
    }
    .kinds {
      display: flex;
      flex-direction: column;
      gap: 10px;
      margin: 0;
      padding: 0;
      border: 0;
    }
    .kinds legend {
      padding: 0 0 6px;
      font-size: 12px;
      color: var(--text-dim);
    }
    .kinds .check {
      align-items: flex-start;
    }
    .tight {
      gap: 0;
    }
    .pw {
      padding-top: 12px;
      border-top: 1px solid var(--divider);
      gap: 12px;
    }
    dl {
      display: grid;
      grid-template-columns: auto 1fr;
      gap: 4px 16px;
      margin: 0;
    }
    dd {
      margin: 0;
    }
  `,
})
export class SettingsPage implements OnInit {
  private readonly http = inject(NexusHttp);
  protected readonly auth = inject(AuthService);
  private readonly notices = inject(Notices);

  protected readonly kinds = KINDS;
  protected readonly min = MIN_PASSWORD;
  protected readonly webVersion = WEB_VERSION;
  protected readonly support = signal(pushSupport());
  protected readonly info = signal<PushInfo | null>(null);
  protected readonly chosen = signal<NotificationKind[]>([]);
  protected readonly subscribed = signal(false);
  protected readonly denied = signal(typeof Notification !== 'undefined' && Notification.permission === 'denied');
  protected readonly busy = signal(false);
  protected readonly pushError = signal<string | null>(null);

  protected readonly oldPw = signal('');
  protected readonly newPw = signal('');
  protected readonly newPw2 = signal('');
  protected readonly pwTotp = signal('');
  protected readonly pwBusy = signal(false);
  protected readonly pwError = signal<string | null>(null);
  protected readonly canChange = computed(
    () =>
      !this.pwBusy() &&
      !!this.oldPw() &&
      this.newPw().length >= MIN_PASSWORD &&
      this.newPw() === this.newPw2() &&
      this.pwTotp().replace(/\s/g, '').length >= 6,
  );

  ngOnInit(): void {
    void this.auth.check(true).catch(() => undefined);
    if (this.support() === 'ok') void this.loadPush();
  }

  private async loadPush(): Promise<void> {
    try {
      const info = await this.http.push();
      this.info.set(info);
      this.chosen.set(info.prefs.kinds);
      const here = !!(await currentSubscription());
      this.subscribed.set(pushOn(!!info.subscribed, here, Notification.permission));
    } catch (e) {
      this.pushError.set(errorText(e));
    }
  }

  protected async enable(): Promise<void> {
    const key = this.info()?.vapidKey;
    if (!key) return;
    this.busy.set(true);
    this.pushError.set(null);
    try {
      const sub = await subscribe(key);
      await this.http.pushSubscribe(sub);
      if (!this.chosen().length) await this.savePrefs(KINDS.filter((k) => k !== 'commandDone'));
      this.subscribed.set(true);
      this.info.update((i) => (i ? { ...i, subscribed: true } : i));
      this.notices.info(t('push.enabled'));
    } catch (e) {
      if (e instanceof Error && e.message === 'denied') {
        this.denied.set(Notification.permission === 'denied');
        this.pushError.set(t('push.notAllowed'));
      } else this.pushError.set(errorText(e));
    } finally {
      this.busy.set(false);
    }
  }

  /** No more notifications here: the nexus forgets this browser's subscription, and the browser drops it. */
  protected async disable(): Promise<void> {
    this.busy.set(true);
    this.pushError.set(null);
    try {
      await this.http.pushUnsubscribe();
      this.subscribed.set(false);
      this.info.update((i) => (i ? { ...i, subscribed: false } : i));
      await unsubscribe().catch(() => undefined);
      this.notices.info(t('push.disabled'));
    } catch (e) {
      this.pushError.set(errorText(e));
    } finally {
      this.busy.set(false);
    }
  }

  protected async toggleKind(kind: NotificationKind, on: boolean): Promise<void> {
    const next = on ? [...new Set([...this.chosen(), kind])] : this.chosen().filter((k) => k !== kind);
    this.busy.set(true);
    try {
      await this.savePrefs(next);
    } catch (e) {
      this.notices.error(e);
      this.chosen.set([...this.chosen()]);
    } finally {
      this.busy.set(false);
    }
  }

  private async savePrefs(kinds: NotificationKind[]): Promise<void> {
    const ordered = KINDS.filter((k) => kinds.includes(k));
    await this.http.pushPrefs({ kinds: ordered });
    this.chosen.set(ordered);
  }

  protected async changePassword(): Promise<void> {
    if (!this.canChange()) return;
    this.pwBusy.set(true);
    this.pwError.set(null);
    try {
      await this.http.changePassword({ old: this.oldPw(), new: this.newPw(), totp: this.pwTotp().replace(/\s/g, '') });
      this.oldPw.set('');
      this.newPw.set('');
      this.newPw2.set('');
      this.pwTotp.set('');
      this.notices.info(t('settings.passwordChanged'));
    } catch (e) {
      this.pwError.set(
        isFailure(e, 'unauthorized') || isFailure(e, 'forbidden') ? t('settings.wrongPassword') : errorText(e),
      );
      this.pwTotp.set('');
    } finally {
      this.pwBusy.set(false);
    }
  }
}
