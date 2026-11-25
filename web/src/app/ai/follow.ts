import { signal } from '@angular/core';
import type { AiEntry } from '../../api/gen/api';

// How the conversation scrolls: it opens at the latest entry and keeps to it
// while new output comes, until you scroll up; a jump goes back down. As the
// phone app does (android/…/ui/ai/Follow.kt); pure, so tested on its own.

/** How close to the end still counts as at the end, in pixels. */
export const END_SLACK = 40;

/**
 * Whether the transcript keeps to its latest entry. It does from the start;
 * scrolling away by hand stops it, and coming back to the end (by hand or
 * with the jump) starts it again.
 */
export class Follow {
  /** New output scrolls the transcript to its end. */
  readonly following = signal(true);
  /** A finger, the wheel or the scrollbar moves the list: not to be scrolled under it. */
  readonly touched = signal(false);

  /** The user started moving the list. */
  dragged(): void {
    this.touched.set(true);
  }

  /** The list stopped moving, at its end or not. */
  settled(atEnd: boolean): void {
    if (this.touched()) {
      this.touched.set(false);
      this.following.set(atEnd);
    } else if (atEnd) {
      this.following.set(true);
    }
  }

  /** Jumped to the latest entry. */
  toLatest(): void {
    this.touched.set(false);
    this.following.set(true);
  }

  /** Jumped somewhere else (to one of your messages). */
  away(): void {
    this.touched.set(false);
    this.following.set(false);
  }

  /** Back to the start (another conversation): following. */
  reset(): void {
    this.toLatest();
  }

  /** Whether to scroll to the end now: following, untouched, and something below. */
  shouldScroll(moreBelow: boolean): boolean {
    return this.following() && !this.touched() && moreBelow;
  }

  /** Whether to offer the jump to the latest entry. */
  showJump(moreBelow: boolean): boolean {
    return !this.following() && moreBelow;
  }
}

/** How far below what shows the list goes on, in pixels (0: at the end). */
export function below(el: { scrollHeight: number; scrollTop: number; clientHeight: number }): number {
  return Math.max(0, el.scrollHeight - el.scrollTop - el.clientHeight);
}

/** Whether a scrolled list is at its end (within `slack` pixels). */
export function atEnd(el: { scrollHeight: number; scrollTop: number; clientHeight: number }, slack = END_SLACK): boolean {
  return below(el) <= slack;
}

/** Where your messages are in the conversation. */
export function promptIndices(entries: AiEntry[]): number[] {
  const out: number[] = [];
  entries.forEach((e, i) => {
    if (e?.type === 'User') out.push(i);
  });
  return out;
}

/** An entry as drawn: its index and its top and bottom, in the list's coordinates. */
export interface EntryBox {
  index: number;
  top: number;
  bottom: number;
}

/** What is at the top of the list. */
export interface TopEntry {
  /** The first entry showing (its bottom below the top). */
  top: number;
  /** Scrolled into it: its start is above the top. */
  inside: boolean;
}

/** The entry at the top of a list scrolled to `viewTop`; `null`: none drawn. */
export function topEntry(boxes: EntryBox[], viewTop: number, slack = 4): TopEntry | null {
  const b = boxes.find((x) => x.bottom > viewTop + slack);
  if (!b) return null;
  return { top: b.index, inside: b.top < viewTop - slack };
}

/**
 * The message of yours before the entry at the top (`top`); scrolled into a
 * message (`inside`), its own start first. `null`: none above.
 */
export function previousPrompt(entries: AiEntry[], top: number, inside: boolean): number | null {
  const all = promptIndices(entries).filter((i) => i < top || (i === top && inside));
  return all.length ? all[all.length - 1]! : null;
}

/** The message of yours after the entry at the top; `null`: none below (the jump goes to the end). */
export function nextPrompt(entries: AiEntry[], top: number): number | null {
  return promptIndices(entries).find((i) => i > top) ?? null;
}
