/**
 * Re-wrapping a terminal's normal screen to the viewer's width: rows joined
 * into logical lines where they wrap, then cut again at the new width, each
 * piece keeping its span's style.
 */

import type { ScreenLine, Span } from '../../api/gen/api';

/** A line as drawn: spans that fit `width` cells. */
export interface VLine {
  spans: Span[];
  /** The logical line it is part of. */
  line: number;
  /** Its first cell's offset in that logical line. */
  start: number;
  cells: number;
}

/** How many cells a code point takes: 0 (combining), 1 or 2 (wide). */
export function cellWidth(cp: number): 0 | 1 | 2 {
  if (cp < 0x300) return 1;
  if (
    (cp >= 0x300 && cp <= 0x36f) ||
    (cp >= 0x200b && cp <= 0x200f) ||
    (cp >= 0xfe00 && cp <= 0xfe0f) ||
    (cp >= 0x1ab0 && cp <= 0x1aff) ||
    (cp >= 0x20d0 && cp <= 0x20ff) ||
    (cp >= 0xe0100 && cp <= 0xe01ef)
  ) {
    return 0;
  }
  if (
    (cp >= 0x1100 && cp <= 0x115f) ||
    (cp >= 0x2e80 && cp <= 0x303e) ||
    (cp >= 0x3041 && cp <= 0x33ff) ||
    (cp >= 0x3400 && cp <= 0x4dbf) ||
    (cp >= 0x4e00 && cp <= 0x9fff) ||
    (cp >= 0xa000 && cp <= 0xa4cf) ||
    (cp >= 0xac00 && cp <= 0xd7a3) ||
    (cp >= 0xf900 && cp <= 0xfaff) ||
    (cp >= 0xfe30 && cp <= 0xfe4f) ||
    (cp >= 0xff00 && cp <= 0xff60) ||
    (cp >= 0xffe0 && cp <= 0xffe6) ||
    (cp >= 0x1f300 && cp <= 0x1f64f) ||
    (cp >= 0x1f900 && cp <= 0x1f9ff) ||
    (cp >= 0x1fa70 && cp <= 0x1faff) ||
    (cp >= 0x20000 && cp <= 0x3fffd)
  ) {
    return 2;
  }
  return 1;
}

const NARROW = /^[\x20-\x7e]*$/;

/** How many cells a text takes. */
export function textCells(text: string): number {
  if (NARROW.test(text)) return text.length;
  let n = 0;
  for (const ch of text) n += cellWidth(ch.codePointAt(0)!);
  return n;
}

function lineCells(spans: Span[]): number {
  let n = 0;
  for (const s of spans) n += textCells(s.text);
  return n;
}

function plain(text: string): Span {
  return { text };
}

/** Whitespace nobody would see: trailing spaces with no background. */
function invisible(s: Span): boolean {
  return !s.bg && !s.inverse && !s.underline && !s.strike;
}

function trimEnd(spans: Span[]): Span[] {
  const out = spans.slice();
  while (out.length) {
    const last = out[out.length - 1]!;
    if (!invisible(last)) break;
    const text = last.text.replace(/\s+$/, '');
    if (text) {
      out[out.length - 1] = { ...last, text };
      break;
    }
    out.pop();
  }
  return out;
}

export interface Joined {
  /** Logical lines, trailing blanks trimmed. */
  lines: Span[][];
  /** Per row: the logical line it is in, and the cell it starts at there. */
  rowLine: number[];
  rowStart: number[];
}

/**
 * Joins rows into logical lines where they wrap. A wrapped row is padded to
 * `cols` (emulators may leave its last cells out), so text stays in place.
 */
export function joinRows(rows: ScreenLine[], cols: number): Joined {
  const lines: Span[][] = [];
  const rowLine: number[] = [];
  const rowStart: number[] = [];
  let cur: Span[] | null = null;
  let curCells = 0;
  for (const row of rows) {
    if (!cur) {
      cur = [];
      curCells = 0;
    }
    rowLine.push(lines.length);
    rowStart.push(curCells);
    const spans = row.spans ?? [];
    cur.push(...spans);
    const cells = lineCells(spans);
    curCells += cells;
    if (row.wraps) {
      if (cells < cols) {
        cur.push(plain(' '.repeat(cols - cells)));
        curCells += cols - cells;
      }
    } else {
      lines.push(trimEnd(cur));
      cur = null;
    }
  }
  if (cur) lines.push(trimEnd(cur));
  return { lines, rowLine, rowStart };
}

/**
 * Cuts logical lines at `width` cells. A wide character that does not fit
 * goes to the next line whole. `pad` makes a line at least that many cells
 * long (where the cursor is).
 */
export function rewrap(lines: Span[][], width: number, pad?: { line: number; cells: number }): VLine[] {
  const w = Math.max(1, Math.floor(width));
  const out: VLine[] = [];
  lines.forEach((spans, li) => {
    let src = spans;
    if (pad && pad.line === li) {
      const have = lineCells(spans);
      if (have < pad.cells) src = [...spans, plain(' '.repeat(pad.cells - have))];
    }
    let cur: Span[] = [];
    let cells = 0;
    let start = 0;
    const flush = () => {
      out.push({ spans: cur, line: li, start, cells });
      start += cells;
      cur = [];
      cells = 0;
    };
    for (const span of src) {
      if (!span.text) continue;
      if (NARROW.test(span.text)) {
        let rest = span.text;
        while (rest.length) {
          if (cells >= w) flush();
          const take = Math.min(w - cells, rest.length);
          cur.push(take === rest.length && rest === span.text ? span : { ...span, text: rest.slice(0, take) });
          cells += take;
          rest = rest.slice(take);
        }
        continue;
      }
      let piece = '';
      const push = () => {
        if (piece) cur.push({ ...span, text: piece });
        piece = '';
      };
      for (const ch of span.text) {
        const cw = cellWidth(ch.codePointAt(0)!);
        if (cw > 0 && cells + cw > w && cells > 0) {
          push();
          flush();
        }
        piece += ch;
        cells += cw;
      }
      push();
    }
    if (cells > 0 || cur.length || start === 0) flush();
  });
  return out;
}

/** Where a logical position (line, cell) is among wrapped lines. */
export function locate(vlines: VLine[], line: number, offset: number): { index: number; col: number } | null {
  let found: { index: number; col: number } | null = null;
  for (let i = 0; i < vlines.length; i++) {
    const v = vlines[i]!;
    if (v.line !== line) {
      if (found) break;
      continue;
    }
    if (v.start <= offset) found = { index: i, col: offset - v.start };
  }
  return found;
}
