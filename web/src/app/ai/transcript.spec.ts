import type { AiEntry, AiStatus, Event } from '../../api/gen/api';
import { matchCommands } from './prompt';
import { applyAiEvent, applyEntries, applyTranscript, workingOn } from './transcript';

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

describe('applyEntries', () => {
  const working: AiStatus = { state: 'working' };

  it('replaces entries in place, by index', () => {
    const old = [user('a'), said('b'), said('c')];
    const next = applyEntries(old, { entries: [{ index: 1, entry: said('B') }], status: working });
    expect(next).toEqual([user('a'), said('B'), said('c')]);
    expect(next[0]).toBe(old[0]);
    expect(old[1]).toEqual(said('b'));
  });

  it('leaves out indices past the end, and keeps the list when nothing changed', () => {
    const old = [user('a')];
    expect(applyEntries(old, { entries: [{ index: 1, entry: said('x') }, { index: -1, entry: said('y') }], status: working })).toBe(old);
    expect(applyEntries(old, { entries: [], status: working })).toBe(old);
  });
});

describe('applyAiEvent', () => {
  const idle: AiStatus = { state: 'idle' };
  const convo = { entries: [user('a'), said('b')], status: idle };

  it('takes transcripts and entries changed, with their status', () => {
    const next = applyAiEvent(convo, { type: 'AiEntries', content: { entries: [{ index: 1, entry: said('B') }], status: { state: 'working', timers: [{ what: 'x' }] } } });
    expect(next.entries).toEqual([user('a'), said('B')]);
    expect(next.status.timers?.length).toBe(1);
    const full = applyAiEvent(convo, { type: 'AiTranscript', content: { from: 2, entries: [said('c')], status: idle } });
    expect(full.entries.length).toBe(3);
  });

  it('ignores other events and those it does not know', () => {
    expect(applyAiEvent(convo, { type: 'Unknown' })).toBe(convo);
    expect(applyAiEvent(convo, { type: 'Teleported', content: { from: 0 } } as unknown as Event)).toBe(convo);
    expect(applyAiEvent(convo, { type: 'AiInfo', content: { commands: [], models: [] } })).toBe(convo);
  });

  it('keeps an entry it does not know in its place', () => {
    const odd = { type: 'Hologram', content: {} } as unknown as AiEntry;
    const next = applyAiEvent(convo, { type: 'AiTranscript', content: { from: 2, entries: [odd, said('c')], status: idle } });
    expect(next.entries[2]).toBe(odd);
    expect(next.entries[3]).toEqual(said('c'));
    // And says nothing of what Claude does.
    expect(workingOn([user('a'), odd])).toBe('Starting');
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
