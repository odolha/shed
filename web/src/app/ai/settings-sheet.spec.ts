import type { AiInfo, AiTab } from '../../api/gen/api';
import { FULL_AUTO, pickMode, settingsChoices, withFolder, withoutFolder } from './settings-sheet';

const tab = (over: Partial<AiTab> = {}): AiTab => ({ folder: '/home/you/shed', mode: 'default', activity: 'idle', ...over });
const info = {
  models: [
    { value: '', name: 'Default (Opus)', description: 'The default', efforts: ['low', 'high'] },
    { value: 'sonnet', name: 'Sonnet', description: '', efforts: [] },
  ],
  commands: [],
} as unknown as AiInfo;

describe('settingsChoices', () => {
  it('marks the default model when none is set, with its efforts', () => {
    const c = settingsChoices(tab(), info);
    expect(c.models.map((m) => [m.value, m.selected])).toEqual([
      ['', true],
      ['sonnet', false],
    ]);
    expect(c.models[1]?.detail).toBeUndefined();
    expect(c.efforts.map((e) => [e.value, e.selected])).toEqual([
      ['', true],
      ['low', false],
      ['high', false],
    ]);
  });

  it('offers no efforts for a model without them', () => {
    const c = settingsChoices(tab({ model: 'sonnet' }), info);
    expect(c.models.find((m) => m.selected)?.value).toBe('sonnet');
    expect(c.efforts).toEqual([]);
  });

  it('says the mode, Full Auto apart, and keeps a mode it does not know', () => {
    expect(settingsChoices(tab({ mode: 'plan' }), info).modes.find((m) => m.selected)?.value).toBe('plan');
    const full = settingsChoices(tab({ mode: FULL_AUTO }), info);
    expect(full.fullAuto).toBe(true);
    expect(full.modes.some((m) => m.selected)).toBe(false);
    const odd = settingsChoices(tab({ mode: 'dontAsk' }), info);
    expect(odd.modes.at(-1)).toEqual({ value: 'dontAsk', name: 'dontAsk', selected: true });
  });

  it('works before Claude Code says what models it has', () => {
    const c = settingsChoices(tab({ model: 'opus' }), null);
    expect(c.models).toEqual([{ value: 'opus', name: 'opus', selected: true }]);
    expect(c.efforts).toEqual([]);
  });

  it('carries the switches and folders', () => {
    const c = settingsChoices(tab({ follow: true, browser: false, addDirs: ['/srv/data'] }), info);
    expect([c.follow, c.browser, c.folders]).toEqual([true, false, ['/srv/data']]);
  });
});

describe('pickMode', () => {
  it('asks first for Full Auto, does nothing for the same, changes the rest', () => {
    expect(pickMode('default', FULL_AUTO)).toEqual({ kind: 'ask' });
    expect(pickMode(FULL_AUTO, FULL_AUTO)).toEqual({ kind: 'same' });
    expect(pickMode('plan', 'plan')).toEqual({ kind: 'same' });
    expect(pickMode(FULL_AUTO, 'plan')).toEqual({ kind: 'change', mode: 'plan' });
  });
});

describe('folders', () => {
  it('adds a folder once, trailing slash or not', () => {
    expect(withFolder(['/a'], '/b')).toEqual(['/a', '/b']);
    const same = ['/a'];
    expect(withFolder(same, '/a/')).toBe(same);
    expect(withFolder(same, '  ')).toBe(same);
    expect(withoutFolder(['/a', '/b'], '/a')).toEqual(['/b']);
  });
});
