/**
 * The part of the page that shows, with a phone's on-screen keyboard: from
 * `visualViewport`, as CSS variables on the root (`--vvh`: the height that
 * shows; `--kb`: what the keyboard covers of the layout, 0 where the browser
 * resizes the page itself) and as signals (`keyboard`: whether it is up).
 */

import { Injectable, type Signal, signal } from '@angular/core';

/** The keyboard counts as up when it takes this many pixels or more. */
const KEYBOARD = 120;

/** Whether a keyboard is up: what shows is well short of the most seen at this width. */
export function keyboardUp(tallest: number, height: number): boolean {
  return tallest - height >= KEYBOARD;
}

/** What the keyboard covers of a layout `inner` pixels high (0 when the layout shrank with it). */
export function keyboardInset(inner: number, height: number, offsetTop: number): number {
  return Math.max(0, Math.round(inner - height - offsetTop));
}

@Injectable({ providedIn: 'root' })
export class Viewport {
  /** The height that shows, in CSS pixels. */
  readonly height = signal(typeof window === 'undefined' ? 0 : window.innerHeight);
  /** An on-screen keyboard is up. */
  readonly keyboard = signal(false);
  private tallest = 0;
  private width = 0;
  private started = false;

  /** Follows the viewport from now on (once, from the app's shell). */
  start(): void {
    if (this.started || typeof window === 'undefined') return;
    this.started = true;
    const vv = window.visualViewport;
    const update = () => {
      // Pinch zoom changes what shows, not the room there is: left as it was.
      if (vv && Math.abs(vv.scale - 1) > 0.01) return;
      const h = vv?.height ?? window.innerHeight;
      const w = vv?.width ?? window.innerWidth;
      if (Math.abs(w - this.width) > 1) {
        // Turned, or resized: what is tallest starts again.
        this.width = w;
        this.tallest = 0;
      }
      this.tallest = Math.max(this.tallest, h, window.innerHeight);
      const root = document.documentElement.style;
      root.setProperty('--vvh', `${Math.round(h)}px`);
      root.setProperty('--kb', `${keyboardInset(window.innerHeight, h, vv?.offsetTop ?? 0)}px`);
      this.height.set(h);
      this.keyboard.set(keyboardUp(this.tallest, h));
    };
    vv?.addEventListener('resize', update);
    vv?.addEventListener('scroll', update);
    window.addEventListener('resize', update);
    update();
  }
}

const queries = new Map<string, Signal<boolean>>();

/** A media query as a signal, kept up to date for the page's life (one per query). */
export function mediaSignal(query: string): Signal<boolean> {
  let s = queries.get(query);
  if (!s) {
    const mq = typeof matchMedia === 'function' ? matchMedia(query) : null;
    const w = signal(mq?.matches ?? false);
    mq?.addEventListener('change', (e) => w.set(e.matches));
    s = w.asReadonly();
    queries.set(query, s);
  }
  return s;
}

/** The phone layout: what is folded and where on narrow screens. */
export const NARROW = '(max-width: 699px)';
