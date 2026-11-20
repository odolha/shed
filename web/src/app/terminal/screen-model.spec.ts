import type { Screen, ScreenLine } from '../../api/gen/api';
import { ScreenModel } from './screen-model';

const line = (text: string): ScreenLine => ({ spans: [{ text }], wraps: false });
const text = (l: ScreenLine) => l.spans.map((s) => s.text).join('');

function screen(over: Partial<Screen>): Screen {
  return {
    full: false,
    cols: 10,
    rows: 3,
    lines: [],
    cursor: { row: 0, col: 0, visible: true },
    ...over,
  };
}

describe('ScreenModel', () => {
  it('takes a full screen, blank where no rows are given', () => {
    const m = new ScreenModel();
    m.apply(screen({ full: true, lines: [{ row: 1, line: line('b') }], title: 'sh', scrollback: 7 }));
    expect(m.lines.map(text)).toEqual(['', 'b', '']);
    expect(m.title).toBe('sh');
    expect(m.scrollback).toBe(7);
  });

  it('changes only the rows a partial update gives', () => {
    const m = new ScreenModel();
    m.apply(screen({ full: true, lines: [0, 1, 2].map((r) => ({ row: r, line: line(`r${r}`) })) }));
    const before = m.lines;
    m.apply(screen({ lines: [{ row: 2, line: line('new') }] }));
    expect(m.lines.map(text)).toEqual(['r0', 'r1', 'new']);
    expect(m.lines).not.toBe(before);
  });

  it('keeps lines that scroll off as history', () => {
    const m = new ScreenModel();
    m.apply(screen({ full: true, lines: [0, 1, 2].map((r) => ({ row: r, line: line(`r${r}`) })) }));
    m.apply(screen({ scrolled: 1, lines: [0, 1, 2].map((r) => ({ row: r, line: line(`s${r}`) })), scrollback: 1 }));
    expect(m.history.map(text)).toEqual(['r0']);
    m.prepend([line('old')]);
    expect(m.history.map(text)).toEqual(['old', 'r0']);
  });

  it('switches to the alternate screen and back without touching history', () => {
    const m = new ScreenModel();
    m.apply(screen({ full: true, lines: [{ row: 0, line: line('shell') }] }));
    m.prepend([line('h')]);
    m.apply(screen({ full: true, alt: true, scrolled: 2, lines: [{ row: 0, line: line('vim') }] }));
    expect(m.alt).toBe(true);
    expect(m.hasMore).toBe(false);
    expect(m.history.map(text)).toEqual(['h']);
    m.apply(screen({ full: true, alt: false, lines: [{ row: 0, line: line('shell') }] }));
    expect(m.alt).toBe(false);
    expect(m.lines.map(text)[0]).toBe('shell');
  });

  it('resizes: rows padded or cut, history dropped when the width changes', () => {
    const m = new ScreenModel();
    m.apply(screen({ full: true, lines: [{ row: 0, line: line('a') }] }));
    m.prepend([line('h')]);
    m.apply(screen({ rows: 5, lines: [] }));
    expect(m.lines.length).toBe(5);
    expect(m.history.length).toBe(1);
    m.apply(screen({ full: true, cols: 40, rows: 2, lines: [{ row: 0, line: line('a') }] }));
    expect(m.lines.length).toBe(2);
    expect(m.cols).toBe(40);
    expect(m.history.length).toBe(0);
  });

  it('knows when there is more scrollback to fetch', () => {
    const m = new ScreenModel(3);
    m.apply(screen({ full: true, scrollback: 10 }));
    expect(m.hasMore).toBe(true);
    m.prepend([line('a'), line('b'), line('c'), line('d')]);
    expect(m.history.map(text)).toEqual(['b', 'c', 'd']);
    expect(m.hasMore).toBe(false);
  });
});
