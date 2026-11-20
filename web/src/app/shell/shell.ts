import { ChangeDetectionStrategy, Component, computed, inject } from '@angular/core';
import { RouterLink, RouterLinkActive, RouterOutlet } from '@angular/router';
import { AuthService } from '../../api/auth';
import { NexusSocket } from '../../api/socket';
import { TPipe, t } from '../../i18n/i18n';
import { Icon } from '../../ui/icon';
import { Menu, MenuTrigger } from '../../ui/menu';
import { Tooltip } from '../../ui/tooltip';

/** The signed-in app: a slim bar on top, the page below. */
@Component({
  selector: 'shed-shell',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [RouterOutlet, RouterLink, RouterLinkActive, Icon, TPipe, Tooltip, Menu, MenuTrigger],
  template: `
    <header class="bar">
      <a routerLink="/" class="brand" [attr.aria-label]="'nav.home' | t">
        <img src="icons/shed.svg" alt="" width="24" height="24" />
        <span>Shed</span>
      </a>
      @if (connection(); as c) {
        <span class="chip" [class.warn]="c.kind === 'warn'" [class.danger]="c.kind === 'danger'" role="status">
          {{ c.text }}
        </span>
        @if (c.retry) {
          <button class="btn small ghost" (click)="socket.connect()">{{ 'conn.retry' | t }}</button>
        }
      }
      <span class="grow"></span>
      <nav class="row" [attr.aria-label]="'nav.main' | t">
        <a class="icon-btn" routerLink="/devices" routerLinkActive="on" [shedTip]="'nav.devices' | t">
          <shed-icon name="device" [size]="18" />
        </a>
        <a class="icon-btn" routerLink="/settings" routerLinkActive="on" [shedTip]="'nav.settings' | t">
          <shed-icon name="settings" [size]="18" />
        </a>
        <button class="icon-btn" [shedMenu]="account" [shedTip]="auth.username() || ('nav.account' | t)">
          <shed-icon name="more" [size]="18" />
        </button>
      </nav>
      <shed-menu #account>
        @if (auth.username()) {
          <div class="menu-label">{{ auth.username() }}</div>
        }
        <button class="menu-item" (click)="auth.signOut()">
          <shed-icon name="logout" />{{ 'nav.signOut' | t }}
        </button>
      </shed-menu>
    </header>
    <main>
      <router-outlet />
    </main>
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      height: 100dvh;
    }
    .bar {
      display: flex;
      align-items: center;
      gap: 8px;
      height: 44px;
      flex: none;
      padding: 0 8px 0 12px;
      padding-top: env(safe-area-inset-top);
      box-sizing: content-box;
      border-bottom: 1px solid var(--divider);
      background: var(--bg);
    }
    .brand {
      display: flex;
      align-items: center;
      gap: 8px;
      color: var(--text);
      text-decoration: none;
      font-weight: 600;
      font-size: 15px;
      border-radius: var(--radius-sm);
    }
    main {
      flex: 1 1 auto;
      min-height: 0;
      overflow: auto;
      display: flex;
      flex-direction: column;
    }
    main > :not(router-outlet) {
      flex: 1 1 auto;
      min-height: 0;
    }
  `,
})
export class Shell {
  protected readonly auth = inject(AuthService);
  protected readonly socket = inject(NexusSocket);

  protected readonly connection = computed(() => {
    const bye = this.socket.goodbye();
    if (bye) return { kind: 'danger', text: t(`goodbye.${bye.code}`), retry: bye.code !== 'unauthorized' };
    switch (this.socket.state()) {
      case 'open':
        return null;
      case 'idle':
      case 'connecting':
        return { kind: 'info', text: t('conn.connecting'), retry: false };
      case 'closed':
        return { kind: 'warn', text: t('conn.reconnecting'), retry: true };
      default:
        return null;
    }
  });
}
