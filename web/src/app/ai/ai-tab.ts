import {
  ChangeDetectionStrategy,
  Component,
  ElementRef,
  computed,
  effect,
  inject,
  input,
  signal,
  untracked,
  viewChild,
} from '@angular/core';
import type { AiEntry, AiInfo, AiSettings, AiStatus, AiTab as AiTabState, Bench, Tab } from '../../api/gen/api';
import { WorkspacesStore } from '../../api/workspaces.store';
import { TPipe, t } from '../../i18n/i18n';
import { Clipboard } from '../../ui/clipboard';
import { Dialogs } from '../../ui/dialog';
import { Icon } from '../../ui/icon';
import { LimitsBars } from '../../ui/limits';
import { Menu, MenuTrigger } from '../../ui/menu';
import { Notices } from '../../ui/notices';
import { span } from '../../ui/time';
import { Tooltip } from '../../ui/tooltip';
import { tildify } from '../shared/tabs';
import { AiEntryView, type UserAction } from './entries';
import { AiPrompt } from './prompt';
import { AiReviewView } from './review';
import { AiSessionsPanel } from './sessions';
import { applyTranscript, workingOn } from './transcript';

/** Claude Code's permission modes: its value, the menu's label key. */
export const MODES: { value: string; key: string }[] = [
  { value: 'default', key: 'ai.mode.default' },
  { value: 'acceptEdits', key: 'ai.mode.acceptEdits' },
  { value: 'auto', key: 'ai.mode.auto' },
  { value: 'plan', key: 'ai.mode.plan' },
];
export const FULL_AUTO = 'bypassPermissions';

const PAGE = 150;

/** An AI tab: a Claude Code conversation, its prompt, sessions and review. */
@Component({
  selector: 'shed-ai-tab',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [TPipe, Icon, Tooltip, Menu, MenuTrigger, LimitsBars, AiEntryView, AiPrompt, AiSessionsPanel, AiReviewView],
  template: `
    @let a = ai();
    <header class="head">
      <button
        class="icon-btn"
        [class.on]="sessionsOpen()"
        (click)="sessionsOpen.set(!sessionsOpen())"
        [shedTip]="'ai.sessions.title' | t"
        [attr.aria-expanded]="sessionsOpen()"
      >
        <shed-icon name="history" [size]="18" />
      </button>
      <div class="title-box">
        <span class="title ellipsis">{{ tab().title }}</span>
        <span class="dim small ellipsis">
          {{ folder() }}
          @if (status().cost) {
            · {{ cost() }}
          }
          @if (context(); as c) {
            · {{ c }}
          }
        </span>
      </div>
      <span class="grow"></span>
      <shed-limits class="wide-only" [limits]="limits()" />
      <button class="btn small" [class.on]="reviewing()" (click)="reviewing.set(!reviewing())">
        <shed-icon name="eye" />{{ 'ai.review.open' | t }}
      </button>
      <button class="icon-btn" [shedMenu]="more" [shedTip]="'common.more' | t"><shed-icon name="more" [size]="18" /></button>
      <shed-menu #more>
        <button class="menu-item" (click)="reviewing.set(true)"><shed-icon name="eye" />{{ 'ai.review.open' | t }}</button>
        <button class="menu-item" (click)="copyAll()"><shed-icon name="copy" />{{ 'ai.copyAll' | t }}</button>
        @if (canChange() && !started()) {
          <button class="menu-item" (click)="call({ type: 'AiStart', content: { tab: tabId() } })">
            <shed-icon name="power" />{{ 'ai.start' | t }}
          </button>
        }
        <button class="menu-item" (click)="resubscribe()"><shed-icon name="refresh" />{{ 'ai.reload' | t }}</button>
      </shed-menu>
    </header>

    <div class="main">
      @if (sessionsOpen()) {
        <aside class="sessions">
          <shed-ai-sessions
            [workspace]="workspace()"
            [tab]="tabId()"
            [current]="a.session ?? a.draftSession ?? null"
            [canChange]="canChange()"
            [reload]="sessionsReload()"
            (opened)="onSessionOpened()"
          />
        </aside>
      }

      <div class="convo">
        @if (reviewing()) {
          <shed-ai-review
            [workspace]="workspace()"
            [tab]="tabId()"
            [home]="home()"
            [canChange]="canChange()"
            (back)="reviewing.set(false)"
          />
        } @else {
          <div class="scroll transcript" #scroller (scroll)="onScroll()">
            <div class="inner">
              @if (paused()) {
                <div class="error-box small">{{ 'ai.paused' | t }}</div>
              }
              @if (endedText(); as e) {
                <div class="error-box small">{{ e }}</div>
              }
              @if (hidden() > 0) {
                <div class="earlier">
                  <button class="btn small" (click)="shown.set(shown() + PAGE)">{{ 'ai.showEarlier' | t: { n: hidden() } }}</button>
                  <button class="btn small ghost" (click)="shown.set(entries().length)">{{ 'ai.showAll' | t }}</button>
                </div>
              }
              @for (e of visible(); track $index + offset()) {
                <shed-ai-entry
                  [entry]="e"
                  [index]="$index + offset()"
                  [workspace]="workspace()"
                  [tab]="tabId()"
                  [canChange]="canChange()"
                  (act)="onUserAction($event)"
                />
              } @empty {
                @if (loaded()) {
                  <div class="empty">
                    <shed-icon name="ai" [size]="28" />
                    <p>{{ 'ai.emptySession' | t }}</p>
                  </div>
                } @else {
                  <div class="empty">{{ 'common.loading' | t }}</div>
                }
              }
            </div>
          </div>
          @if (!atBottom()) {
            <button class="icon-btn jump" (click)="scrollDown(true)" [shedTip]="'ai.toEnd' | t">
              <shed-icon name="arrow-down" [size]="18" />
            </button>
          }
        }

        <div class="bottom">
          @if (a.browser) {
            <div class="strip small">
              <shed-icon name="globe" [size]="14" />
              <span class="grow">{{ 'ai.browserOn' | t }}</span>
              @if (canChange()) {
                <button class="btn ghost small" (click)="settings({ browser: false })">{{ 'ai.disconnect' | t }}</button>
              }
            </div>
          }
          @if (a.queued?.length) {
            <div class="queue">
              <div class="dim small">{{ 'ai.queued' | t: { n: a.queued?.length ?? 0 } }}</div>
              @for (q of a.queued ?? []; track $index; let i = $index) {
                <div class="queued">
                  <shed-icon name="queue" [size]="14" />
                  <span class="ellipsis grow">{{ q }}</span>
                  @if (canChange()) {
                    <button class="btn ghost small" (click)="editQueued(i, q)">{{ 'common.edit' | t }}</button>
                    <button class="btn ghost small" (click)="queued(i, q, true)">{{ 'ai.sendNow' | t }}</button>
                    <button class="btn ghost small danger" (click)="queued(i, '', false)">{{ 'common.remove' | t }}</button>
                  }
                </div>
              }
            </div>
          }
          @if (drafts().length) {
            <div class="drafts">
              <button class="btn ghost small" (click)="draftsOpen.set(!draftsOpen())" [attr.aria-expanded]="draftsOpen()">
                <shed-icon [name]="draftsOpen() ? 'chevron-down' : 'chevron-right'" />
                {{ 'ai.drafts' | t: { n: drafts().length } }}
              </button>
              @if (draftsOpen()) {
                @for (d of drafts(); track d.index) {
                  <div class="queued">
                    <shed-icon name="pencil" [size]="14" />
                    <span class="ellipsis grow">{{ d.text }}</span>
                    @if (canChange()) {
                      <button class="btn ghost small" (click)="sendDraft(d.index, d.text)">{{ 'common.send' | t }}</button>
                      <button class="btn ghost small" (click)="editDraft(d.index, d.text)">{{ 'common.edit' | t }}</button>
                      <button class="btn ghost small danger" (click)="deleteDraft(d.index, d.text)">{{ 'common.delete' | t }}</button>
                    }
                  </div>
                }
              }
            </div>
          }

          <shed-ai-prompt
            #prompt
            [workspace]="workspace()"
            [tab]="tabId()"
            [ai]="a"
            [info]="info()"
            [working]="busy()"
            [canChange]="canChange()"
            [(editing)]="editing"
          />

          <div class="status small">
            @if (busy()) {
              <span class="dots" aria-hidden="true"><i></i><i></i><i></i></span>
              <span class="ellipsis doing">{{ doing() }}</span>
            } @else if (a.activity === 'waiting' || waitingHere()) {
              <span class="accent">{{ 'ai.waiting' | t }}</span>
            }
            <span class="grow"></span>
            <button class="chipbtn" [shedMenu]="modelMenu" [disabled]="!canChange()">{{ modelLabel() }}</button>
            @if (efforts().length) {
              <button class="chipbtn" [shedMenu]="effortMenu" [disabled]="!canChange()">{{ effortLabel() }}</button>
            }
            <button class="chipbtn" [class.danger-text]="a.mode === FULL_AUTO" [shedMenu]="modeMenu" [disabled]="!canChange()">
              {{ modeLabel() }}
            </button>
            <button
              class="icon-btn small"
              [class.on]="a.follow"
              (click)="settings({ follow: !a.follow })"
              [disabled]="!canChange()"
              [shedTip]="'ai.follow' | t"
              [attr.aria-pressed]="a.follow ?? false"
            >
              <shed-icon name="eye" />
            </button>
            <button
              class="icon-btn small"
              [class.on]="a.browser"
              (click)="settings({ browser: !a.browser })"
              [disabled]="!canChange()"
              [shedTip]="'ai.browser' | t"
              [attr.aria-pressed]="a.browser ?? false"
            >
              <shed-icon name="globe" />
            </button>
          </div>
        </div>
      </div>
    </div>

    <shed-menu #modelMenu>
      <div class="menu-label">{{ 'ai.model' | t }}</div>
      @for (m of models(); track m.value) {
        <button class="menu-item" (click)="settings({ model: m.value })">
          <span class="col" style="gap: 0">
            <span>{{ m.name }}</span>
            @if (m.description) {
              <span class="dim small">{{ m.description }}</span>
            }
          </span>
          @if ((a.model ?? '') === m.value) {
            <span class="check-mark"><shed-icon name="check" /></span>
          }
        </button>
      } @empty {
        <div class="menu-label">{{ 'common.loading' | t }}</div>
      }
    </shed-menu>
    <shed-menu #effortMenu>
      <div class="menu-label">{{ 'ai.effort' | t }}</div>
      <button class="menu-item" (click)="settings({ effort: '' })">
        {{ 'ai.effortDefault' | t }}
        @if (!a.effort) {
          <span class="check-mark"><shed-icon name="check" /></span>
        }
      </button>
      @for (e of efforts(); track e) {
        <button class="menu-item" (click)="settings({ effort: e })">
          {{ 'ai.effortN' | t: { effort: e } }}
          @if (a.effort === e) {
            <span class="check-mark"><shed-icon name="check" /></span>
          }
        </button>
      }
    </shed-menu>
    <shed-menu #modeMenu>
      <div class="menu-label">{{ 'ai.modeTitle' | t }}</div>
      @for (m of modes; track m.value) {
        <button class="menu-item" (click)="settings({ mode: m.value })">
          {{ m.key | t }}
          @if (a.mode === m.value) {
            <span class="check-mark"><shed-icon name="check" /></span>
          }
        </button>
      }
      <div class="menu-sep"></div>
      <button class="menu-item danger" (click)="fullAuto()">
        {{ 'ai.mode.full' | t }}
        @if (a.mode === FULL_AUTO) {
          <span class="check-mark"><shed-icon name="check" /></span>
        }
      </button>
    </shed-menu>
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      flex: 1 1 auto;
      min-height: 0;
    }
    .head {
      display: flex;
      align-items: center;
      gap: 8px;
      padding: 6px 8px;
      border-bottom: 1px solid var(--divider);
      min-height: 48px;
    }
    .title-box {
      display: flex;
      flex-direction: column;
      min-width: 0;
      flex: 0 1 auto;
    }
    .title {
      font-weight: 600;
    }
    .main {
      flex: 1;
      min-height: 0;
      display: flex;
      position: relative;
    }
    .sessions {
      width: 300px;
      flex: none;
      display: flex;
      flex-direction: column;
      border-right: 1px solid var(--divider);
      background: var(--bg);
      min-height: 0;
    }
    .sessions > * {
      flex: 1;
    }
    @media (max-width: 800px) {
      .sessions {
        position: absolute;
        inset: 0 auto 0 0;
        width: min(340px, 88vw);
        z-index: 5;
        box-shadow: var(--shadow);
      }
    }
    .convo {
      flex: 1;
      min-width: 0;
      display: flex;
      flex-direction: column;
      position: relative;
    }
    .transcript {
      flex: 1;
    }
    .inner {
      max-width: 860px;
      margin: 0 auto;
      padding: 12px 16px 24px;
    }
    .earlier {
      display: flex;
      justify-content: center;
      gap: 8px;
      margin-bottom: 8px;
    }
    .jump {
      position: absolute;
      right: 18px;
      bottom: 210px;
      background: var(--raised);
      border: 1px solid var(--divider);
      box-shadow: var(--shadow);
      border-radius: 50%;
      z-index: 2;
    }
    .bottom {
      max-width: 860px;
      width: 100%;
      margin: 0 auto;
      padding: 6px 12px calc(8px + env(safe-area-inset-bottom));
      display: flex;
      flex-direction: column;
      gap: 6px;
    }
    .strip {
      display: flex;
      align-items: center;
      gap: 6px;
      padding: 2px 4px 2px 10px;
      border-radius: var(--radius-sm);
      background: color-mix(in srgb, var(--info) 12%, transparent);
    }
    .queue,
    .drafts {
      display: flex;
      flex-direction: column;
      gap: 2px;
    }
    .drafts > .btn {
      align-self: flex-start;
    }
    .queued {
      display: flex;
      align-items: center;
      gap: 6px;
      padding: 2px 4px 2px 8px;
      border-radius: var(--radius-sm);
      background: var(--surface);
      border: 1px solid var(--divider);
      flex-wrap: wrap;
    }
    .status {
      display: flex;
      align-items: center;
      flex-wrap: wrap;
      gap: 4px;
      color: var(--text-dim);
      min-height: 28px;
    }
    .accent {
      color: var(--accent);
    }
    .doing {
      max-width: 50%;
    }
    .chipbtn {
      border: 1px solid transparent;
      background: none;
      color: var(--text-dim);
      font: inherit;
      padding: 2px 8px;
      border-radius: 11px;
      cursor: pointer;
    }
    .chipbtn:hover:not(:disabled) {
      background: var(--hover);
      color: var(--text);
    }
    .chipbtn:disabled {
      cursor: default;
    }
    .dots {
      display: inline-flex;
      gap: 3px;
      margin-right: 4px;
    }
    .dots i {
      width: 5px;
      height: 5px;
      border-radius: 50%;
      background: var(--accent);
      animation: blink 1.2s infinite ease-in-out;
    }
    .dots i:nth-child(2) {
      animation-delay: 0.2s;
    }
    .dots i:nth-child(3) {
      animation-delay: 0.4s;
    }
    @keyframes blink {
      0%,
      80%,
      100% {
        opacity: 0.25;
      }
      40% {
        opacity: 1;
      }
    }
    @media (max-width: 700px) {
      .wide-only {
        display: none;
      }
      .inner {
        padding: 8px 10px 20px;
      }
    }
  `,
})
export class AiTab {
  readonly workspace = input.required<string>();
  readonly bench = input.required<Bench>();
  readonly tab = input.required<Tab>();

  private readonly store = inject(WorkspacesStore);
  private readonly notices = inject(Notices);
  private readonly dialogs = inject(Dialogs);
  private readonly clip = inject(Clipboard);
  private readonly scroller = viewChild<ElementRef<HTMLElement>>('scroller');
  private readonly prompt = viewChild<AiPrompt>('prompt');

  protected readonly PAGE = PAGE;
  protected readonly FULL_AUTO = FULL_AUTO;
  protected readonly modes = MODES;

  protected readonly tabId = computed(() => this.tab().id);
  protected readonly ai = computed<AiTabState>(() => {
    const k = this.tab().kind;
    return k.type === 'Ai' ? k.content : { folder: '', mode: 'default', activity: 'idle' };
  });
  protected readonly canChange = computed(() => this.store.canChange(this.workspace()));
  protected readonly home = computed(() => this.store.state(this.workspace())?.home ?? '');
  protected readonly limits = computed(() => this.store.state(this.workspace())?.limits);
  protected readonly folder = computed(() => tildify(this.ai().folder, this.home()));

  protected readonly entries = signal<AiEntry[]>([]);
  protected readonly status = signal<AiStatus>({ state: 'idle' });
  protected readonly info = signal<AiInfo | null>(null);
  protected readonly loaded = signal(false);
  protected readonly paused = signal(false);
  protected readonly endedText = signal<string | null>(null);
  protected readonly shown = signal(PAGE);
  protected readonly atBottom = signal(true);
  protected readonly sessionsOpen = signal(false);
  protected readonly reviewing = signal(false);
  protected readonly draftsOpen = signal(false);
  protected readonly sessionsReload = signal(0);
  protected readonly editing = signal<number | null>(null);
  private readonly resub = signal(0);
  private readonly tick = signal(0);

  protected readonly offset = computed(() => Math.max(0, this.entries().length - this.shown()));
  protected readonly visible = computed(() => this.entries().slice(this.offset()));
  protected readonly hidden = computed(() => this.offset());
  protected readonly busy = computed(() => this.status().state === 'working' || this.ai().activity === 'working');
  protected readonly started = computed(() => !!this.status().session || this.entries().length > 0);
  protected readonly waitingHere = computed(() => this.status().state === 'waiting');

  protected readonly doing = computed(() => {
    this.tick();
    const what = workingOn(this.entries());
    const since = this.status().since;
    const secs = since ? Math.floor(Date.now() / 1000) - since : 0;
    return since ? `${what} · ${span(secs)}` : what;
  });

  protected readonly cost = computed(() => `$${(this.status().cost ?? 0).toFixed(2)}`);
  protected readonly context = computed(() => {
    const s = this.status();
    if (!s.context) return null;
    const k = (n: number) => (n >= 1_000_000 ? `${+(n / 1_000_000).toFixed(1)}M` : `${Math.round(n / 1000)}k`);
    return s.contextMax ? t('ai.context', { used: k(s.context), max: k(s.contextMax) }) : t('ai.contextUsed', { used: k(s.context) });
  });

  protected readonly models = computed(() => this.info()?.models ?? []);
  protected readonly currentModel = computed(() => {
    const value = this.ai().model ?? '';
    return this.models().find((m) => m.value === value) ?? this.models().find((m) => m.value === '');
  });
  protected readonly modelLabel = computed(() => this.currentModel()?.name ?? (this.ai().model || t('ai.modelDefault')));
  protected readonly efforts = computed(() => this.currentModel()?.efforts ?? []);
  protected readonly effortLabel = computed(() => {
    const e = this.ai().effort;
    return e ? t('ai.effortN', { effort: e }) : t('ai.effortDefault');
  });
  protected readonly modeLabel = computed(() => {
    const mode = this.ai().mode;
    if (mode === FULL_AUTO) return t('ai.mode.full');
    const m = MODES.find((x) => x.value === mode);
    return m ? t(m.key) : mode;
  });

  /** This session's drafts, with their index in the tab's list. */
  protected readonly drafts = computed(() => {
    const a = this.ai();
    const session = a.session ?? a.draftSession ?? this.status().session ?? null;
    return (a.drafts ?? [])
      .map((d, index) => ({ ...d, index }))
      .filter((d) => session !== null && d.session === session);
  });

  constructor() {
    // The conversation, while the tab is shown.
    effect((onCleanup) => {
      const ws = this.workspace();
      const tab = this.tabId();
      this.resub();
      untracked(() => {
        this.entries.set([]);
        this.loaded.set(false);
        this.paused.set(false);
        this.endedText.set(null);
        this.shown.set(PAGE);
        this.atBottom.set(true);
      });
      const sub = this.store.socket.subscribe(ws, { type: 'Ai', content: { tab } }, {
        event: (e) => {
          if (e.type === 'AiTranscript') {
            const before = this.entries().length;
            this.entries.update((old) => applyTranscript(old, e.content));
            this.status.set(e.content.status);
            this.loaded.set(true);
            this.paused.set(false);
            if (this.atBottom() || before === 0) this.scrollDown(false);
          } else if (e.type === 'AiInfo') {
            this.info.set(e.content);
          }
        },
        paused: () => this.paused.set(true),
        ended: (err) => this.endedText.set(err ? t(`error.${err.code}`) : t('ai.ended')),
      });
      onCleanup(() => sub.close());
    });
    // A clock for "working for 12s".
    effect((onCleanup) => {
      if (!this.busy()) return;
      const timer = setInterval(() => this.tick.update((n) => n + 1), 1000);
      onCleanup(() => clearInterval(timer));
    });
    // The sessions list follows turns ending and drafts changing.
    effect(() => {
      this.busy();
      this.ai().drafts?.length;
      untracked(() => this.sessionsReload.update((n) => n + 1));
    });
  }

  protected resubscribe(): void {
    this.resub.update((n) => n + 1);
  }

  protected onScroll(): void {
    const el = this.scroller()?.nativeElement;
    if (!el) return;
    const bottom = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
    if (bottom !== this.atBottom()) this.atBottom.set(bottom);
    if (el.scrollTop < 40 && this.hidden() > 0) {
      const height = el.scrollHeight;
      this.shown.update((n) => n + PAGE);
      requestAnimationFrame(() => (el.scrollTop += el.scrollHeight - height));
    }
  }

  protected scrollDown(smooth: boolean): void {
    requestAnimationFrame(() => {
      const el = this.scroller()?.nativeElement;
      if (el) el.scrollTo({ top: el.scrollHeight, behavior: smooth ? 'smooth' : 'auto' });
      this.atBottom.set(true);
    });
  }

  protected onSessionOpened(): void {
    if (matchMedia('(max-width: 800px)').matches) this.sessionsOpen.set(false);
    this.reviewing.set(false);
  }

  protected async call(call: Parameters<WorkspacesStore['request']>[1]): Promise<boolean> {
    return (await this.notices.run(this.store.request(this.workspace(), call))) !== undefined;
  }

  protected settings(change: Omit<AiSettings, 'tab'>): void {
    void this.call({ type: 'AiSettings', content: { tab: this.tabId(), ...change } });
  }

  protected async fullAuto(): Promise<void> {
    if (this.ai().mode === FULL_AUTO) return;
    const ok = await this.dialogs.confirm({
      title: t('ai.fullAuto.title'),
      body: t('ai.fullAuto.body'),
      ok: t('ai.fullAuto.ok'),
      danger: true,
    });
    if (ok) this.settings({ mode: FULL_AUTO });
  }

  protected async onUserAction(a: UserAction): Promise<void> {
    switch (a.kind) {
      case 'edit':
        this.editing.set(a.index);
        this.prompt()?.fill(a.text);
        break;
      case 'resend': {
        const last = this.lastUserIndex() === a.index;
        if (!last) {
          const ok = await this.dialogs.confirm({
            title: t('ai.resend.title'),
            body: t('ai.resend.body'),
            ok: t('ai.sendAgain'),
          });
          if (!ok) return;
        }
        void this.call({ type: 'AiEdit', content: { tab: this.tabId(), entry: a.index, text: a.text, pictures: [] } });
        break;
      }
      case 'revert': {
        const files = await this.dialogs.choose<boolean>({
          title: t('ai.revertTitle'),
          body: t('ai.revertBody'),
          choices: [
            { label: t('ai.revertConversation'), value: false, danger: true },
            { label: t('ai.revertFiles'), value: true, danger: true, primary: true },
          ],
        });
        if (files === null) return;
        void this.call({ type: 'AiRevert', content: { tab: this.tabId(), entry: a.index, files } });
        break;
      }
    }
  }

  private lastUserIndex(): number {
    const e = this.entries();
    for (let i = e.length - 1; i >= 0; i--) if (e[i]?.type === 'User') return i;
    return -1;
  }

  protected async editQueued(index: number, text: string): Promise<void> {
    const next = await this.dialogs.prompt({ title: t('ai.editQueued'), value: text, multiline: true, ok: t('common.save') });
    if (next !== null && next.trim() && next !== text) void this.queued(index, next.trim(), false);
  }

  protected queued(index: number, text: string, send: boolean): Promise<boolean> {
    return this.call({ type: 'AiQueued', content: { tab: this.tabId(), index, text, send } });
  }

  protected async sendDraft(index: number, text: string): Promise<void> {
    if (await this.call({ type: 'AiSend', content: { tab: this.tabId(), text, pictures: [] } })) {
      void this.call({ type: 'AiRemoveDraft', content: { tab: this.tabId(), index, text } });
    }
  }

  protected async editDraft(index: number, text: string): Promise<void> {
    if (await this.call({ type: 'AiRemoveDraft', content: { tab: this.tabId(), index, text } })) {
      this.prompt()?.fill(text);
    }
  }

  protected async deleteDraft(index: number, text: string): Promise<void> {
    const ok = await this.dialogs.confirm({
      title: t('ai.deleteDraft.title'),
      body: text.length > 200 ? text.slice(0, 200) + '…' : text,
      ok: t('ai.deleteDraft.ok'),
      danger: true,
    });
    if (ok) void this.call({ type: 'AiRemoveDraft', content: { tab: this.tabId(), index, text } });
  }

  protected copyAll(): void {
    const parts: string[] = [];
    for (const e of this.entries()) {
      if (e.type === 'User') parts.push(`> ${e.content.text.replace(/\n/g, '\n> ')}`);
      else if (e.type === 'Assistant') parts.push(e.content.text);
      else if (e.type === 'Tool') parts.push(`· ${e.content.summary}`);
    }
    void this.clip.copy(parts.join('\n\n'));
  }
}
