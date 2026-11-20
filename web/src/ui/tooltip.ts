import { Directive, ElementRef, OnDestroy, inject, input } from '@angular/core';

let tip: HTMLElement | null = null;
let nextId = 1;

function tipElement(): HTMLElement {
  if (!tip) {
    tip = document.createElement('div');
    tip.className = 'tip';
    tip.setAttribute('popover', 'manual');
    tip.setAttribute('role', 'tooltip');
    tip.id = 'shed-tip';
    document.body.appendChild(tip);
  }
  return tip;
}

/**
 * A tooltip on hover and keyboard focus: `[shedTip]="'Copy'"`. It also names
 * the element for screen readers when it has no text of its own.
 */
@Directive({
  selector: '[shedTip]',
  host: {
    '(mouseenter)': 'show()',
    '(mouseleave)': 'hide()',
    '(focusin)': 'show()',
    '(focusout)': 'hide()',
    '(click)': 'hide()',
    '[attr.aria-label]': 'label()',
  },
})
export class Tooltip implements OnDestroy {
  readonly shedTip = input<string | null | undefined>('');
  private readonly el = inject<ElementRef<HTMLElement>>(ElementRef);
  private timer: ReturnType<typeof setTimeout> | null = null;
  private readonly id = nextId++;

  protected label(): string | null {
    const host = this.el.nativeElement;
    if (host.hasAttribute('aria-label') && host.getAttribute('aria-label') !== this.shedTip()) {
      return host.getAttribute('aria-label');
    }
    return host.textContent?.trim() ? null : (this.shedTip() ?? null);
  }

  protected show(): void {
    const text = this.shedTip();
    if (!text || matchMedia('(pointer: coarse)').matches) return;
    if (this.timer) clearTimeout(this.timer);
    this.timer = setTimeout(() => {
      const t = tipElement();
      if (!t.showPopover) return;
      t.textContent = text;
      t.dataset['owner'] = String(this.id);
      if (!t.matches(':popover-open')) t.showPopover();
      const r = this.el.nativeElement.getBoundingClientRect();
      const m = t.getBoundingClientRect();
      let x = r.left + r.width / 2 - m.width / 2;
      x = Math.max(6, Math.min(x, window.innerWidth - m.width - 6));
      let y = r.bottom + 6;
      if (y + m.height > window.innerHeight - 6) y = r.top - m.height - 6;
      t.style.left = `${Math.round(x)}px`;
      t.style.top = `${Math.round(y)}px`;
    }, 450);
  }

  protected hide(): void {
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    if (tip && tip.dataset['owner'] === String(this.id) && tip.matches(':popover-open')) tip.hidePopover();
  }

  ngOnDestroy(): void {
    this.hide();
  }
}
