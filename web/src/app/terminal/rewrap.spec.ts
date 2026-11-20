import type { ScreenLine, Span } from '../../api/gen/api';
import { cellWidth, joinRows, locate, rewrap, textCells } from './rewrap';

const row = (text: string, wraps = false): ScreenLine => ({ spans: text ? [{ text }] : [], wraps });
const texts = (spans: Span[]) => spans.map((s) => s.text).join('');

describe('joinRows', () => {
  it('joins rows where they wrap into one logical line', () => {
    const j = joinRows([row('hello wor', true), row('ld'), row('next')], 9);
    expect(j.lines.map(texts)).toEqual(['hello world', 'next']);
    expect(j.rowLine).toEqual([0, 0, 1]);
    expect(j.rowStart).toEqual([0, 9, 0]);
  });

  it('pads a wrapped row left short, so text keeps its place', () => {
    const j = joinRows([row('abc', true), row('def')], 5);
    expect(texts(j.lines[0]!)).toBe('abc  def');
  });

  it('keeps empty lines and trims trailing blanks', () => {
    const j = joinRows([row('a   '), row(''), row('b')], 10);
    expect(j.lines.map(texts)).toEqual(['a', '', 'b']);
  });

  it('keeps trailing spaces that have a background', () => {
    const j = joinRows([{ spans: [{ text: 'x' }, { text: '   ', bg: '1' }], wraps: false }], 10);
    expect(texts(j.lines[0]!)).toBe('x   ');
  });
});

describe('rewrap', () => {
  it('cuts long lines at a narrower width', () => {
    const v = rewrap([[{ text: 'abcdefghij' }]], 4);
    expect(v.map((l) => texts(l.spans))).toEqual(['abcd', 'efgh', 'ij']);
    expect(v.map((l) => l.start)).toEqual([0, 4, 8]);
    expect(v.every((l) => l.line === 0)).toBe(true);
  });

  it('joins back at a wider width', () => {
    const j = joinRows([row('abcd', true), row('efgh', true), row('ij')], 4);
    const v = rewrap(j.lines, 80);
    expect(v.map((l) => texts(l.spans))).toEqual(['abcdefghij']);
  });

  it('keeps each piece of a split span in its style', () => {
    const v = rewrap([[{ text: 'ab' }, { text: 'cdef', fg: '2', bold: true }, { text: 'g' }]], 3);
    expect(v.map((l) => texts(l.spans))).toEqual(['abc', 'def', 'g']);
    expect(v[0]!.spans[1]).toEqual({ text: 'c', fg: '2', bold: true });
    expect(v[1]!.spans[0]).toEqual({ text: 'def', fg: '2', bold: true });
    expect(v[2]!.spans[0]).toEqual({ text: 'g' });
  });

  it('keeps empty lines as lines', () => {
    const v = rewrap([[{ text: 'a' }], [], [{ text: 'b' }]], 10);
    expect(v.length).toBe(3);
    expect(v[1]!.cells).toBe(0);
  });

  it('counts wide characters as two cells and never splits them', () => {
    expect(cellWidth('漢'.codePointAt(0)!)).toBe(2);
    expect(textCells('a漢b')).toBe(4);
    const v = rewrap([[{ text: 'a漢字b' }]], 4);
    expect(v.map((l) => texts(l.spans))).toEqual(['a漢', '字b']);
    expect(v[1]!.start).toBe(3);
  });

  it('pads the cursor line and finds the cursor', () => {
    const j = joinRows([row('$ ls', false)], 80);
    const v = rewrap(j.lines, 3, { line: 0, cells: 6 });
    expect(v.map((l) => texts(l.spans))).toEqual(['$ l', 's  ']);
    expect(locate(v, 0, 5)).toEqual({ index: 1, col: 2 });
    expect(locate(v, 0, 0)).toEqual({ index: 0, col: 0 });
  });
});
