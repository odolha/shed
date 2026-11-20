/** A span's colours and attributes as CSS, for drawing. */

import type { Span } from '../../api/gen/api';

/** A span ready to draw: text, classes, inline style. */
export interface RSpan {
  t: string;
  c: string;
  s: string;
}

const LEVELS = [0, 95, 135, 175, 215, 255];

function hex(n: number): string {
  return n.toString(16).padStart(2, '0');
}

/** `#rrggbb`, or a palette index `0`…`255`, as a CSS colour. */
export function colorCss(c: string | null | undefined): string | null {
  if (!c) return null;
  if (c.startsWith('#')) return c;
  const n = Number(c);
  if (!Number.isInteger(n) || n < 0 || n > 255) return null;
  if (n < 16) return `var(--ansi-${n})`;
  if (n < 232) {
    const i = n - 16;
    return `#${hex(LEVELS[Math.floor(i / 36)]!)}${hex(LEVELS[Math.floor(i / 6) % 6]!)}${hex(LEVELS[i % 6]!)}`;
  }
  const g = 8 + (n - 232) * 10;
  return `#${hex(g)}${hex(g)}${hex(g)}`;
}

export function drawSpan(span: Span): RSpan {
  let fg = colorCss(span.fg);
  let bg = colorCss(span.bg);
  // Bold text in one of the first eight colours shows in its bright one.
  if (span.bold && span.fg && /^[0-7]$/.test(span.fg)) fg = `var(--ansi-${Number(span.fg) + 8})`;
  if (span.inverse) {
    const f = fg ?? 'var(--term-fg)';
    fg = bg ?? 'var(--term-bg)';
    bg = f;
  }
  let s = '';
  if (fg) s += `color:${fg};`;
  if (bg) s += `background-color:${bg};`;
  let c = '';
  if (span.bold) c += ' b';
  if (span.dim) c += ' d';
  if (span.italic) c += ' i';
  if (span.underline) c += ' u';
  if (span.strike) c += ' s';
  return { t: span.text, c: c.trim(), s };
}

export function drawSpans(spans: Span[]): RSpan[] {
  return spans.map(drawSpan);
}
