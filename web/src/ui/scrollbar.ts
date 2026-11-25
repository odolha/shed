/**
 * A thin scrollbar over a scrolled element, for where the browser shows none
 * (phones) or one too thin to grab: it shows while the element scrolls (and
 * on hover), and its thumb can be dragged. The element's own scrollbar is
 * hidden (`.no-scrollbar`).
 *
 *   <div class="wrap" style="position: relative">
 *     <div class="scroll no-scrollbar" #list>…</div>
 *     <shed-scrollbar [for]="list" />
 *   </div>
 */

import {
  ChangeDetectionStrategy,
  Component,
  DestroyRef,
  ElementRef,
  effect,
  inject,
  input,
  output,
  signal,
} from '@angular/core';

/** The shortest a thumb gets, in pixels, so a long list's stays grabbable. */
export const MIN_THUMB = 32;

/** What a scrolled element measures. */
export interface Scrolled {
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
}

/** The thumb on a track `track` pixels high: its top and height; `null` when nothing scrolls. */
export function thumbFor(s: Scrolled, track: number, min = MIN_THUMB): { top: number; height: number } | null {
  const range = s.scrollHeight - s.clientHeight;
  if (range <= 1 || track <= 0 || s.scrollHeight <= 0) return null;
  const height = Math.min(track, Math.max(min, (s.clientHeight / s.scrollHeight) * track));
  const frac = Math.max(0, Math.min(1, s.scrollTop / range));
  return { top: frac * (track - height), height };
}

/** Where the element scrolls to for the thumb's top at `thumbTop`. */
export function scrollTopFor(thumbTop: number, s: Scrolled, track: number, thumb: number): number {
  const range = s.scrollHeight - s.clientHeight;
  const room = track - thumb;
  if (range <= 0 || room <= 0) return 0;
  return Math.max(0, Math.min(range, (thumbTop / room) * range));
}

const SHOW_FOR = 1200;

@Component({
  selector: 'shed-scrollbar',
  changeDetection: ChangeDetectionStrategy.OnPush,
  host: {
    '[class.shown]': 'shown() || dragging()',
    '[class.dragging]': 'dragging()',
    '[class.none]': 'thumb() === null',
    'aria-hidden': 'true',
    '(pointerenter)': 'hover(true)',
    '(pointerleave)': 'hover(false)',
  },
  template: `
    @if (thumb(); as th) {
      <div
        class="thumb"
        [style.top.px]="th.top"
        [style.height.px]="th.height"
        (pointerdown)="down($event)"
        (pointermove)="move($event)"
        (pointerup)="up($event)"
        (pointercancel)="up($event)"
      >
        <i></i>
      </div>
    }
  `,
  styles: `
    :host {
      position: absolute;
      top: 2px;
      right: 0;
      bottom: 2px;
      width: 14px;
      z-index: 3;
      pointer-events: none;
    }
    :host(.none) {
      display: none;
    }
    .thumb {
      position: absolute;
      right: 0;
      width: 14px;
      display: flex;
      justify-content: flex-end;
      padding-right: 3px;
      pointer-events: none;
      touch-action: none;
      cursor: pointer;
      opacity: 0;
      transition: opacity 0.25s;
    }
    .thumb i {
      display: block;
      width: 4px;
      height: 100%;
      border-radius: 3px;
      background: color-mix(in srgb, var(--text) 38%, transparent);
      transition: width 0.12s;
    }
    :host(.shown) .thumb {
      opacity: 1;
      pointer-events: auto;
    }
    :host(.dragging) .thumb i,
    .thumb:hover i {
      width: 7px;
      background: color-mix(in srgb, var(--text) 55%, transparent);
    }
    @media (pointer: coarse) {
      :host,
      .thumb {
        width: 28px;
      }
      .thumb i {
        width: 5px;
      }
      :host(.dragging) .thumb i {
        width: 8px;
      }
    }
  `,
})
export class Scrollbar {
  /** The scrolled element. */
  readonly for = input.required<HTMLElement>();
  /** The thumb was taken (true) and let go (false). */
  readonly drag = output<boolean>();

  private readonly host = inject<ElementRef<HTMLElement>>(ElementRef);
  protected readonly thumb = signal<{ top: number; height: number } | null>(null);
  protected readonly shown = signal(false);
  protected readonly dragging = signal(false);
  private hideTimer: ReturnType<typeof setTimeout> | null = null;
  private grab: { y: number; top: number } | null = null;
  private hovered = false;
  private frame = 0;

  constructor() {
    effect((onCleanup) => {
      const el = this.for();
      const onScroll = () => {
        this.measure();
        this.flash();
      };
      el.addEventListener('scroll', onScroll, { passive: true });
      let ro: ResizeObserver | null = null;
      if (typeof ResizeObserver !== 'undefined') {
        ro = new ResizeObserver(() => this.measure());
        ro.observe(el);
        if (el.firstElementChild) ro.observe(el.firstElementChild);
      }
      this.measure();
      onCleanup(() => {
        el.removeEventListener('scroll', onScroll);
        ro?.disconnect();
      });
    });
    inject(DestroyRef).onDestroy(() => {
      if (this.hideTimer) clearTimeout(this.hideTimer);
      cancelAnimationFrame(this.frame);
    });
  }

  private measure(): void {
    cancelAnimationFrame(this.frame);
    this.frame = requestAnimationFrame(() => {
      const el = this.for();
      this.thumb.set(thumbFor(el, this.host.nativeElement.clientHeight));
    });
  }

  /** Shows the bar for a while (it scrolled). */
  private flash(): void {
    this.shown.set(true);
    if (this.hideTimer) clearTimeout(this.hideTimer);
    this.hideTimer = setTimeout(() => {
      if (!this.dragging() && !this.hovered) this.shown.set(false);
    }, SHOW_FOR);
  }

  protected hover(on: boolean): void {
    this.hovered = on;
    if (!on) this.flash();
  }

  protected down(e: PointerEvent): void {
    const th = this.thumb();
    if (!th) return;
    e.preventDefault();
    e.stopPropagation();
    (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
    this.grab = { y: e.clientY, top: th.top };
    this.dragging.set(true);
    this.drag.emit(true);
  }

  protected move(e: PointerEvent): void {
    const g = this.grab;
    const th = this.thumb();
    if (!g || !th) return;
    e.preventDefault();
    const el = this.for();
    const track = this.host.nativeElement.clientHeight;
    const top = Math.max(0, Math.min(track - th.height, g.top + e.clientY - g.y));
    el.scrollTop = scrollTopFor(top, el, track, th.height);
  }

  protected up(e: PointerEvent): void {
    if (!this.grab) return;
    (e.currentTarget as HTMLElement).releasePointerCapture?.(e.pointerId);
    this.grab = null;
    this.dragging.set(false);
    this.drag.emit(false);
    this.flash();
  }
}
