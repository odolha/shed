/**
 * Cutting a line of text into styled pieces, from ranges: code colours
 * (`Highlight`), matched characters in quick open, search hits.
 *
 * Positions are UTF-16 code units, as the API gives them and as JavaScript
 * strings index (an emoji counts two), so they go straight to `slice`. A cut
 * never splits an emoji's two halves.
 */

import type { Highlight } from '../../api/gen/api';

export interface Segment {
  text: string;
  /** The CSS class, or null for plain text. */
  cls: string | null;
}

export interface Mark {
  start: number;
  end: number;
  cls: string;
}

/** Pieces of `text`; overlapping marks: the one starting first wins. */
export function segment(text: string, marks: readonly Mark[]): Segment[] {
  if (!marks.length) return text ? [{ text, cls: null }] : [];
  const sorted = marks.filter((m) => m.end > m.start).sort((a, b) => a.start - b.start || b.end - a.end);
  const out: Segment[] = [];
  let pos = 0;
  for (const m of sorted) {
    const s = Math.max(snapStart(text, m.start), pos);
    const e = snapEnd(text, Math.min(m.end, text.length));
    if (e <= s) continue;
    if (s > pos) out.push({ text: text.slice(pos, s), cls: null });
    out.push({ text: text.slice(s, e), cls: m.cls });
    pos = e;
  }
  if (pos < text.length) out.push({ text: text.slice(pos), cls: null });
  return out;
}

/** The second half of a surrogate pair (an emoji's), at `i`. */
function lowHalf(text: string, i: number): boolean {
  const c = text.charCodeAt(i);
  const before = text.charCodeAt(i - 1);
  return i > 0 && c >= 0xdc00 && c <= 0xdfff && before >= 0xd800 && before <= 0xdbff;
}

/** A start inside a pair moves back to its first half. */
function snapStart(text: string, i: number): number {
  return lowHalf(text, i) ? i - 1 : i;
}

/** An end inside a pair moves on past its second half. */
function snapEnd(text: string, i: number): number {
  return i < text.length && lowHalf(text, i) ? i + 1 : i;
}

/** A cut at `i` that keeps pairs whole (for trimming long lines). */
export function safeCut(text: string, i: number): number {
  return snapStart(text, Math.max(0, Math.min(i, text.length)));
}

/** `string.special` → `hl-string.special`; anything odd is dropped. */
export function kindClass(kind: string): string | null {
  return /^[a-z][a-z0-9._-]*$/i.test(kind) ? `hl-${kind.toLowerCase()}` : null;
}

/** A line of code coloured by its highlights. */
export function highlightLine(text: string, highlights: readonly Highlight[] | undefined): Segment[] {
  const marks: Mark[] = [];
  for (const h of highlights ?? []) {
    const cls = kindClass(h.kind);
    if (cls) marks.push({ start: h.start, end: h.end, cls });
  }
  return segment(text, marks);
}

/** Single positions (quick open's `hits`) as runs of marks. */
export function positionMarks(positions: readonly number[], cls: string): Mark[] {
  const sorted = [...positions].sort((a, b) => a - b);
  const marks: Mark[] = [];
  for (const p of sorted) {
    const last = marks[marks.length - 1];
    if (last && last.end === p) last.end = p + 1;
    else if (!last || p >= last.end) marks.push({ start: p, end: p + 1, cls });
  }
  return marks;
}

/** Ranges as `start, end, start, end…` (search hits) as marks. */
export function rangeMarks(ranges: readonly number[], cls: string): Mark[] {
  const marks: Mark[] = [];
  for (let i = 0; i + 1 < ranges.length; i += 2) {
    marks.push({ start: ranges[i]!, end: ranges[i + 1]!, cls });
  }
  return marks;
}

/** `src/app/main.rs` → [`src/app/`, `main.rs`]. */
export function splitPath(path: string): [string, string] {
  const trimmed = path.endsWith('/') ? path.slice(0, -1) : path;
  const i = trimmed.lastIndexOf('/');
  return i < 0 ? ['', trimmed] : [trimmed.slice(0, i + 1), trimmed.slice(i + 1)];
}

/** The folder holding a path, relative to the root (`''` for the root). */
export function parentOf(path: string): string {
  const trimmed = path.endsWith('/') ? path.slice(0, -1) : path;
  const i = trimmed.lastIndexOf('/');
  return i < 0 ? '' : trimmed.slice(0, i);
}

/** `1536` → `1.5 KB`. */
export function fileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB'];
  let v = bytes / 1024;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u++;
  }
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[u]}`;
}
