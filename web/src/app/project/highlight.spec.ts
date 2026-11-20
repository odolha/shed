import { fileSize, highlightLine, parentOf, positionMarks, rangeMarks, segment, splitPath } from './highlight';

describe('segment', () => {
  it('cuts text at the marks', () => {
    expect(segment('let x = 1;', [{ start: 0, end: 3, cls: 'k' }, { start: 8, end: 9, cls: 'n' }])).toEqual([
      { text: 'let', cls: 'k' },
      { text: ' x = ', cls: null },
      { text: '1', cls: 'n' },
      { text: ';', cls: null },
    ]);
  });

  it('keeps plain text whole without marks', () => {
    expect(segment('abc', [])).toEqual([{ text: 'abc', cls: null }]);
    expect(segment('', [])).toEqual([]);
  });

  it('clips overlaps and marks past the end', () => {
    expect(segment('abcdef', [{ start: 0, end: 4, cls: 'a' }, { start: 2, end: 10, cls: 'b' }])).toEqual([
      { text: 'abcd', cls: 'a' },
      { text: 'ef', cls: 'b' },
    ]);
  });

  it('counts UTF-16 code units, as the API does (an emoji is two)', () => {
    expect(segment('😀 ok', [{ start: 3, end: 5, cls: 'k' }])).toEqual([
      { text: '😀 ', cls: null },
      { text: 'ok', cls: 'k' },
    ]);
    expect(segment('a😀b', [{ start: 1, end: 3, cls: 'e' }])).toEqual([
      { text: 'a', cls: null },
      { text: '😀', cls: 'e' },
      { text: 'b', cls: null },
    ]);
  });

  it('never splits an emoji in two', () => {
    // Quick open marks a matched emoji by its first unit only.
    expect(segment('x😀y', positionMarks([1], 'm'))).toEqual([
      { text: 'x', cls: null },
      { text: '😀', cls: 'm' },
      { text: 'y', cls: null },
    ]);
    expect(segment('x😀y', [{ start: 2, end: 4, cls: 'm' }])).toEqual([
      { text: 'x', cls: null },
      { text: '😀y', cls: 'm' },
    ]);
  });
});

describe('highlightLine', () => {
  it('makes classes from kinds and drops odd ones', () => {
    const segs = highlightLine('fn main', [
      { start: 0, end: 2, kind: 'keyword' },
      { start: 3, end: 7, kind: 'bad kind"' },
    ]);
    expect(segs).toEqual([
      { text: 'fn', cls: 'hl-keyword' },
      { text: ' main', cls: null },
    ]);
  });
});

describe('marks', () => {
  it('joins neighbouring positions', () => {
    expect(positionMarks([4, 0, 1, 2], 'm')).toEqual([
      { start: 0, end: 3, cls: 'm' },
      { start: 4, end: 5, cls: 'm' },
    ]);
  });

  it('reads ranges in pairs', () => {
    expect(rangeMarks([1, 3, 5, 6, 9], 'h')).toEqual([
      { start: 1, end: 3, cls: 'h' },
      { start: 5, end: 6, cls: 'h' },
    ]);
  });
});

describe('paths', () => {
  it('splits and finds parents', () => {
    expect(splitPath('src/app/main.rs')).toEqual(['src/app/', 'main.rs']);
    expect(splitPath('README.md')).toEqual(['', 'README.md']);
    expect(parentOf('src/app/main.rs')).toBe('src/app');
    expect(parentOf('src/')).toBe('');
    expect(parentOf('a')).toBe('');
  });

  it('writes sizes', () => {
    expect(fileSize(512)).toBe('512 B');
    expect(fileSize(1536)).toBe('1.5 KB');
    expect(fileSize(50 * 1024 * 1024)).toBe('50 MB');
  });
});
