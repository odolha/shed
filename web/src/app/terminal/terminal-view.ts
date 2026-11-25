/**
 * One terminal pane: its screen as text (the `Screen` topic), re-wrapped to
 * this view's width on the normal screen and fitted on the alternate one;
 * scrollback fetched when scrolling up; keys and text sent as `Input`. On the
 * alternate screen, clicks go to a program that asked for the mouse (Shift
 * selects text instead), and the wheel goes to it in notches.
 */

import {
  ChangeDetectionStrategy,
  Component,
  DestroyRef,
  ElementRef,
  afterNextRender,
  afterRenderEffect,
  computed,
  effect,
  inject,
  input,
  output,
  signal,
  untracked,
  viewChild,
} from '@angular/core';
import type { InputItem, MouseButton, Pane, ScreenLine } from '../../api/gen/api';
import { WorkspacesStore } from '../../api/workspaces.store';
import { TPipe } from '../../i18n/i18n';
import { Clipboard } from '../../ui/clipboard';
import { Icon } from '../../ui/icon';
import { Notices } from '../../ui/notices';
import { Tooltip } from '../../ui/tooltip';
import { tildify } from '../shared/tabs';
import { type Mods, clipboardShortcut, keyToInput, mergeInput, textToInput } from './keys';
import { type Cell, type GridGeom, type MouseMods, WheelNotches, buttonOf, cellAt, mouseItem, wheelItems } from './mouse';
import { joinRows, locate, rewrap } from './rewrap';
import { BLANK, ScreenModel } from './screen-model';
import { type RSpan, drawSpans } from './style';

interface ScreenSnap {
  lines: ScreenLine[];
  cols: number;
  rows: number;
  alt: boolean;
  /** The program asked for mouse events. */
  mouse: boolean;
  cursor: { row: number; col: number; visible: boolean };
  title: string | null;
  hasMore: boolean;
}

interface Drawn {
  lines: RSpan[][];
  cursorIndex: number;
  cursorCol: number;
}

/** The text that stays in the input catcher, so phone keyboards can delete. */
const SENTINEL = ' ';
const PAD_X = 12;
const PAD_Y = 8;
const MAC = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

function emptyRow(l: ScreenLine): boolean {
  return l.spans.every((s) => !s.bg && !s.inverse && !s.text.trim());
}

@Component({
  selector: 'shed-terminal-view',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [Icon, TPipe, Tooltip],
  host: { '(focusin)': 'focused.emit()', '(pointerdown)': 'focused.emit()' },
  template: `
    <div class="head">
      <shed-icon [name]="pane().tool ? 'tool' : 'terminal'" [size]="14" />
      <span class="ellipsis grow">{{ heading() }}</span>
      @if (!canType()) {
        <span class="chip">{{ (online() ? 'common.viewOnly' : 'common.offline') | t }}</span>
      }
      @if (heldElsewhere()) {
        <span class="chip warn">{{ 'term.sizeHeldElsewhere' | t }}</span>
      }
      <span class="dim small mono size">{{ snap().cols }}×{{ snap().rows }}</span>
      @if (canType()) {
        <button
          class="icon-btn small"
          [class.on]="holding()"
          [attr.aria-pressed]="holding()"
          [shedTip]="(holding() ? 'term.releaseSize' : 'term.takeSize') | t"
          (click)="toggleSize()"
        >
          <shed-icon name="resize" [size]="15" />
        </button>
        <button
          class="icon-btn small"
          [class.on]="showKeys()"
          [attr.aria-pressed]="showKeys()"
          [shedTip]="'term.keyBar' | t"
          (click)="showKeys.set(!showKeys())"
        >
          <shed-icon name="keyboard" [size]="15" />
        </button>
      }
    </div>
    <div
      class="screen"
      #screen
      role="region"
      [attr.aria-label]="'term.screen' | t"
      [class.alt]="snap().alt"
      [class.mouse]="snap().alt && snap().mouse && canType()"
      (scroll)="onScroll()"
      (click)="onClick()"
      (mousedown)="onMouseDown($event)"
      (pointerdown)="onPointerDown($event)"
      (pointermove)="onPointerMove($event)"
      (pointerup)="onPointerUp($event)"
      (pointercancel)="onPointerCancel($event)"
      (contextmenu)="onContextMenu($event)"
      (wheel)="onWheel($event)"
    >
      <span class="measure" #measure aria-hidden="true">MMMMMMMMMMMMMMMMMMMM</span>
      @if (status() === 'paused') {
        <div class="state">{{ 'term.paused' | t }}</div>
      } @else if (status() === 'ended') {
        <div class="state">{{ 'term.ended' | t }}</div>
      } @else if (status() === 'loading') {
        <div class="state">{{ 'common.loading' | t }}</div>
      }
      @if (snap().alt) {
        @let a = alt();
        <div class="grid" #grid [style.font-size.px]="altFont()">
          @for (row of a.lines; track $index; let r = $index) {
            <div class="l">
              @for (s of row; track $index) {
                <span [class]="s.c" [style]="s.s">{{ s.t }}</span>
              }
              @if (r === a.cursorIndex) {
                <span class="cur" [style.left.ch]="a.cursorCol"></span>
              }
            </div>
          }
        </div>
      } @else {
        @if (snap().hasMore) {
          <button class="btn small ghost more" [disabled]="loadingMore()" (click)="loadMore(); $event.stopPropagation()">
            {{ (loadingMore() ? 'common.loading' : 'term.earlier') | t }}
          </button>
        }
        @for (row of earlier(); track $index) {
          <div class="l">
            @for (s of row; track $index) {
              <span [class]="s.c" [style]="s.s">{{ s.t }}</span>
            }
          </div>
        }
        @let n = normal();
        @for (row of n.lines; track $index; let r = $index) {
          <div class="l">
            @for (s of row; track $index) {
              <span [class]="s.c" [style]="s.s">{{ s.t }}</span>
            }
            @if (r === n.cursorIndex) {
              <span class="cur" [class.blur]="!hasFocus()" [style.left.ch]="n.cursorCol"></span>
            }
          </div>
        }
      }
    </div>
    <textarea
      class="catcher"
      #catcher
      rows="1"
      autocapitalize="off"
      autocomplete="off"
      autocorrect="off"
      spellcheck="false"
      enterkeyhint="send"
      [attr.aria-label]="'term.input' | t"
      [readOnly]="!canType()"
      (keydown)="onKeydown($event)"
      (input)="onInput($event)"
      (compositionend)="onCompositionEnd()"
      (paste)="onPaste($event)"
      (focus)="hasFocus.set(true)"
      (blur)="hasFocus.set(false)"
    ></textarea>
    @if (showKeys() && canType()) {
      <div class="keys" role="toolbar" [attr.aria-label]="'term.keyBar' | t" (pointerdown)="keepFocus($event)">
        <button class="btn small" (click)="key('Escape')">Esc</button>
        <button class="btn small" (click)="key('Tab')">Tab</button>
        <button class="btn small" [class.on]="sticky().ctrl" [attr.aria-pressed]="!!sticky().ctrl" (click)="toggleSticky('ctrl')">
          Ctrl
        </button>
        <button class="btn small" [class.on]="sticky().alt" [attr.aria-pressed]="!!sticky().alt" (click)="toggleSticky('alt')">
          Alt
        </button>
        <button class="btn small" (click)="key('Left')" [attr.aria-label]="'term.key.left' | t">←</button>
        <button class="btn small" (click)="key('Up')" [attr.aria-label]="'term.key.up' | t">↑</button>
        <button class="btn small" (click)="key('Down')" [attr.aria-label]="'term.key.down' | t">↓</button>
        <button class="btn small" (click)="key('Right')" [attr.aria-label]="'term.key.right' | t">→</button>
        <button class="btn small" (click)="text('|')">|</button>
        <button class="btn small" (click)="text('~')">~</button>
        <button class="btn small" (click)="text('/')">/</button>
        <button class="btn small" (click)="text('-')">-</button>
        <button class="btn small" (click)="pasteFromClipboard()" [shedTip]="'term.paste' | t">
          <shed-icon name="copy" [size]="14" />{{ 'term.paste' | t }}
        </button>
      </div>
    }
  `,
  styles: `
    :host {
      position: relative;
      display: flex;
      flex-direction: column;
      min-height: 0;
      min-width: 0;
      flex: 1 1 auto;
      background: var(--term-bg);
      color: var(--term-fg);
    }
    .head {
      display: flex;
      align-items: center;
      gap: 6px;
      min-height: 30px;
      padding: 0 4px 0 10px;
      font-size: 12px;
      color: var(--text-dim);
      border-bottom: 1px solid var(--divider);
      background: var(--surface);
    }
    .size {
      white-space: nowrap;
    }
    .screen {
      position: relative;
      flex: 1 1 auto;
      min-height: 0;
      overflow: auto;
      padding: 4px 6px;
      font-family: var(--mono);
      font-size: 13px;
      cursor: text;
      overscroll-behavior: contain;
    }
    .screen.mouse {
      cursor: default;
    }
    @media (max-width: 500px) {
      .screen {
        font-size: 12px;
      }
    }
    .measure {
      position: absolute;
      visibility: hidden;
      white-space: pre;
      line-height: 1.3em;
      display: inline-block;
      pointer-events: none;
    }
    .l {
      position: relative;
      height: 1.3em;
      line-height: 1.3em;
      white-space: pre;
      content-visibility: auto;
      contain-intrinsic-size: auto 1.3em;
    }
    .grid {
      width: max-content;
    }
    .grid .l {
      content-visibility: visible;
    }
    .cur {
      position: absolute;
      top: 0;
      width: 1ch;
      height: 1.3em;
      background: var(--term-cursor);
      opacity: 0.85;
      pointer-events: none;
    }
    .cur.blur {
      background: transparent;
      outline: 1px solid var(--term-cursor);
      outline-offset: -1px;
    }
    .b {
      font-weight: 700;
    }
    .d {
      opacity: 0.6;
    }
    .i {
      font-style: italic;
    }
    .u {
      text-decoration: underline;
    }
    .s {
      text-decoration: line-through;
    }
    .u.s {
      text-decoration: underline line-through;
    }
    .state {
      padding: 12px;
      color: var(--text-dim);
      font-family: var(--font);
    }
    .more {
      display: flex;
      margin: 2px auto 6px;
      font-family: var(--font);
    }
    .catcher {
      position: absolute;
      left: 0;
      bottom: 0;
      width: 1px;
      height: 1px;
      padding: 0;
      border: 0;
      opacity: 0;
      resize: none;
      overflow: hidden;
      font-size: 16px;
      color: transparent;
      background: transparent;
      caret-color: transparent;
    }
    .keys {
      display: flex;
      gap: 4px;
      padding: 6px 6px calc(6px + env(safe-area-inset-bottom));
      overflow-x: auto;
      border-top: 1px solid var(--divider);
      background: var(--surface);
      font-family: var(--font);
      flex: none;
    }
    .keys .btn {
      min-width: 40px;
      font-family: var(--mono);
    }
    @media (pointer: coarse) {
      .keys .btn {
        min-width: 44px;
        min-height: 40px;
      }
    }
  `,
})
export class TerminalView {
  readonly workspace = input.required<string>();
  readonly pane = input.required<Pane>();
  /** The view was clicked or focused: the tab makes it the active pane. */
  readonly focused = output<void>();

  private readonly store = inject(WorkspacesStore);
  private readonly notices = inject(Notices);
  private readonly clipboard = inject(Clipboard);
  private readonly screenEl = viewChild.required<ElementRef<HTMLElement>>('screen');
  private readonly measureEl = viewChild.required<ElementRef<HTMLElement>>('measure');
  private readonly catcher = viewChild.required<ElementRef<HTMLTextAreaElement>>('catcher');
  private readonly gridEl = viewChild<ElementRef<HTMLElement>>('grid');

  private model = new ScreenModel();
  protected readonly snap = signal<ScreenSnap>(this.snapshot());
  private readonly history = signal<ScreenLine[]>([]);
  protected readonly status = signal<'loading' | 'live' | 'paused' | 'ended'>('loading');
  protected readonly loadingMore = signal(false);
  protected readonly hasFocus = signal(false);
  protected readonly showKeys = signal(typeof matchMedia !== 'undefined' && matchMedia('(pointer: coarse)').matches);
  protected readonly sticky = signal<Mods>({});
  protected readonly holding = signal(false);

  private readonly cellW = signal(7.8);
  private readonly lineH = signal(17);
  private readonly basePx = signal(13);
  private readonly viewW = signal(0);
  private readonly viewH = signal(0);

  private readonly paneId = computed(() => this.pane().id);
  protected readonly online = computed(() => this.store.online(this.workspace()));
  protected readonly canType = computed(() => this.store.canChange(this.workspace()));
  protected readonly heldElsewhere = computed(() => !!this.pane().sizeHeld && !this.holding());
  protected readonly heading = computed(() => {
    const p = this.pane();
    const home = this.store.state(this.workspace())?.home ?? '';
    return p.tool || this.snap().title || p.title || tildify(p.cwd, home);
  });

  /** Columns this view shows on the normal screen. */
  private readonly width = computed(() =>
    Math.max(10, Math.floor((this.viewW() - PAD_X) / Math.max(1, this.cellW()))),
  );

  protected readonly earlier = computed<RSpan[][]>(() => {
    const h = this.history();
    if (!h.length) return [];
    const cols = untracked(this.snap).cols;
    return rewrap(joinRows(h, cols).lines, this.width()).map((v) => drawSpans(v.spans));
  });

  protected readonly normal = computed<Drawn>(() => {
    const s = this.snap();
    if (s.alt) return { lines: [], cursorIndex: -1, cursorCol: 0 };
    let last = -1;
    s.lines.forEach((l, i) => {
      if (!emptyRow(l)) last = i;
    });
    if (s.cursor.visible) last = Math.max(last, s.cursor.row);
    const rows = s.lines.slice(0, last + 1);
    const joined = joinRows(rows, s.cols);
    const cur = s.cursor.visible && s.cursor.row < rows.length;
    const line = cur ? joined.rowLine[s.cursor.row]! : -1;
    const offset = cur ? joined.rowStart[s.cursor.row]! + s.cursor.col : 0;
    const v = rewrap(joined.lines, this.width(), cur ? { line, cells: offset + 1 } : undefined);
    const at = cur ? locate(v, line, offset) : null;
    return { lines: v.map((x) => drawSpans(x.spans)), cursorIndex: at?.index ?? -1, cursorCol: at?.col ?? 0 };
  });

  protected readonly alt = computed<Drawn>(() => {
    const s = this.snap();
    if (!s.alt) return { lines: [], cursorIndex: -1, cursorCol: 0 };
    return {
      lines: s.lines.map((l) => drawSpans(l.spans)),
      cursorIndex: s.cursor.visible ? s.cursor.row : -1,
      cursorCol: s.cursor.col,
    };
  });

  /** The alternate screen's font size: the grid fits the width, down to 6 px. */
  protected readonly altFont = computed(() => {
    const base = this.basePx();
    const grid = this.snap().cols * this.cellW();
    const avail = this.viewW() - PAD_X;
    if (!grid || avail <= 0) return base;
    return Math.max(6, Math.min(base, (base * avail) / grid));
  });

  private stick = true;
  private anchor: number | null = null;
  private frame = 0;
  private queue: InputItem[] = [];
  private flushTimer: ReturnType<typeof setTimeout> | null = null;
  private sizeTimer: ReturnType<typeof setTimeout> | null = null;
  private failed = false;
  private readonly wheel = new WheelNotches();
  /** A button held down over the grid, sent as pressed. */
  private pressed: { button: MouseButton; cell: Cell; pointer: number } | null = null;
  /** A finger down over the grid: a tap is a click, a drag scrolls. */
  private touch: { x: number; y: number; pointer: number } | null = null;

  constructor() {
    const destroy = inject(DestroyRef);

    effect((onCleanup) => {
      const ws = this.workspace();
      const pane = this.paneId();
      untracked(() => {
        this.model = new ScreenModel();
        this.publish();
        this.status.set('loading');
        this.stick = true;
      });
      const sub = this.store.socket.subscribe(
        ws,
        { type: 'Screen', content: { pane } },
        {
          event: (e) => {
            if (e.type !== 'Screen') return;
            this.model.apply(e.content);
            if (this.status() !== 'live') this.status.set('live');
            this.schedule();
          },
          paused: () => this.status.set('paused'),
          ended: () => this.status.set('ended'),
        },
      );
      onCleanup(() => sub.close());
    });

    // Keep to the bottom as output comes, or in place after older lines came in.
    afterRenderEffect(() => {
      this.earlier();
      this.normal();
      this.alt();
      const el = this.screenEl().nativeElement;
      if (this.anchor !== null) {
        el.scrollTop = el.scrollHeight - this.anchor;
        this.anchor = null;
      } else if (this.stick) {
        el.scrollTop = el.scrollHeight;
      }
    });

    // While holding the size, follow this view's size.
    effect(() => {
      this.viewW();
      this.viewH();
      if (!untracked(this.holding)) return;
      if (this.sizeTimer) clearTimeout(this.sizeTimer);
      this.sizeTimer = setTimeout(() => this.takeSize(), 300);
    });

    afterNextRender(() => {
      const el = this.screenEl().nativeElement;
      const ro = new ResizeObserver(() => {
        this.measure();
        this.viewW.set(el.clientWidth);
        this.viewH.set(el.clientHeight);
      });
      ro.observe(el);
      void document.fonts?.ready.then(() => this.measure());
      this.resetCatcher();
      destroy.onDestroy(() => ro.disconnect());
    });

    destroy.onDestroy(() => {
      cancelAnimationFrame(this.frame);
      if (this.sizeTimer) clearTimeout(this.sizeTimer);
      this.flush();
      if (this.holding()) {
        void this.store.request(this.workspace(), { type: 'ReleaseSize', content: { pane: this.paneId() } }).catch(() => undefined);
      }
    });
  }

  /** Focuses the input (the tab does it for the active pane). */
  focus(): void {
    this.catcher().nativeElement.focus({ preventScroll: true });
  }

  /** Types into the pane, as if typed here. */
  send(items: InputItem[]): void {
    if (!this.canType() || !items.length) return;
    this.queue.push(...items);
    if (!this.flushTimer) this.flushTimer = setTimeout(() => this.flush(), 16);
    this.stick = true;
  }

  private snapshot(): ScreenSnap {
    const m = this.model;
    return {
      lines: m.lines.length ? m.lines : [BLANK],
      cols: m.cols,
      rows: m.rows,
      alt: m.alt,
      mouse: m.mouse,
      cursor: m.cursor,
      title: m.title,
      hasMore: m.ready && m.hasMore,
    };
  }

  private publish(): void {
    this.snap.set(this.snapshot());
    this.history.set(this.model.history);
  }

  private schedule(): void {
    if (this.frame) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      this.publish();
    });
  }

  private measure(): void {
    const m = this.measureEl().nativeElement.getBoundingClientRect();
    if (m.width > 0) this.cellW.set(m.width / 20);
    if (m.height > 0) this.lineH.set(m.height);
    const px = parseFloat(getComputedStyle(this.screenEl().nativeElement).fontSize);
    if (px > 0) this.basePx.set(px);
  }

  protected onScroll(): void {
    const el = this.screenEl().nativeElement;
    this.stick = el.scrollHeight - el.scrollTop - el.clientHeight < 8;
    if (el.scrollTop < 120 && this.snap().hasMore && !this.loadingMore()) void this.loadMore();
  }

  protected async loadMore(): Promise<void> {
    if (this.loadingMore() || !this.model.hasMore) return;
    this.loadingMore.set(true);
    const model = this.model;
    try {
      const lines = await this.store.call(
        this.workspace(),
        { type: 'Scrollback', content: { pane: this.paneId(), from: model.history.length, count: 200 } },
        'Lines',
      );
      if (model !== this.model) return;
      const el = this.screenEl().nativeElement;
      this.anchor = el.scrollHeight - el.scrollTop;
      this.stick = false;
      if (lines.length) model.prepend(lines);
      // Fewer than asked: that was all of it.
      if (lines.length < 200) model.scrollback = model.history.length;
      this.publish();
    } catch (e) {
      this.notices.error(e);
    } finally {
      this.loadingMore.set(false);
    }
  }

  protected onClick(): void {
    const sel = getSelection();
    if (sel && !sel.isCollapsed) return;
    this.focus();
  }

  /**
   * Clicks go to the program: it asked for the mouse, its grid is shown, we
   * may type, and Shift is not held (Shift selects text, as in Shed's window).
   */
  private mouseToProgram(e: MouseMods): boolean {
    const s = this.snap();
    return s.alt && s.mouse && this.canType() && !e.shiftKey;
  }

  /** The alternate screen's grid, where it is and how big its cells are now. */
  private geom(): GridGeom | null {
    const grid = this.gridEl()?.nativeElement;
    if (!grid) return null;
    const r = grid.getBoundingClientRect();
    const s = this.snap();
    const scale = this.altFont() / Math.max(1, this.basePx());
    return {
      left: r.left,
      top: r.top,
      cellW: this.cellW() * scale,
      cellH: s.rows > 0 && r.height > 0 ? r.height / s.rows : this.lineH() * scale,
      cols: s.cols,
      rows: s.rows,
    };
  }

  private cellOf(e: MouseEvent): Cell | null {
    const g = this.geom();
    return g ? cellAt(g, e.clientX, e.clientY) : null;
  }

  protected onMouseDown(e: MouseEvent): void {
    // The program gets the click: no text selection starts.
    if (this.mouseToProgram(e)) e.preventDefault();
  }

  protected onContextMenu(e: MouseEvent): void {
    if (this.mouseToProgram(e)) e.preventDefault();
  }

  protected onPointerDown(e: PointerEvent): void {
    if (!this.mouseToProgram(e)) return;
    if (e.pointerType === 'touch') {
      this.touch = { x: e.clientX, y: e.clientY, pointer: e.pointerId };
      return;
    }
    const button = buttonOf(e.button);
    const cell = this.cellOf(e);
    if (!button || !cell) return;
    (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
    this.pressed = { button, cell, pointer: e.pointerId };
    this.send([mouseItem('press', cell, e, button)]);
  }

  protected onPointerMove(e: PointerEvent): void {
    const p = this.pressed;
    if (!p || p.pointer !== e.pointerId) return;
    const cell = this.cellOf(e);
    if (!cell || (cell.col === p.cell.col && cell.row === p.cell.row)) return;
    p.cell = cell;
    this.send([mouseItem('move', cell, e, p.button)]);
  }

  protected onPointerUp(e: PointerEvent): void {
    const t = this.touch;
    if (t && t.pointer === e.pointerId) {
      this.touch = null;
      // A tap, not a drag: a click where it was.
      const cell = this.cellOf(e);
      if (cell && Math.hypot(e.clientX - t.x, e.clientY - t.y) < 10 && this.mouseToProgram(e)) {
        this.send([mouseItem('press', cell, e, 'left'), mouseItem('release', cell, e, 'left')]);
      }
      return;
    }
    const p = this.pressed;
    if (!p || p.pointer !== e.pointerId) return;
    this.pressed = null;
    this.send([mouseItem('release', this.cellOf(e) ?? p.cell, e, p.button)]);
  }

  protected onPointerCancel(e: PointerEvent): void {
    if (this.touch?.pointer === e.pointerId) this.touch = null;
    const p = this.pressed;
    if (!p || p.pointer !== e.pointerId) return;
    this.pressed = null;
    this.send([mouseItem('release', p.cell, e, p.button)]);
  }

  /**
   * The wheel on the alternate screen goes to the program in notches: as the
   * mouse when it asked for it, else the link makes arrow keys of it (less).
   * Shift keeps the browser's own scrolling, as does a grid taller than the
   * view when the program did not ask for the mouse.
   */
  protected onWheel(e: WheelEvent): void {
    const s = this.snap();
    if (!s.alt || !this.canType() || e.shiftKey || !e.deltaY) return;
    const el = this.screenEl().nativeElement;
    if (!s.mouse && el.scrollHeight > el.clientHeight + 1) return;
    e.preventDefault();
    const n = this.wheel.add(e.deltaY, e.deltaMode);
    if (!n) return;
    this.send(wheelItems(n, this.cellOf(e) ?? { col: 0, row: 0 }, e));
  }

  protected keepFocus(e: PointerEvent): void {
    // Buttons of the key bar must not take the focus (the phone keyboard would close).
    if ((e.target as HTMLElement).closest('button')) e.preventDefault();
  }

  protected onKeydown(e: KeyboardEvent): void {
    const clip = clipboardShortcut(e, MAC);
    if (clip === 'copy') {
      const text = getSelection()?.toString() ?? '';
      if (text) {
        e.preventDefault();
        void this.clipboard.copy(text);
      } else if (!MAC) {
        e.preventDefault();
      }
      return;
    }
    if (clip === 'paste') return; // the paste event follows
    if (!this.canType()) return;
    const sticky = this.sticky();
    const item = keyToInput(e, sticky);
    if (!item) return;
    e.preventDefault();
    this.clearSticky();
    this.send([item]);
  }

  protected onInput(e: Event): void {
    const ie = e as InputEvent;
    if (ie.isComposing) return;
    const el = this.catcher().nativeElement;
    const typed = el.value.startsWith(SENTINEL) ? el.value.slice(SENTINEL.length) : el.value;
    if (ie.inputType === 'deleteContentBackward' && !typed) {
      this.send([{ type: 'Key', content: { key: 'Backspace', ctrl: false, alt: false, shift: false } }]);
    } else if (ie.inputType === 'insertLineBreak' || ie.inputType === 'insertParagraph') {
      if (typed.replace(/\n/g, '')) this.typeText(typed.replace(/\n/g, ''));
      this.send([{ type: 'Key', content: { key: 'Enter', ctrl: false, alt: false, shift: false } }]);
    } else if (typed) {
      const parts = typed.split('\n');
      parts.forEach((p, i) => {
        if (p) this.typeText(p);
        if (i < parts.length - 1) this.send([{ type: 'Key', content: { key: 'Enter', ctrl: false, alt: false, shift: false } }]);
      });
    }
    this.resetCatcher();
  }

  protected onCompositionEnd(): void {
    const el = this.catcher().nativeElement;
    const typed = el.value.startsWith(SENTINEL) ? el.value.slice(SENTINEL.length) : el.value;
    if (typed) this.typeText(typed);
    this.resetCatcher();
  }

  protected onPaste(e: ClipboardEvent): void {
    e.preventDefault();
    const text = e.clipboardData?.getData('text/plain') ?? '';
    if (text) this.send([{ type: 'Paste', content: text }]);
  }

  protected async pasteFromClipboard(): Promise<void> {
    try {
      const text = await navigator.clipboard.readText();
      if (text) this.send([{ type: 'Paste', content: text }]);
    } catch (e) {
      this.notices.error(e);
    }
    this.focus();
  }

  protected key(name: string): void {
    const s = this.sticky();
    this.clearSticky();
    this.send([{ type: 'Key', content: { key: name, ctrl: !!s.ctrl, alt: !!s.alt, shift: false } }]);
  }

  protected text(ch: string): void {
    this.typeText(ch);
  }

  protected toggleSticky(which: 'ctrl' | 'alt'): void {
    this.sticky.update((s) => ({ ...s, [which]: !s[which] }));
    this.focus();
  }

  protected toggleSize(): void {
    if (this.holding()) {
      this.holding.set(false);
      void this.notices.run(this.store.request(this.workspace(), { type: 'ReleaseSize', content: { pane: this.paneId() } }));
    } else {
      this.holding.set(true);
      void this.takeSize();
    }
  }

  private async takeSize(): Promise<void> {
    this.measure();
    const el = this.screenEl().nativeElement;
    const cols = Math.max(20, Math.floor((el.clientWidth - PAD_X) / Math.max(1, this.cellW())));
    const rows = Math.max(5, Math.floor((el.clientHeight - PAD_Y) / Math.max(1, this.lineH())));
    try {
      await this.store.request(this.workspace(), { type: 'TakeSize', content: { pane: this.paneId(), cols, rows } });
    } catch (e) {
      this.holding.set(false);
      this.notices.error(e);
    }
  }

  private typeText(text: string): void {
    const s = this.sticky();
    this.clearSticky();
    this.send(textToInput(text, s));
  }

  private clearSticky(): void {
    if (this.sticky().ctrl || this.sticky().alt) this.sticky.set({});
  }

  private resetCatcher(): void {
    const el = this.catcher().nativeElement;
    el.value = SENTINEL;
    el.setSelectionRange(SENTINEL.length, SENTINEL.length);
  }

  private flush(): void {
    if (this.flushTimer) {
      clearTimeout(this.flushTimer);
      this.flushTimer = null;
    }
    if (!this.queue.length) return;
    const input = mergeInput(this.queue);
    this.queue = [];
    this.store
      .request(this.workspace(), { type: 'Input', content: { pane: this.paneId(), input } })
      .then(() => (this.failed = false))
      .catch((e: unknown) => {
        // One notice per run of failures, not one per key.
        if (!this.failed) this.notices.error(e);
        this.failed = true;
      });
  }
}

