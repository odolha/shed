import {
  ChangeDetectionStrategy,
  Component,
  DestroyRef,
  OnInit,
  computed,
  inject,
  signal,
} from '@angular/core';
import { AuthService } from '../../api/auth';
import { errorText } from '../../api/errors';
import type { AuditEntry, Device, DeviceKind, PendingPairing } from '../../api/gen/api';
import { NexusHttp } from '../../api/http';
import { TPipe, t } from '../../i18n/i18n';
import { Dialogs } from '../../ui/dialog';
import { Icon } from '../../ui/icon';
import { Menu, MenuTrigger } from '../../ui/menu';
import { Notices } from '../../ui/notices';
import { AgoPipe, Clock, ClockPipe, span } from '../../ui/time';
import { Tooltip } from '../../ui/tooltip';
import { ApproveDialog, type ApproveData, type ApproveResult } from './approve-dialog';

const AUDIT_PAGE = 50;
const POLL_MS = 5000;

export function deviceIcon(kind: DeviceKind): string {
  return kind === 'phone' ? 'device' : kind === 'workspace' ? 'server' : 'globe';
}

/** What an audit entry's `what` means, or the word itself when unknown. */
export function auditText(what: string): string {
  const key = `devices.audit.${what}`;
  const text = t(key);
  return text === key ? what : text;
}

function where(address: string | undefined, country: string | null | undefined): string {
  return [address, country].filter(Boolean).join(' · ');
}

/** Paired devices and workspaces, pairing requests, and the audit log. */
@Component({
  selector: 'shed-devices-page',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [TPipe, Icon, Menu, MenuTrigger, Tooltip, AgoPipe, ClockPipe],
  template: `
    <div class="page col big-gap">
      <h1>{{ 'devices.title' | t }}</h1>

      <section class="col" aria-labelledby="pair-h">
        <div class="row">
          <h2 id="pair-h" class="grow">{{ 'devices.pairing' | t }}</h2>
          <button class="btn primary small" (click)="approve()">
            <shed-icon name="check" [size]="14" />{{ 'devices.approveOne' | t }}
          </button>
        </div>
        @for (p of pending(); track $index) {
          <div class="card item pairing">
            <shed-icon name="server" [size]="22" />
            <div class="grow col tight">
              <strong class="ellipsis">{{ p.name }}</strong>
              <span class="dim small">
                {{ 'devices.pairDetail' | t: { version: p.version, where: where(p.address, p.country) } }}
                · {{ expiresIn(p) }}
              </span>
            </div>
            <div class="row actions">
              <button class="btn small" (click)="approve(p)">{{ 'devices.approveEllipsis' | t }}</button>
            </div>
          </div>
        }
        <p class="dim hint">
          {{ 'devices.noPairing' | t }}
          <code>shed nexus join {{ origin }}</code>
        </p>
      </section>

      <section class="col" aria-labelledby="dev-h">
        <div class="row">
          <h2 id="dev-h" class="grow">{{ 'devices.devices' | t }}</h2>
          <button class="icon-btn" (click)="load()" [shedTip]="'common.refresh' | t">
            <shed-icon name="refresh" />
          </button>
        </div>
        @if (error(); as e) {
          <div class="error-box" role="alert">{{ e }}</div>
        }
        @for (d of devices(); track d.id) {
          <div class="card item">
            <shed-icon [name]="icon(d.kind)" [size]="22" />
            <div class="grow col tight">
              <div class="row">
                <span class="dot" [class.on]="d.online" [shedTip]="(d.online ? 'common.online' : 'common.offline') | t"></span>
                <strong class="ellipsis">{{ d.name }}</strong>
                @if (d.current) {
                  <span class="chip accent">{{ 'devices.thisBrowser' | t }}</span>
                }
              </div>
              <span class="dim small">
                {{ 'devices.kind.' + d.kind | t }} ·
                @if (d.online) {
                  {{ 'devices.connected' | t }}
                } @else {
                  {{ 'common.lastSeen' | t: { ago: (d.seen | ago: clock.now()) } }}
                }
                @if (where(d.address, d.country); as w) {
                  · {{ w }}
                }
              </span>
              <span class="dim small">{{ 'devices.added' | t: { at: (d.created | clock) } }}</span>
            </div>
            <button class="icon-btn" [shedMenu]="m" [attr.aria-label]="'common.more' | t">
              <shed-icon name="more" />
            </button>
            <shed-menu #m>
              <button class="menu-item" (click)="rename(d)"><shed-icon name="pencil" />{{ 'common.rename' | t }}</button>
              <div class="menu-sep"></div>
              <button class="menu-item danger" (click)="revoke(d)"><shed-icon name="trash" />{{ 'devices.revoke' | t }}</button>
            </shed-menu>
          </div>
        } @empty {
          @if (loading()) {
            <p class="dim">{{ 'common.loading' | t }}</p>
          }
        }
      </section>

      <section class="col" aria-labelledby="audit-h">
        <h2 id="audit-h">{{ 'devices.audit' | t }}</h2>
        @if (audit().length) {
          <ol class="audit card">
            @for (a of shownAudit(); track $index) {
              <li>
                <span class="when dim small" [shedTip]="a.at | clock">{{ a.at | ago: clock.now() }}</span>
                <span class="what" [class.danger-text]="a.what.endsWith('failed')">{{ auditText(a.what) }}</span>
                <span class="dim small ellipsis detail">
                  {{ deviceName(a.device) }}
                  @if (where(a.address, a.country); as w) {
                    · {{ w }}
                  }
                  @if (a.detail) {
                    · {{ a.detail }}
                  }
                </span>
              </li>
            }
          </ol>
          @if (audit().length > auditShown()) {
            <button class="btn ghost" (click)="auditShown.set(auditShown() + page)">{{ 'devices.showMore' | t }}</button>
          }
        } @else if (!loading()) {
          <p class="dim">{{ 'devices.noAudit' | t }}</p>
        }
      </section>
    </div>
  `,
  styles: `
    .page {
      max-width: 760px;
    }
    .big-gap {
      gap: 24px;
    }
    .tight {
      gap: 2px;
    }
    .item {
      display: flex;
      align-items: center;
      gap: 12px;
      padding: 10px 8px 10px 14px;
    }
    .item > shed-icon {
      color: var(--text-dim);
    }
    .pairing {
      border-color: color-mix(in srgb, var(--accent) 45%, var(--divider));
      flex-wrap: wrap;
    }
    .pair-code {
      font-size: 18px;
      letter-spacing: 0.06em;
      color: var(--accent);
    }
    .actions {
      margin-left: auto;
    }
    .hint {
      margin: 0;
    }
    .hint code {
      display: inline-block;
      margin-top: 4px;
      padding: 2px 6px;
      border-radius: 4px;
      background: var(--surface);
      border: 1px solid var(--divider);
      user-select: all;
    }
    .dot {
      background: var(--divider);
    }
    .dot.on {
      background: var(--ok);
    }
    .audit {
      margin: 0;
      padding: 4px 0;
      list-style: none;
    }
    .audit li {
      display: grid;
      grid-template-columns: 7em 9em 1fr;
      gap: 8px;
      align-items: baseline;
      padding: 6px 14px;
    }
    .audit li + li {
      border-top: 1px solid var(--divider);
    }
    @media (max-width: 560px) {
      .audit li {
        grid-template-columns: 1fr auto;
      }
      .audit .detail {
        grid-column: 1 / -1;
      }
    }
  `,
})
export class DevicesPage implements OnInit {
  private readonly http = inject(NexusHttp);
  private readonly auth = inject(AuthService);
  private readonly dialogs = inject(Dialogs);
  private readonly notices = inject(Notices);
  protected readonly clock = inject(Clock);

  protected readonly page = AUDIT_PAGE;
  protected readonly origin = location.origin;
  protected readonly devices = signal<Device[]>([]);
  protected readonly pending = signal<PendingPairing[]>([]);
  protected readonly audit = signal<AuditEntry[]>([]);
  protected readonly auditShown = signal(AUDIT_PAGE);
  protected readonly loading = signal(true);
  protected readonly error = signal<string | null>(null);

  protected readonly shownAudit = computed(() => this.audit().slice(0, this.auditShown()));
  private readonly names = computed(() => new Map(this.devices().map((d) => [d.id, d.name])));

  protected readonly icon = deviceIcon;
  protected readonly auditText = auditText;
  protected readonly where = where;

  constructor() {
    const timer = setInterval(() => {
      if (document.visibilityState === 'visible') void this.loadPending();
    }, POLL_MS);
    inject(DestroyRef).onDestroy(() => clearInterval(timer));
  }

  ngOnInit(): void {
    void this.load();
  }

  protected async load(): Promise<void> {
    this.loading.set(true);
    this.error.set(null);
    try {
      const [devices, audit] = await Promise.all([this.http.devices(), this.http.audit()]);
      this.devices.set(devices);
      this.audit.set([...audit].sort((a, b) => b.at - a.at));
    } catch (e) {
      this.error.set(errorText(e));
    } finally {
      this.loading.set(false);
    }
    await this.loadPending();
  }

  private async loadPending(): Promise<void> {
    try {
      this.pending.set(await this.http.pendingPairings());
    } catch {
      // The device list shows the error; polling goes on.
    }
  }

  protected deviceName(id: string | null | undefined): string {
    if (!id) return '';
    return this.names().get(id) ?? id;
  }

  protected expiresIn(p: PendingPairing): string {
    const left = p.expires - this.clock.now();
    return left > 0 ? t('devices.expiresIn', { span: span(left) }) : t('devices.expired');
  }

  /** Approve (or reject) by the code shown on the workspace, typed here. */
  protected async approve(p?: PendingPairing): Promise<void> {
    const r = await this.dialogs.open<ApproveData, ApproveResult>(ApproveDialog, { name: p?.name ?? '' });
    if (!r) return;
    try {
      if (r.action === 'reject') {
        await this.http.rejectPairing({ code: r.code });
        this.notices.info(t('devices.rejected'));
        await this.loadPending();
      } else {
        await this.http.approvePairing({ code: r.code, name: r.name, totp: r.totp });
        this.notices.info(t('devices.approved', { name: r.name }));
        await this.load();
      }
    } catch (e) {
      this.notices.error(e);
    }
  }

  protected async rename(d: Device): Promise<void> {
    const name = await this.dialogs.prompt({ title: t('devices.renameTitle'), label: t('devices.name'), value: d.name });
    if (!name || name === d.name) return;
    if ((await this.notices.run(this.http.renameDevice({ device: d.id, name }))) !== undefined) await this.load();
  }

  protected async revoke(d: Device): Promise<void> {
    const totp = await this.dialogs.totp({
      title: t('devices.revokeTitle', { name: d.name }),
      body: t(d.current ? 'devices.revokeCurrent' : 'devices.revokeBody'),
      ok: t('devices.revoke'),
      danger: true,
    });
    if (!totp) return;
    try {
      await this.http.revoke({ device: d.id, totp });
    } catch (e) {
      this.notices.error(e);
      return;
    }
    if (d.current) await this.auth.signOut();
    else {
      this.notices.info(t('devices.revoked', { name: d.name }));
      await this.load();
    }
  }
}
