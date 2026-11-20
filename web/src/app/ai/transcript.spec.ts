import type { AiEntry } from '../../api/gen/api';
import { matchCommands } from './prompt';
import { applyTranscript, workingOn } from './transcript';

const user = (text: string): AiEntry => ({ type: 'User', content: { text } });
const said = (text: string): AiEntry => ({ type: 'Assistant', content: { text } });

describe('applyTranscript', () => {
  it('replaces the entries from `from` on', () => {
    const old = [user('a'), said('b'), said('c')];
    expect(applyTranscript(old, { from: 1, entries: [said('B')], status: { state: 'working' } })).toEqual([user('a'), said('B')]);
    expect(applyTranscript(old, { from: 0, entries: [user('x')], status: { state: 'idle' } })).toEqual([user('x')]);
  });

  it('appends, and keeps the same list for a status-only event', () => {
    const old = [user('a')];
    expect(applyTranscript(old, { from: 1, entries: [said('b')], status: { state: 'working' } })).toEqual([user('a'), said('b')]);
    expect(applyTranscript(old, { from: 1, entries: [], status: { state: 'idle' } })).toBe(old);
    expect(applyTranscript(old, { from: 9, entries: [said('c')], status: { state: 'idle' } })).toEqual([user('a'), said('c')]);
  });
});

describe('workingOn', () => {
  it('names the tool running, else what Claude is doing', () => {
    const tool: AiEntry = { type: 'Tool', content: { id: '1', name: 'Bash', summary: '$ cargo test', result: null } };
    expect(workingOn([user('a'), tool])).toBe('$ cargo test');
    expect(workingOn([user('a'), said('x')])).toBe('Writing');
    expect(workingOn([user('a'), { type: 'Thinking', content: { text: '' } }])).toBe('Thinking');
  });
});

describe('matchCommands', () => {
  const commands = [{ name: 'compact' }, { name: '/clear' }, { name: 'review' }, { name: 'init' }, { name: 'precompact' }];

  it('lists commands after a slash, prefix matches first', () => {
    expect(matchCommands(commands, '/c').map((c) => c.name)).toEqual(['compact', '/clear', 'precompact']);
    expect(matchCommands(commands, '/').length).toBe(5);
  });

  it('stops once a space is typed or without a slash', () => {
    expect(matchCommands(commands, '/compact now')).toEqual([]);
    expect(matchCommands(commands, 'compact')).toEqual([]);
  });
});
