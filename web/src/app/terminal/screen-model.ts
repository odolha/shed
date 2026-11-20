/**
 * A terminal pane's screen as the link sends it (`Screen` events), and the
 * scrollback this viewer has: lines that scrolled off while watching, and
 * those fetched with `Scrollback`. The history is always contiguous with the
 * screen's top: its last line is the one just above the screen.
 *
 * Arrays are replaced, never changed in place, so views can compare them.
 */

import type { Cursor, Screen, ScreenLine } from '../../api/gen/api';

export const BLANK: ScreenLine = { spans: [], wraps: false };

export class ScreenModel {
  cols = 80;
  rows = 24;
  alt = false;
  mouse = false;
  title: string | null = null;
  cursor: Cursor = { row: 0, col: 0, visible: false };
  /** Lines of scrollback the workspace has. */
  scrollback = 0;
  lines: ScreenLine[] = [];
  /** Oldest first; ends just above the screen. */
  history: ScreenLine[] = [];
  /** Any event came. */
  ready = false;

  constructor(readonly maxHistory = 5000) {}

  /** More scrollback can be fetched. */
  get hasMore(): boolean {
    return !this.alt && this.history.length < Math.min(this.scrollback, this.maxHistory);
  }

  apply(s: Screen): void {
    const prev = this.lines;
    const resized = s.cols !== this.cols && this.ready;
    if (s.scrolled && !s.alt && !this.alt) {
      if (s.scrolled > prev.length) {
        // Lines went by that nobody saw: what we have no longer joins on.
        this.history = [];
      } else {
        this.pushHistory(prev.slice(0, s.scrolled));
      }
    }
    if (resized && !s.alt) {
      // The emulator reflows its scrollback on a resize: fetch it again.
      this.history = [];
    }
    let lines: ScreenLine[];
    if (s.full || !this.ready) {
      lines = new Array<ScreenLine>(s.rows).fill(BLANK);
    } else {
      lines = prev.slice(0, s.rows);
      while (lines.length < s.rows) lines.push(BLANK);
    }
    for (const r of s.lines) {
      if (r.row < s.rows) lines[r.row] = r.line;
    }
    this.lines = lines;
    this.cols = s.cols;
    this.rows = s.rows;
    this.alt = !!s.alt;
    this.mouse = !!s.mouse;
    this.cursor = s.cursor;
    this.title = s.title ?? null;
    this.scrollback = s.scrollback ?? 0;
    this.ready = true;
  }

  /** Lines fetched from further back (oldest first), put before what we have. */
  prepend(lines: ScreenLine[]): void {
    if (!lines.length) return;
    const room = Math.max(0, this.maxHistory - this.history.length);
    this.history = [...lines.slice(Math.max(0, lines.length - room)), ...this.history];
  }

  private pushHistory(lines: ScreenLine[]): void {
    const next = [...this.history, ...lines];
    // Over the cap, the oldest go; they can be fetched again.
    this.history = next.length > this.maxHistory ? next.slice(next.length - this.maxHistory) : next;
  }
}
