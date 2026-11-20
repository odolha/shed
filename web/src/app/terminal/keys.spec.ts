import { clipboardShortcut, keyToInput, mergeInput, textToInput, type KeyLike } from './keys';
import { colorCss, drawSpan } from './style';
import { place } from './terminal-tab';

const k = (key: string, mods: Partial<KeyLike> = {}): KeyLike => ({
  key,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  metaKey: false,
  ...mods,
});

describe('keyToInput', () => {
  it('names special keys as the API does', () => {
    expect(keyToInput(k('ArrowUp'))).toEqual({ type: 'Key', content: { key: 'Up', ctrl: false, alt: false, shift: false } });
    expect(keyToInput(k('Enter'))?.content).toMatchObject({ key: 'Enter' });
    expect(keyToInput(k('F5'))?.content).toMatchObject({ key: 'F5' });
    expect(keyToInput(k('Tab', { shiftKey: true }))?.content).toMatchObject({ key: 'Tab', shift: true });
  });

  it('sends Ctrl and Alt combinations as keys', () => {
    expect(keyToInput(k('c', { ctrlKey: true }))).toEqual({ type: 'Key', content: { key: 'c', ctrl: true, alt: false, shift: false } });
    expect(keyToInput(k('C', { ctrlKey: true, shiftKey: true }))?.content).toMatchObject({ key: 'c', shift: true });
    expect(keyToInput(k('b', { altKey: true }))?.content).toMatchObject({ key: 'b', alt: true });
  });

  it('leaves plain text, IME input and Cmd shortcuts alone', () => {
    expect(keyToInput(k('a'))).toBeNull();
    expect(keyToInput(k('Unidentified'))).toBeNull();
    expect(keyToInput(k('a', { isComposing: true }))).toBeNull();
    expect(keyToInput(k('c', { metaKey: true }))).toBeNull();
    expect(keyToInput(k('Shift', { shiftKey: true }))).toBeNull();
    // AltGr: Ctrl+Alt with a symbol is the symbol, typed.
    expect(keyToInput(k('@', { ctrlKey: true, altKey: true }))).toBeNull();
  });

  it('applies the key bar sticky modifiers', () => {
    expect(keyToInput(k('ArrowLeft'), { ctrl: true })?.content).toMatchObject({ key: 'Left', ctrl: true });
    expect(textToInput('cd', { ctrl: true })).toEqual([
      { type: 'Key', content: { key: 'c', ctrl: true, alt: false, shift: false } },
      { type: 'Text', content: 'd' },
    ]);
    expect(textToInput('ls')).toEqual([{ type: 'Text', content: 'ls' }]);
  });

  it('knows copy and paste shortcuts', () => {
    expect(clipboardShortcut(k('C', { ctrlKey: true, shiftKey: true }), false)).toBe('copy');
    expect(clipboardShortcut(k('V', { ctrlKey: true, shiftKey: true }), false)).toBe('paste');
    expect(clipboardShortcut(k('c', { ctrlKey: true }), false)).toBeNull();
    expect(clipboardShortcut(k('c', { metaKey: true }), true)).toBe('copy');
  });

  it('merges a burst of text into one item', () => {
    expect(
      mergeInput([
        { type: 'Text', content: 'l' },
        { type: 'Text', content: 's' },
        { type: 'Key', content: { key: 'Enter', ctrl: false, alt: false, shift: false } },
        { type: 'Text', content: 'x' },
      ]).map((i) => i.type),
    ).toEqual(['Text', 'Key', 'Text']);
  });
});

describe('colours', () => {
  it('maps palette indexes and hex colours', () => {
    expect(colorCss('1')).toBe('var(--ansi-1)');
    expect(colorCss('#123456')).toBe('#123456');
    expect(colorCss('196')).toBe('#ff0000');
    expect(colorCss('232')).toBe('#080808');
    expect(colorCss(null)).toBeNull();
  });

  it('swaps colours for inverse text', () => {
    expect(drawSpan({ text: 'x', inverse: true }).s).toBe('color:var(--term-bg);background-color:var(--term-fg);');
    expect(drawSpan({ text: 'x', bold: true, underline: true }).c).toBe('b u');
  });
});

describe('place', () => {
  it('lays out splits by their ratio', () => {
    const p = place({
      type: 'Split',
      content: {
        dir: 'right',
        ratio: 0.5,
        first: { type: 'Pane', content: 'a' },
        second: {
          type: 'Split',
          content: { dir: 'down', ratio: 0.25, first: { type: 'Pane', content: 'b' }, second: { type: 'Pane', content: 'c' } },
        },
      },
    });
    expect(p).toEqual([
      { id: 'a', x: 0, y: 0, w: 0.5, h: 1 },
      { id: 'b', x: 0.5, y: 0, w: 0.5, h: 0.25 },
      { id: 'c', x: 0.5, y: 0.25, w: 0.5, h: 0.75 },
    ]);
  });
});
