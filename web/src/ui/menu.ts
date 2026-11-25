/**
 * Menus on the browser's popover layer: light dismiss and Escape for free.
 *
 *   <button class="icon-btn" [shedMenu]="m" aria-label="More">…</button>
 *   <shed-menu #m>
 *     <button class="menu-item" (click)="rename()">…</button>
 *   </shed-menu>
 *
 * Clicking a `.menu-item` closes the menu (unless it has `data-keep`).
 */

import {
  ChangeDetectionStrategy,
  Component,
  Directive,
  ElementRef,
  inject,
  input,
  output,
  signal,
} from '@angular/core';

@Component({
  selector: 'shed-menu',
  changeDetection: ChangeDetectionStrategy.OnPush,
  host: {
    popover: 'auto',
    class: 'menu',
    role: 'menu',
    '(toggle)': 'onToggle($event)',
    '(click)': 'onClick($event)',
    '(keydown)': 'onKey($event)',
  },
  template: `<ng-content />`,
})
export class Menu {
  private readonly host = inject<ElementRef<HTMLElement>>(ElementRef);
  private anchor: HTMLElement | null = null;
  readonly isOpen = signal(false);
  readonly opened = output<void>();
  readonly closed = output<void>();

  toggle(anchor: HTMLElement): void {
    if (this.isOpen()) this.close();
    else this.open(anchor);
  }

  open(anchor: HTMLElement): void {
    this.anchor = anchor;
    const el = this.host.nativeElement;
    if (!el.showPopover) return;
    if (!el.matches(':popover-open')) el.showPopover();
    const r = anchor.getBoundingClientRect();
    this.place(r.left, r.bottom + 4, r.right, r.top - 4);
    this.focusFirst();
  }

  /** At a point (a right-click or long press). */
  openAt(x: number, y: number, from?: HTMLElement): void {
    this.anchor = from ?? null;
    const el = this.host.nativeElement;
    if (!el.showPopover) return;
    if (!el.matches(':popover-open')) el.showPopover();
    this.place(x, y, x, y);
    this.focusFirst();
  }

  close(): void {
    const el = this.host.nativeElement;
    if (el.hidePopover && el.matches(':popover-open')) el.hidePopover();
  }

  private place(left: number, top: number, right: number, above: number): void {
    const el = this.host.nativeElement;
    el.style.left = '0px';
    el.style.top = '0px';
    const m = el.getBoundingClientRect();
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    let x = left;
    if (x + m.width > vw - 8) x = Math.max(8, right - m.width);
    let y = top;
    if (y + m.height > vh - 8 && above - m.height >= 8) y = above - m.height;
    else if (y + m.height > vh - 8) y = Math.max(8, vh - 8 - m.height);
    el.style.left = `${Math.round(x)}px`;
    el.style.top = `${Math.round(y)}px`;
  }

  private items(): HTMLElement[] {
    return Array.from(this.host.nativeElement.querySelectorAll<HTMLElement>('.menu-item:not(:disabled)'));
  }

  private focusFirst(): void {
    queueMicrotask(() => this.items()[0]?.focus({ preventScroll: true }));
  }

  protected onToggle(e: Event): void {
    const open = (e as ToggleEvent).newState === 'open';
    this.isOpen.set(open);
    if (open) this.opened.emit();
    else {
      this.closed.emit();
      if (this.anchor && this.host.nativeElement.contains(document.activeElement)) {
        this.anchor.focus({ preventScroll: true });
      }
    }
  }

  protected onClick(e: MouseEvent): void {
    const item = (e.target as HTMLElement).closest('.menu-item');
    if (item && !item.hasAttribute('data-keep') && !(item as HTMLButtonElement).disabled) this.close();
  }

  protected onKey(e: KeyboardEvent): void {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    const items = this.items();
    if (!items.length) return;
    e.preventDefault();
    const i = items.indexOf(document.activeElement as HTMLElement);
    const next = e.key === 'ArrowDown' ? (i + 1) % items.length : (i - 1 + items.length) % items.length;
    items[next]?.focus();
  }
}

/** Opens a `shed-menu` from a button. */
@Directive({
  selector: '[shedMenu]',
  host: {
    'aria-haspopup': 'menu',
    '[attr.aria-expanded]': 'shedMenu().isOpen()',
    '(click)': 'onClick($event)',
  },
})
export class MenuTrigger {
  readonly shedMenu = input.required<Menu>();
  private readonly el = inject<ElementRef<HTMLElement>>(ElementRef);

  protected onClick(e: MouseEvent): void {
    e.stopPropagation();
    this.shedMenu().toggle(this.el.nativeElement);
  }
}

/** How long a press is long, in ms (Android's own long press comes first, as `contextmenu`). */
const LONG_PRESS = 550;

/**
 * Opens a `shed-menu` on right-click, and on a long press on touch screens:
 * Android sends that as `contextmenu`; iOS does not, so a press held still
 * this long opens it too (and the tap it ends in is dropped).
 */
@Directive({
  selector: '[shedContextMenu]',
  host: {
    '(contextmenu)': 'onContext($event)',
    '(touchstart)': 'onTouchStart($event)',
    '(touchmove)': 'onTouchMove($event)',
    '(touchend)': 'onTouchEnd($event)',
    '(touchcancel)': 'cancel()',
  },
})
export class ContextMenuTrigger {
  readonly shedContextMenu = input.required<Menu | null | undefined>();
  private readonly el = inject<ElementRef<HTMLElement>>(ElementRef);
  private timer: ReturnType<typeof setTimeout> | null = null;
  private start: { x: number; y: number } | null = null;
  private openedAt = 0;

  constructor() {
    // No link preview or text callout on iOS where a long press has a menu.
    this.el.nativeElement.style.setProperty('-webkit-touch-callout', 'none');
  }

  protected onContext(e: MouseEvent): void {
    const menu = this.shedContextMenu();
    if (!menu) return;
    e.preventDefault();
    this.cancel();
    // Opened by the long press already.
    if (Date.now() - this.openedAt < 1000) return;
    menu.openAt(e.clientX, e.clientY, this.el.nativeElement);
  }

  protected onTouchStart(e: TouchEvent): void {
    this.cancel();
    const t = e.touches[0];
    if (!this.shedContextMenu() || e.touches.length !== 1 || !t) return;
    this.start = { x: t.clientX, y: t.clientY };
    this.timer = setTimeout(() => {
      this.timer = null;
      const menu = this.shedContextMenu();
      if (!menu || !this.start) return;
      this.openedAt = Date.now();
      menu.openAt(this.start.x, this.start.y, this.el.nativeElement);
    }, LONG_PRESS);
  }

  protected onTouchMove(e: TouchEvent): void {
    const t = e.touches[0];
    if (!this.start || !t) return;
    if (Math.hypot(t.clientX - this.start.x, t.clientY - this.start.y) > 10) this.cancel();
  }

  protected onTouchEnd(e: TouchEvent): void {
    this.cancel();
    // The press opened the menu: its end is not a tap on what is under it.
    if (Date.now() - this.openedAt < 1000 && e.cancelable) e.preventDefault();
  }

  protected cancel(): void {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    this.start = null;
  }
}
