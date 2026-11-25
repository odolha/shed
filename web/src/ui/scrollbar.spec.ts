import { MIN_THUMB, scrollTopFor, thumbFor } from './scrollbar';
import { keyboardInset, keyboardUp } from './viewport';

describe('the scrollbar', () => {
  it('has no thumb when nothing scrolls', () => {
    expect(thumbFor({ scrollTop: 0, scrollHeight: 400, clientHeight: 400 }, 400)).toBeNull();
  });

  it('sizes the thumb to what shows and places it by the scroll', () => {
    const s = { scrollTop: 0, scrollHeight: 2000, clientHeight: 500 };
    expect(thumbFor(s, 500)).toEqual({ top: 0, height: 125 });
    expect(thumbFor({ ...s, scrollTop: 1500 }, 500)).toEqual({ top: 375, height: 125 });
    expect(thumbFor({ ...s, scrollTop: 750 }, 500)?.top).toBeCloseTo(187.5);
  });

  it('keeps a long list’s thumb big enough to grab', () => {
    expect(thumbFor({ scrollTop: 0, scrollHeight: 100_000, clientHeight: 500 }, 500)?.height).toBe(MIN_THUMB);
  });

  it('scrolls where the thumb is dragged, within the list', () => {
    const s = { scrollTop: 0, scrollHeight: 2000, clientHeight: 500 };
    expect(scrollTopFor(0, s, 500, 125)).toBe(0);
    expect(scrollTopFor(375, s, 500, 125)).toBe(1500);
    expect(scrollTopFor(187.5, s, 500, 125)).toBe(750);
    expect(scrollTopFor(999, s, 500, 125)).toBe(1500);
    expect(scrollTopFor(-5, s, 500, 125)).toBe(0);
  });
});

describe('the on-screen keyboard', () => {
  it('is up when what shows is well short of the tallest seen', () => {
    expect(keyboardUp(915, 915)).toBe(false);
    expect(keyboardUp(915, 860)).toBe(false);
    expect(keyboardUp(915, 520)).toBe(true);
  });

  it('covers what the layout did not shrink for', () => {
    expect(keyboardInset(915, 520, 0)).toBe(395);
    expect(keyboardInset(520, 520, 0)).toBe(0);
    expect(keyboardInset(915, 520, 395)).toBe(0);
  });
});
