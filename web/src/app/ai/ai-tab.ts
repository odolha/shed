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
import { Menu, MenuTrigger } from '../../ui/menu';
import { Notices } from '../../ui/notices';
import { nowSecs, span } from '../../ui/time';
import { Scrollbar } from '../../ui/scrollbar';
import { Tooltip } from '../../ui/tooltip';
import { NARROW, mediaSignal } from '../../ui/viewport';
import { BenchActions } from '../bench/bench-actions';
import { tildify } from '../shared/tabs';
import { AiEntryView, AiRunCard, openRun, type UserAction } from './entries';
import {
  END_SLACK,
  Follow,
  type EntryBox,
  type TopEntry,
  atEnd,
  below,
  nextPrompt,
  previousPrompt,
  promptIndices,
  topEntry,
} from './follow';
import { AiPrompt } from './prompt';
import { AiReviewView } from './review';
import { AiSessionsPanel } from './sessions';
import { AiSettingsSheet, FULL_AUTO, MODES, type SettingsSheetData } from './settings-sheet';
import { AiLive, type LiveRun, liveRuns, runningStarts, tickEvery, timerLabel, timerTip, waitingSummary } from './runs';
import { applyAiEvent, workingOn } from './transcript';

export { FULL_AUTO, MODES } from './settings-sheet';

const PAGE = 150;

/** An AI tab: a Claude Code conversation, its prompt, sessions and review. */
@Component({
  selector: 'shed-ai-tab',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [TPipe, Icon, Tooltip, Menu, MenuTrigger, Scrollbar, AiEntryView, AiRunCard, AiPrompt, AiSessionsPanel, AiReviewView],
  providers: [AiLive],
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
      <button class="btn small" [class.on]="reviewing()" (click)="reviewing.set(!reviewing())" [shedTip]="'ai.review.open' | t">
        <shed-icon name="eye" /><span class="review-label">{{ 'ai.review.open' | t }}</span>
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

      <div class="convo" [class.full]="typing() && narrow()">
        @if (reviewing()) {
          <shed-ai-review
            [workspace]="workspace()"
            [tab]="tabId()"
            [home]="home()"
            [canChange]="canChange()"
            (back)="reviewing.set(false)"
          />
        } @else {
          <div class="view">
            <div
              class="scroll transcript no-scrollbar"
              #scroller
              tabindex="-1"
              (scroll)="onScroll()"
              (wheel)="userInput()"
              (keydown)="userInput()"
              (touchstart)="finger(true)"
              (touchend)="finger(false)"
              (touchcancel)="finger(false)"
            >
              <div class="inner" [class.navpad]="nav()">
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
                    [attr.data-i]="$index + offset()"
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
            <shed-scrollbar [for]="scroller" (drag)="barDrag($event)" />
            <div class="float">
              @if (nav()) {
                <div class="nav">
                  <button class="navbtn" (click)="previous()" [disabled]="previousIndex() === null" [shedTip]="'ai.previousPrompt' | t">
                    <shed-icon name="chevron-up" [size]="18" />
                  </button>
                  <button class="navbtn" (click)="next()" [disabled]="nextIndex() === null && !moreBelow()" [shedTip]="'ai.nextPrompt' | t">
                    <shed-icon name="chevron-down" [size]="18" />
                  </button>
                </div>
              }
              @if (follow.showJump(moreBelow()) && !(typing() && narrow())) {
                <button class="jump" (click)="toLatest()" [shedTip]="'ai.toEnd' | t">
                  <shed-icon name="arrow-down" [size]="18" />
                </button>
              }
            </div>
          </div>
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
          @if (waitingCount()) {
            <div class="waiting">
              @if (waitingCount() > 1 || typing()) {
                <button class="sumline small" (click)="openWaiting()" [attr.aria-expanded]="showWaiting()">
                  <shed-icon [name]="showWaiting() ? 'chevron-down' : 'chevron-right'" [size]="14" />
                  @if (runs().length) {
                    <span class="dots" aria-hidden="true"><i></i><i></i><i></i></span>
                  } @else {
                    <shed-icon name="clock" [size]="14" />
                  }
                  <span class="ellipsis grow">{{ summary() }}</span>
                </button>
              }
              @if (showWaiting()) {
                <div class="items">
                  @for (r of runs(); track r.index) {
                    <button class="item" (click)="openLive(r)">
                      <shed-ai-run-card [entry]="r.tool" />
                    </button>
                  }
                  @for (tm of timers(); track $index) {
                    <div class="timer small" tabindex="0" [shedTip]="timerTip(tm)">
                      <shed-icon [name]="tm.kind === 'watch' ? 'eye' : 'clock'" [size]="14" />
                      <span class="when">{{ timerLabel(tm, now()) }}</span>
                      <span class="ellipsis grow">{{ tm.what }}</span>
                    </div>
                  }
                </div>
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
                      <button class="btn ghost small" (click)="mergeDraft(d.text)" [shedTip]="'ai.mergeDraft.tip' | t">
                        <shed-icon name="plus" [size]="14" />{{ 'ai.mergeDraft' | t }}
                      </button>
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
            [settingsDanger]="a.mode === FULL_AUTO"
            (settings)="openSettings()"
            [(editing)]="editing"
            [(typing)]="typing"
          />

          <div class="status small" [class.quiet]="!busy() && a.activity !== 'waiting' && !waitingHere()">
            @if (busy()) {
              <span class="dots" aria-hidden="true"><i></i><i></i><i></i></span>
              <span class="ellipsis doing">{{ doing() }}</span>
            } @else if (a.activity === 'waiting' || waitingHere()) {
              <span class="accent">{{ 'ai.waiting' | t }}</span>
            }
            <span class="grow"></span>
            <span class="chips">
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
            </span>
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
    .view {
      flex: 1;
      min-height: 0;
      display: flex;
      flex-direction: column;
      position: relative;
    }
    .transcript {
      flex: 1;
      outline: none;
      overscroll-behavior: contain;
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
    /* Over the end of the conversation, at the side: kept clear of what is read at the end. */
    .float {
      position: absolute;
      right: 14px;
      bottom: 10px;
      z-index: 4;
      display: flex;
      flex-direction: column;
      align-items: center;
      gap: 8px;
      pointer-events: none;
    }
    .float > * {
      pointer-events: auto;
    }
    .nav {
      display: flex;
      flex-direction: column;
      border: 1px solid var(--divider);
      border-radius: 20px;
      background: color-mix(in srgb, var(--surface) 78%, transparent);
      overflow: hidden;
    }
    .navbtn {
      display: flex;
      align-items: center;
      justify-content: center;
      width: 36px;
      height: 36px;
      border: 0;
      background: none;
      color: var(--text-dim);
      cursor: pointer;
      -webkit-tap-highlight-color: transparent;
    }
    .navbtn:hover:not(:disabled) {
      background: var(--hover);
      color: var(--text);
    }
    .navbtn:disabled {
      color: color-mix(in srgb, var(--text-dim) 40%, transparent);
      cursor: default;
    }
    .jump {
      display: flex;
      align-items: center;
      justify-content: center;
      width: 40px;
      height: 40px;
      border: 0;
      border-radius: 50%;
      background: var(--accent);
      color: var(--accent-ink);
      box-shadow: var(--shadow);
      cursor: pointer;
    }
    .inner.navpad {
      padding-bottom: 96px;
    }
    @media (pointer: coarse) {
      .navbtn {
        width: 40px;
        height: 44px;
      }
      .jump {
        width: 44px;
        height: 44px;
      }
      .inner.navpad {
        padding-bottom: 108px;
      }
    }
    .bottom {
      max-width: 860px;
      width: 100%;
      margin: 0 auto;
      padding: 6px calc(12px + env(safe-area-inset-right)) calc(8px + env(safe-area-inset-bottom))
        calc(12px + env(safe-area-inset-left));
      display: flex;
      flex-direction: column;
      gap: 6px;
    }
    .waiting {
      display: flex;
      flex-direction: column;
      gap: 4px;
    }
    .sumline {
      display: flex;
      align-items: center;
      gap: 6px;
      min-height: 32px;
      padding: 2px 8px;
      border: 1px solid color-mix(in srgb, var(--accent) 40%, transparent);
      border-radius: var(--radius-sm);
      background: none;
      color: var(--text);
      font: inherit;
      text-align: left;
      cursor: pointer;
    }
    .sumline:hover {
      background: var(--hover);
    }
    .items {
      display: flex;
      flex-direction: column;
      gap: 4px;
      max-height: 38vh;
      overflow: auto;
    }
    .item {
      padding: 6px 8px;
      border: 1px solid var(--divider);
      border-left: 3px solid color-mix(in srgb, var(--accent) 60%, var(--divider));
      border-radius: var(--radius-sm);
      background: var(--surface);
      color: inherit;
      font: inherit;
      font-size: 13px;
      text-align: left;
      cursor: pointer;
    }
    .item:hover {
      background: var(--hover);
    }
    .strip {
      display: flex;
      align-items: center;
      gap: 6px;
      padding: 2px 4px 2px 10px;
      border-radius: var(--radius-sm);
      background: color-mix(in srgb, var(--info) 12%, transparent);
    }
    .timer {
      display: flex;
      align-items: center;
      gap: 6px;
      min-width: 0;
      min-height: 28px;
      padding: 2px 8px;
      border-radius: var(--radius-sm);
      border: 1px solid color-mix(in srgb, var(--accent) 40%, transparent);
      color: var(--accent);
    }
    .timer .grow {
      color: var(--text);
    }
    .when {
      flex: none;
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
    .chips {
      display: contents;
    }
    @media (max-width: 699px) {
      .wide-only,
      .chips {
        display: none;
      }
      .inner {
        padding: 8px 12px 20px 10px;
      }
      .status.quiet {
        display: none;
      }
      /* Typing on a phone: the prompt has the whole tab. The conversation
         stays, at no height, so it comes back scrolled where it was. */
      .convo.full .view {
        flex: 0 0 0;
        overflow: hidden;
      }
      .convo.full .bottom {
        flex: 1;
        min-height: 0;
      }
      .convo.full .bottom > :not(shed-ai-prompt) {
        display: none;
      }
      .status {
        min-height: 20px;
      }
      .head .review-label {
        display: none;
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
  private readonly live = inject(AiLive);
  private readonly actions = inject(BenchActions);
  protected readonly narrow = mediaSignal(NARROW);

  protected readonly PAGE = PAGE;
  protected readonly FULL_AUTO = FULL_AUTO;
  protected readonly modes = MODES;
  protected readonly timerLabel = timerLabel;
  protected readonly timerTip = timerTip;

  protected readonly tabId = computed(() => this.tab().id);
  protected readonly ai = computed<AiTabState>(() => {
    const k = this.tab().kind;
    return k.type === 'Ai' ? k.content : { folder: '', mode: 'default', activity: 'idle' };
  });
  protected readonly canChange = computed(() => this.store.canChange(this.workspace()));
  protected readonly home = computed(() => this.store.state(this.workspace())?.home ?? '');
  protected readonly folder = computed(() => tildify(this.ai().folder, this.home()));

  protected readonly entries = signal<AiEntry[]>([]);
  protected readonly status = signal<AiStatus>({ state: 'idle' });
  protected readonly info = signal<AiInfo | null>(null);
  protected readonly loaded = signal(false);
  protected readonly paused = signal(false);
  protected readonly endedText = signal<string | null>(null);
  protected readonly shown = signal(PAGE);
  /** Whether the conversation keeps to its end. */
  protected readonly follow = new Follow();
  /** There is more below what shows. */
  protected readonly moreBelow = signal(false);
  /** The conversation is longer than what shows. */
  private readonly scrollable = signal(false);
  /** The entry at the top of what shows, for the jumps between messages. */
  private readonly atTop = signal<TopEntry | null>(null);
  /** The prompt has the focus (on a phone: the keyboard is up). */
  protected readonly typing = signal(false);
  /** The list of what runs and waits above the prompt is open. */
  private readonly waitingOpen = signal(false);
  private fingerDown = false;
  private dragging = false;
  private inputAt = 0;
  private settleTimer: ReturnType<typeof setTimeout> | null = null;
  private measureFrame = 0;
  protected readonly sessionsOpen = signal(false);
  protected readonly reviewing = signal(false);
  protected readonly draftsOpen = signal(false);
  protected readonly sessionsReload = signal(0);
  protected readonly editing = signal<number | null>(null);
  private readonly resub = signal(0);
  private readonly tick = signal(0);

  protected readonly now = this.live.now;
  protected readonly timers = computed(() => this.status().timers ?? []);
  /** Sub-agents and background tasks still running. */
  protected readonly runs = computed(() => liveRuns(this.entries()));
  protected readonly waitingCount = computed(() => this.runs().length + this.timers().length);
  protected readonly summary = computed(() => waitingSummary(this.runs(), this.timers()));
  /** The list shows: one thing alone, or several opened; folded to its line while typing. */
  protected readonly showWaiting = computed(
    () => !this.typing() && (this.waitingCount() === 1 || this.waitingOpen()),
  );
  private readonly prompts = computed(() => promptIndices(this.entries()));
  /** ▲ ▼ between your messages: when there are some to go to, and not while typing. */
  protected readonly nav = computed(() => !this.typing() && this.scrollable() && this.prompts().length > 0);
  protected readonly previousIndex = computed(() => {
    const top = this.atTop();
    return top ? previousPrompt(this.entries(), top.top, top.inside) : null;
  });
  protected readonly nextIndex = computed(() => {
    const top = this.atTop();
    return top ? nextPrompt(this.entries(), top.top) : null;
  });
  /** When the runs going on started: what the clock counts besides timers. */
  private readonly starts = computed(() => runningStarts(this.entries()));

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
        this.follow.reset();
        this.moreBelow.set(false);
      });
      const sub = this.store.socket.subscribe(ws, { type: 'Ai', content: { tab } }, {
        event: (e) => {
          switch (e?.type) {
            case 'AiTranscript':
            case 'AiEntries': {
              const before = this.entries().length;
              const next = applyAiEvent({ entries: this.entries(), status: this.status() }, e);
              this.entries.set(next.entries);
              this.status.set(next.status);
              if (e.type === 'AiTranscript') {
                this.loaded.set(true);
                this.paused.set(false);
                if (before === 0 || this.follow.shouldScroll(true)) this.scrollToEnd(false);
                else this.measureSoon();
              }
              break;
            }
            case 'AiInfo':
              this.info.set(e.content);
              break;
            default:
              // Events of a newer workspace: not for this version.
              break;
          }
        },
        paused: () => this.paused.set(true),
        ended: (err) => this.endedText.set(err ? t(`error.${err.code}`) : t('ai.ended')),
      });
      onCleanup(() => sub.close());
    });
    // Kept at the end while what is around it changes size (the queue, the
    // prompt growing, a phone's keyboard), if it was at the end.
    effect((onCleanup) => {
      const el = this.scroller()?.nativeElement;
      if (!el || typeof ResizeObserver === 'undefined') return;
      const ro = new ResizeObserver(() => {
        if (this.follow.shouldScroll(below(el) > 0)) el.scrollTop = el.scrollHeight;
        this.measureSoon();
      });
      ro.observe(el);
      if (el.firstElementChild) ro.observe(el.firstElementChild);
      onCleanup(() => ro.disconnect());
    });
    // What the entries need to know besides their own: Claude working, timers.
    effect(() => {
      this.live.busy.set(this.busy());
      this.live.timers.set(this.timers());
    });
    // A clock for "working for 12s", runs' times and timers' countdowns:
    // every second while they change by the second, else every 30 s, else none.
    effect((onCleanup) => {
      this.tick();
      const now = nowSecs();
      // Fresh for what just changed (a timer set, a run started).
      untracked(() => this.live.now.set(now));
      const every = tickEvery(this.busy(), this.starts(), this.timers(), now);
      if (every === null) return;
      const timer = setTimeout(() => {
        this.live.now.set(nowSecs());
        this.tick.update((n) => n + 1);
      }, every);
      onCleanup(() => clearTimeout(timer));
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

  /** A wheel or a key on the conversation: the user moves it. */
  protected userInput(): void {
    this.inputAt = Date.now();
  }

  protected finger(down: boolean): void {
    this.fingerDown = down;
    if (!down) this.settleSoon();
  }

  protected barDrag(on: boolean): void {
    this.dragging = on;
    if (on) this.follow.dragged();
    else this.settleSoon();
  }

  protected onScroll(): void {
    const el = this.scroller()?.nativeElement;
    if (!el) return;
    // Moved by hand (a finger, the wheel, keys, the scrollbar), not by following.
    if (this.fingerDown || this.dragging || Date.now() - this.inputAt < 300) this.follow.dragged();
    this.settleSoon();
    this.measureSoon();
    if (el.scrollTop < 40 && this.hidden() > 0) {
      const height = el.scrollHeight;
      this.shown.update((n) => n + PAGE);
      requestAnimationFrame(() => (el.scrollTop += el.scrollHeight - height));
    }
  }

  /** Once the conversation stops moving: following again at its end, not elsewhere. */
  private settleSoon(): void {
    if (this.settleTimer) clearTimeout(this.settleTimer);
    this.settleTimer = setTimeout(() => {
      this.settleTimer = null;
      const el = this.scroller()?.nativeElement;
      if (!el || this.fingerDown || this.dragging) return;
      this.follow.settled(atEnd(el));
    }, 160);
  }

  /** What shows, measured at the next frame: more below, the entry at the top. */
  private measureSoon(): void {
    cancelAnimationFrame(this.measureFrame);
    this.measureFrame = requestAnimationFrame(() => {
      const el = this.scroller()?.nativeElement;
      if (!el) return;
      this.moreBelow.set(below(el) > END_SLACK);
      this.scrollable.set(el.scrollHeight > el.clientHeight + 4);
      const top = el.getBoundingClientRect().top;
      const boxes: EntryBox[] = [];
      for (const n of Array.from(el.querySelectorAll<HTMLElement>('shed-ai-entry[data-i]'))) {
        const r = n.getBoundingClientRect();
        boxes.push({ index: Number(n.dataset['i']), top: r.top - top, bottom: r.bottom - top });
        // Past the top: the rest are below it.
        if (r.bottom - top > 8) break;
      }
      const at = topEntry(boxes, 0);
      const old = this.atTop();
      if (at?.top !== old?.top || at?.inside !== old?.inside) this.atTop.set(at);
    });
  }

  /** To the latest entry, following it again (the ↓ button). */
  protected toLatest(): void {
    this.follow.toLatest();
    this.scrollToEnd(true);
  }

  private scrollToEnd(smooth: boolean): void {
    // After the new entries are drawn (the frame after this one).
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        const el = this.scroller()?.nativeElement;
        if (el) el.scrollTo({ top: el.scrollHeight, behavior: smooth ? 'smooth' : 'auto' });
        this.measureSoon();
      }),
    );
  }

  protected previous(): void {
    const i = this.previousIndex();
    if (i === null) return;
    this.follow.away();
    this.goTo(i);
  }

  protected next(): void {
    const i = this.nextIndex();
    if (i === null) {
      this.toLatest();
      return;
    }
    this.follow.away();
    this.goTo(i);
  }

  /** Scrolls an entry to the top, drawing earlier ones first if it is among them. */
  private goTo(index: number): void {
    if (index < this.offset()) this.shown.set(this.entries().length - index);
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        const el = this.scroller()?.nativeElement;
        const target = el?.querySelector<HTMLElement>(`shed-ai-entry[data-i="${index}"]`);
        if (!el || !target) return;
        const y = el.scrollTop + target.getBoundingClientRect().top - el.getBoundingClientRect().top - 8;
        el.scrollTo({ top: Math.max(0, y), behavior: 'smooth' });
      }),
    );
  }

  /** The line over what waits: opens the list (and folds the prompt away while typing). */
  protected openWaiting(): void {
    if (this.typing()) {
      this.waitingOpen.set(true);
      (document.activeElement as HTMLElement | null)?.blur();
      return;
    }
    this.waitingOpen.update((o) => !o);
  }

  /** A run's steps and report, from the strip above the prompt. */
  protected openLive(r: LiveRun): void {
    const tool = computed(() => {
      const e = this.entries()[r.index];
      return e?.type === 'Tool' ? e.content : r.tool;
    });
    openRun(this.dialogs, { tool, workspace: this.workspace(), tab: this.tabId(), live: this.live }, this.narrow());
  }

  /** The tab's settings as a sheet (narrow screens: the icon beside Send). */
  protected openSettings(): void {
    const data: SettingsSheetData = {
      ai: this.ai,
      info: this.info,
      canChange: this.canChange,
      home: this.home,
      change: (c) => this.settings(c),
      fullAuto: () => this.fullAuto(),
      pickFolder: () => this.actions.pickFolder(this.workspace(), t('ai.folderAddTitle'), this.bench()),
    };
    void this.dialogs.open<SettingsSheetData, null>(AiSettingsSheet, data, { sheet: true });
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

  /** Adds a draft's text at the end of the prompt; the draft stays. */
  protected mergeDraft(text: string): void {
    this.prompt()?.merge(text);
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
      else if (e.type === 'Note') parts.push(`· ${e.content.title}`);
    }
    void this.clip.copy(parts.join('\n\n'));
  }
}
