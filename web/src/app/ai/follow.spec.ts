import type { AiEntry } from '../../api/gen/api';
import { Follow, atEnd, below, nextPrompt, previousPrompt, promptIndices, topEntry } from './follow';

const user = (text: string): AiEntry => ({ type: 'User', content: { text } }) as AiEntry;
const said = (text: string): AiEntry => ({ type: 'Assistant', content: { text } }) as AiEntry;
// 0 you, 1 Claude, 2 you, 3 Claude, 4 Claude, 5 you, 6 Claude
const convo = [user('a'), said('b'), user('c'), said('d'), said('e'), user('f'), said('g')];

describe('Follow', () => {
  it('follows from the start and scrolls while something is below', () => {
    const f = new Follow();
    expect(f.following()).toBe(true);
    expect(f.shouldScroll(true)).toBe(true);
    expect(f.shouldScroll(false)).toBe(false);
    expect(f.showJump(true)).toBe(false);
  });

  it('is not scrolled under a finger, and stops following when left above the end', () => {
    const f = new Follow();
    f.dragged();
    expect(f.shouldScroll(true)).toBe(false);
    f.settled(false);
    expect(f.following()).toBe(false);
    expect(f.showJump(true)).toBe(true);
    expect(f.showJump(false)).toBe(false);
  });

  it('follows again when scrolled back to the end by hand', () => {
    const f = new Follow();
    f.dragged();
    f.settled(false);
    f.dragged();
    f.settled(true);
    expect(f.following()).toBe(true);
  });

  it('keeps following when the list settles away from the end by itself (content grew)', () => {
    const f = new Follow();
    f.settled(false);
    expect(f.following()).toBe(true);
  });

  it('jumps: to the latest follows, to a message does not', () => {
    const f = new Follow();
    f.away();
    expect(f.following()).toBe(false);
    // A smooth scroll there settles away from the end: still not following.
    f.settled(false);
    expect(f.following()).toBe(false);
    f.dragged();
    f.toLatest();
    expect(f.following()).toBe(true);
    expect(f.touched()).toBe(false);
  });
});

describe('at the end', () => {
  it('measures what is left below, with some slack', () => {
    expect(below({ scrollHeight: 1000, scrollTop: 600, clientHeight: 400 })).toBe(0);
    expect(below({ scrollHeight: 1000, scrollTop: 500, clientHeight: 400 })).toBe(100);
    expect(atEnd({ scrollHeight: 1000, scrollTop: 570, clientHeight: 400 })).toBe(true);
    expect(atEnd({ scrollHeight: 1000, scrollTop: 500, clientHeight: 400 })).toBe(false);
    // Nothing to scroll: at the end.
    expect(atEnd({ scrollHeight: 300, scrollTop: 0, clientHeight: 400 })).toBe(true);
  });
});

describe('jumping between your messages', () => {
  it('finds your messages', () => {
    expect(promptIndices(convo)).toEqual([0, 2, 5]);
  });

  it('goes to the previous message, or the start of the one scrolled into', () => {
    expect(previousPrompt(convo, 4, false)).toBe(2);
    expect(previousPrompt(convo, 5, false)).toBe(2);
    expect(previousPrompt(convo, 5, true)).toBe(5);
    expect(previousPrompt(convo, 0, false)).toBeNull();
    expect(previousPrompt(convo, 0, true)).toBe(0);
  });

  it('goes to the next message, none past the last', () => {
    expect(nextPrompt(convo, 0)).toBe(2);
    expect(nextPrompt(convo, 2)).toBe(5);
    expect(nextPrompt(convo, 5)).toBeNull();
  });

  it('knows the entry at the top and whether it is scrolled into', () => {
    const boxes = [
      { index: 3, top: -300, bottom: -20 },
      { index: 4, top: -20, bottom: 200 },
      { index: 5, top: 200, bottom: 260 },
    ];
    expect(topEntry(boxes, 0)).toEqual({ top: 4, inside: true });
    // Just at its start (after a jump, 8 px below the top): not inside.
    expect(topEntry([{ index: 5, top: 8, bottom: 60 }], 0)).toEqual({ top: 5, inside: false });
    expect(topEntry([], 0)).toBeNull();
  });
});
