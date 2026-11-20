/**
 * Keys typed in a terminal, as the API's `InputItem`s. Named keys and
 * Ctrl/Alt combinations go as `Key`; plain text comes through the input
 * events (so phone keyboards and IMEs work) and goes as `Text`.
 */

import type { InputItem, Key } from '../../api/gen/api';

/** The parts of a KeyboardEvent the mapping looks at. */
export interface KeyLike {
  key: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
  isComposing?: boolean;
}

export interface Mods {
  ctrl?: boolean;
  alt?: boolean;
}

const NAMED: Record<string, string> = {
  Enter: 'Enter',
  Tab: 'Tab',
  Backspace: 'Backspace',
  Escape: 'Escape',
  Esc: 'Escape',
  ArrowUp: 'Up',
  ArrowDown: 'Down',
  ArrowLeft: 'Left',
  ArrowRight: 'Right',
  Up: 'Up',
  Down: 'Down',
  Left: 'Left',
  Right: 'Right',
  Home: 'Home',
  End: 'End',
  PageUp: 'PageUp',
  PageDown: 'PageDown',
  Insert: 'Insert',
  Delete: 'Delete',
};
for (let i = 1; i <= 12; i++) NAMED[`F${i}`] = `F${i}`;

/** The API's name for a named key (`ArrowUp` → `Up`), or null. */
export function namedKey(key: string): string | null {
  return NAMED[key] ?? null;
}

function single(key: string): boolean {
  return [...key].length === 1;
}

/**
 * What a keydown sends, or null when it is left to the input events (plain
 * text) or to the browser (Cmd shortcuts, lone modifiers). `sticky` adds the
 * key bar's Ctrl/Alt.
 */
export function keyToInput(e: KeyLike, sticky: Mods = {}): InputItem | null {
  if (e.isComposing || e.key === 'Unidentified' || e.key === 'Process' || e.key === 'Dead') return null;
  if (e.metaKey) return null;
  const ctrl = e.ctrlKey || !!sticky.ctrl;
  const alt = e.altKey || !!sticky.alt;
  const named = namedKey(e.key);
  if (named) {
    return { type: 'Key', content: { key: named, ctrl, alt, shift: e.shiftKey } };
  }
  if (single(e.key) && (ctrl || alt)) {
    // AltGr on Windows/Linux reports Ctrl+Alt: let the character through as text.
    if (e.ctrlKey && e.altKey && !/[a-z]/i.test(e.key)) return null;
    return { type: 'Key', content: charKey(e.key, ctrl, alt, e.shiftKey) };
  }
  return null;
}

function charKey(ch: string, ctrl: boolean, alt: boolean, shift: boolean): Key {
  const lower = ch.toLowerCase();
  const letter = lower !== ch.toUpperCase();
  return { key: letter ? lower : ch, ctrl, alt, shift: letter ? shift || ch !== lower : false };
}

/** Text typed, with the key bar's sticky modifiers applied to its first character. */
export function textToInput(text: string, sticky: Mods = {}): InputItem[] {
  if (!text) return [];
  if (!sticky.ctrl && !sticky.alt) return [{ type: 'Text', content: text }];
  const [first, ...rest] = [...text];
  const out: InputItem[] = [{ type: 'Key', content: charKey(first!, !!sticky.ctrl, !!sticky.alt, false) }];
  if (rest.length) out.push({ type: 'Text', content: rest.join('') });
  return out;
}

/** Copy and paste shortcuts that should not reach the program. */
export function clipboardShortcut(e: KeyLike, mac: boolean): 'copy' | 'paste' | null {
  const k = e.key.toLowerCase();
  if (mac ? e.metaKey && !e.ctrlKey : e.ctrlKey && e.shiftKey && !e.altKey) {
    if (k === 'c') return 'copy';
    if (k === 'v') return 'paste';
  }
  return null;
}

/** Joins neighbouring Text items, so a burst of typing is one item. */
export function mergeInput(items: InputItem[]): InputItem[] {
  const out: InputItem[] = [];
  for (const item of items) {
    const last = out[out.length - 1];
    if (item.type === 'Text' && last?.type === 'Text') {
      out[out.length - 1] = { type: 'Text', content: last.content + item.content };
    } else {
      out.push(item);
    }
  }
  return out;
}
