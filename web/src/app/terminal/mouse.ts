/**
 * The mouse over the alternate screen's grid, as `InputItem::Mouse`: which
 * cell the pointer is over, which button, and the wheel in notches. The link
 * sends it on only when the program asked for the mouse (or, for the wheel
 * in less and the like, as arrow keys).
 */

import type { InputItem, MouseButton, MouseKind } from '../../api/gen/api';

/** Where the grid is on the page and how big its cells are, in CSS pixels. */
export interface GridGeom {
  left: number;
  top: number;
  cellW: number;
  cellH: number;
  cols: number;
  rows: number;
}

export interface Cell {
  col: number;
  row: number;
}

export interface MouseMods {
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

/** The 0-based cell under a point (client coordinates), kept on the grid. */
export function cellAt(g: GridGeom, x: number, y: number): Cell {
  const clamp = (v: number, n: number) => Math.max(0, Math.min(n - 1, v));
  const col = g.cellW > 0 ? Math.floor((x - g.left) / g.cellW) : 0;
  const row = g.cellH > 0 ? Math.floor((y - g.top) / g.cellH) : 0;
  return { col: clamp(col, Math.max(1, g.cols)), row: clamp(row, Math.max(1, g.rows)) };
}

/** `PointerEvent.button` as the API's button; null for others (back, forward). */
export function buttonOf(button: number): MouseButton | null {
  switch (button) {
    case 0:
      return 'left';
    case 1:
      return 'middle';
    case 2:
      return 'right';
    default:
      return null;
  }
}

export function mouseItem(kind: MouseKind, cell: Cell, mods: MouseMods, button: MouseButton | null = null): InputItem {
  return {
    type: 'Mouse',
    content: {
      kind,
      button,
      col: cell.col,
      row: cell.row,
      ctrl: mods.ctrlKey,
      alt: mods.altKey,
      shift: mods.shiftKey,
    },
  };
}

/** Pixels of wheel that make one notch (a mouse wheel gives about this per click). */
const NOTCH_PX = 100;
/** Lines of wheel (Firefox counts lines) that make one notch. */
const NOTCH_LINES = 3;

/**
 * Wheel movement into whole notches, keeping the rest for the next event
 * (touchpads move a little at a time). Positive is down, negative up.
 */
export class WheelNotches {
  private rest = 0;

  /** `deltaMode`: 0 pixels, 1 lines, 2 pages. */
  add(deltaY: number, deltaMode: number): number {
    const notches = deltaMode === 2 ? deltaY * 4 : deltaMode === 1 ? deltaY / NOTCH_LINES : deltaY / NOTCH_PX;
    // A change of direction starts afresh.
    if (this.rest && Math.sign(notches) !== Math.sign(this.rest)) this.rest = 0;
    this.rest += notches;
    // (A hair of slack, so 0.4 + 0.6 is a notch despite floating point.)
    const whole = this.rest > 0 ? Math.floor(this.rest + 1e-9) : Math.ceil(this.rest - 1e-9);
    if (!whole) return 0;
    this.rest -= whole;
    return whole;
  }
}

/** Notches as wheel items at a cell, at most `max` (a flung wheel should not flood). */
export function wheelItems(notches: number, cell: Cell, mods: MouseMods, max = 10): InputItem[] {
  const n = Math.min(max, Math.abs(notches));
  const kind: MouseKind = notches < 0 ? 'wheelUp' : 'wheelDown';
  return Array.from({ length: n }, () => mouseItem(kind, cell, mods));
}
