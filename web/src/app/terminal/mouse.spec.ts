import { WheelNotches, buttonOf, cellAt, mouseItem, wheelItems } from './mouse';

const NONE = { ctrlKey: false, altKey: false, shiftKey: false };

describe('cellAt', () => {
  const g = { left: 100, top: 50, cellW: 8, cellH: 16, cols: 80, rows: 24 };

  it('finds the 0-based cell under the pointer', () => {
    expect(cellAt(g, 100, 50)).toEqual({ col: 0, row: 0 });
    expect(cellAt(g, 107.9, 65.9)).toEqual({ col: 0, row: 0 });
    expect(cellAt(g, 108, 66)).toEqual({ col: 1, row: 1 });
    expect(cellAt(g, 100 + 8 * 10 + 4, 50 + 16 * 5 + 1)).toEqual({ col: 10, row: 5 });
  });

  it('keeps to the grid outside it', () => {
    expect(cellAt(g, 0, 0)).toEqual({ col: 0, row: 0 });
    expect(cellAt(g, 5000, 5000)).toEqual({ col: 79, row: 23 });
  });
});

describe('mouse items', () => {
  it('maps buttons', () => {
    expect(buttonOf(0)).toBe('left');
    expect(buttonOf(1)).toBe('middle');
    expect(buttonOf(2)).toBe('right');
    expect(buttonOf(3)).toBeNull();
  });

  it('carries the cell, button and modifiers', () => {
    expect(mouseItem('press', { col: 3, row: 4 }, { ctrlKey: true, altKey: false, shiftKey: false }, 'right')).toEqual({
      type: 'Mouse',
      content: { kind: 'press', button: 'right', col: 3, row: 4, ctrl: true, alt: false, shift: false },
    });
  });

  it('makes one wheel item per notch, capped', () => {
    const at = { col: 1, row: 2 };
    expect(wheelItems(-2, at, NONE).map((i) => i.type === 'Mouse' && i.content.kind)).toEqual(['wheelUp', 'wheelUp']);
    expect(wheelItems(1, at, NONE).map((i) => i.type === 'Mouse' && i.content.kind)).toEqual(['wheelDown']);
    expect(wheelItems(50, at, NONE)).toHaveLength(10);
  });
});

describe('WheelNotches', () => {
  it('counts a mouse wheel click as a notch', () => {
    const w = new WheelNotches();
    expect(w.add(100, 0)).toBe(1);
    expect(w.add(-100, 0)).toBe(-1);
    expect(w.add(3, 1)).toBe(1);
  });

  it('adds up small touchpad moves', () => {
    const w = new WheelNotches();
    expect(w.add(40, 0)).toBe(0);
    expect(w.add(40, 0)).toBe(0);
    expect(w.add(40, 0)).toBe(1);
  });

  it('starts afresh when the direction turns', () => {
    const w = new WheelNotches();
    expect(w.add(90, 0)).toBe(0);
    expect(w.add(-20, 0)).toBe(0);
    expect(w.add(-80, 0)).toBe(-1);
  });
});
