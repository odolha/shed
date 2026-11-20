import { rangeMarks, segment } from './highlight';
import { trimHit } from './search';

describe('trimHit', () => {
  it('keeps short lines and their ranges as they are', () => {
    expect(trimHit('😀 needle', [3, 9])).toEqual({ text: '😀 needle', ranges: [3, 9] });
  });

  it('cuts long lines before the first match, in UTF-16 units', () => {
    const text = '😀'.repeat(100) + 'needle' + 'x'.repeat(200);
    const at = 200; // a hundred emoji are two hundred units
    const cut = trimHit(text, [at, at + 6]);
    expect(cut.text.startsWith('…')).toBe(true);
    const segs = segment(cut.text, rangeMarks(cut.ranges, 'match'));
    expect(segs.find((s) => s.cls === 'match')?.text).toBe('needle');
  });

  it('does not cut an emoji in half', () => {
    // Emoji start at odd units here, so 40 before the match (262) is a second half.
    const text = 'a' + '😀'.repeat(150) + 'b' + 'needle';
    const at = 302;
    const cut = trimHit(text, [at, at + 6]);
    expect(cut.text.charCodeAt(1)).toBe(0xd83d);
    const segs = segment(cut.text, rangeMarks(cut.ranges, 'match'));
    expect(segs.find((s) => s.cls === 'match')?.text).toBe('needle');
  });
});
