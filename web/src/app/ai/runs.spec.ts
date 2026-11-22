import { TestBed } from '@angular/core/testing';
import type { AiEntry, AiRun, AiTimer, AiTool } from '../../api/gen/api';
import { AiEntryView, noteIcon } from './entries';
import { isPicture, mergedDraft } from './prompt';
import { coarse, runLine, runningStarts, tickEvery, timerLabel, timerWhen, took, tokens, withAttachedFiles } from './runs';

const NOW = 1_800_000_000;
const agent = (run: AiRun): AiTool => ({ id: 'a1', name: 'Agent', summary: 'Agent: Look around', run });

describe('coarse and took', () => {
  it('says spans as the desktop does', () => {
    expect(coarse(40, false)).toBe('40 s');
    expect(coarse(179, false)).toBe('2 min');
    expect(coarse(179, true)).toBe('3 min');
    expect(coarse(7200, false)).toBe('2 h');
    expect(coarse(7500, false)).toBe('2 h 5 min');
    expect(took(40)).toBe('40 s');
    expect(took(125)).toBe('2 min 5 s');
    expect(took(600)).toBe('10 min');
    expect(took(3900)).toBe('1 h 5 min');
    expect(tokens(12_345)).toBe('12k');
    expect(tokens(1_500_000)).toBe('1.5M');
  });
});

describe('runLine', () => {
  it('says how a run is doing, what it does and its tool calls', () => {
    const run: AiRun = { state: 'running', started: NOW - 200, last: 'Read src/main.rs', tools: 4 };
    expect(runLine(agent(run), run, NOW)).toBe('running 3 min · Read src/main.rs · 4 tool calls');
    const one: AiRun = { state: 'running', started: NOW - 5, last: 'Starting', tools: 1 };
    expect(runLine(agent(one), one, NOW)).toBe('running 5 s · 1 tool call');
  });

  it('says how long a finished run took, and how others ended', () => {
    const done: AiRun = { state: 'done', started: NOW - 100, ended: NOW - 60, last: 'Read x', tools: 0 };
    expect(runLine(agent(done), done, NOW)).toBe('done in 40 s');
    expect(runLine(agent({ state: 'failed' }), { state: 'failed' }, NOW)).toBe('failed');
    expect(runLine(agent({ state: 'stopped' }), { state: 'stopped' }, NOW)).toBe('stopped');
    expect(runLine(agent({ state: 'done' }), { state: 'done' }, NOW)).toBe('done');
    // A command in the background: no tool calls of its own.
    const bash: AiTool = { id: 'b', name: 'Bash', summary: '$ npm start', run: { state: 'running', tools: 3 } };
    expect(runLine(bash, bash.run!, NOW)).toBe('running');
  });
});

describe('timers', () => {
  it('says when they fire', () => {
    const wakeup: AiTimer = { kind: 'wakeup', what: 'Check the build', at: NOW + 18 * 60 - 30 };
    expect(timerWhen(wakeup, NOW)).toBe('in 18 min');
    expect(timerLabel(wakeup, NOW)).toBe('Wakes up in 18 min');
    const job: AiTimer = { kind: 'job', what: 'Look at CI', at: NOW + 50, recurring: true, every: 'Every minute' };
    expect(timerLabel(job, NOW)).toBe('Scheduled job, every minute, next in 50 s');
    expect(timerLabel({ ...job, at: NOW - 5 }, NOW)).toBe('Scheduled job, every minute');
    const watch: AiTimer = { kind: 'watch', what: 'Tests', at: NOW + 300 };
    expect(timerLabel(watch, NOW)).toBe('Watching, ends in 5 min');
    expect(timerLabel({ kind: 'watch', what: 'Tests' }, NOW)).toBe('Watching');
    expect(timerWhen({ what: 'x', at: NOW - 10 }, NOW)).toBe('due now');
    expect(timerWhen({ what: 'x' }, NOW)).toBe('later');
  });

  it('ticks every second near a change, every 30 s otherwise, else not at all', () => {
    expect(tickEvery(true, [], [], NOW)).toBe(1000);
    expect(tickEvery(false, [], [], NOW)).toBeNull();
    expect(tickEvery(false, [NOW - 10], [], NOW)).toBe(1000);
    expect(tickEvery(false, [NOW - 600], [], NOW)).toBe(30_000);
    expect(tickEvery(false, [], [{ what: 'x', at: NOW + 30 }], NOW)).toBe(1000);
    expect(tickEvery(false, [], [{ what: 'x', at: NOW + 3000 }], NOW)).toBe(30_000);
    // Long overdue, or with no time: nothing to count.
    expect(tickEvery(false, [], [{ what: 'x', at: NOW - 600 }, { what: 'y' }], NOW)).toBeNull();
  });

  it('finds the runs going on', () => {
    const entries: AiEntry[] = [
      { type: 'Tool', content: agent({ state: 'running', started: 5 }) },
      { type: 'Tool', content: agent({ state: 'done', started: 1 }) },
      { type: 'Tool', content: { id: 'x', name: 'Read', summary: 'Read a' } },
    ];
    expect(runningStarts(entries)).toEqual([5]);
  });
});

describe('attachments', () => {
  it('appends the paths as the desktop does', () => {
    expect(withAttachedFiles('Look', ['/s/a.pdf', '/s/b.txt'])).toBe('Look\n\nAttached files:\n- /s/a.pdf\n- /s/b.txt');
    expect(withAttachedFiles('', ['/s/a.pdf'])).toBe('Attached files:\n- /s/a.pdf');
    expect(withAttachedFiles('Just text', [])).toBe('Just text');
  });

  it('sends pictures as pictures, the rest as files', () => {
    expect(isPicture({ type: 'image/png' })).toBe(true);
    expect(isPicture({ type: 'image/webp' })).toBe(true);
    expect(isPicture({ type: 'image/svg+xml' })).toBe(false);
    expect(isPicture({ type: 'application/pdf' })).toBe(false);
  });
});

describe('notes', () => {
  it('have an icon of their kind, a task’s for kinds not known', () => {
    expect(noteIcon({ kind: 'wakeup', title: 'x' })).toBe('clock');
    expect(noteIcon({ title: 'x' })).toBe('tool');
    expect(noteIcon({ kind: 'later' as never, title: 'x' })).toBe('tool');
    expect(noteIcon({ kind: 'command', title: 'x', error: true })).toBe('alert');
  });
});

describe('AiEntryView', () => {
  function render(entry: AiEntry): HTMLElement {
    const f = TestBed.createComponent(AiEntryView);
    f.componentRef.setInput('entry', entry);
    f.componentRef.setInput('index', 0);
    f.componentRef.setInput('workspace', 'w');
    f.componentRef.setInput('tab', 't');
    f.detectChanges();
    return f.nativeElement as HTMLElement;
  }

  it('shows an entry of a newer workspace as nothing', () => {
    const el = render({ type: 'Hologram', content: { text: 'hi' } } as unknown as AiEntry);
    expect(el.textContent?.trim()).toBe('');
    expect(render({ type: 'Unknown' }).textContent?.trim()).toBe('');
  });

  it('shows a note with its title', () => {
    const el = render({ type: 'Note', content: { kind: 'task', title: 'Background command finished', detail: 'All **good**' } });
    expect(el.textContent).toContain('Background command finished');
    expect(el.textContent).not.toContain('good');
  });

  it('shows a sub-agent with how it runs', () => {
    const el = render({ type: 'Tool', content: agent({ state: 'running', started: Math.floor(Date.now() / 1000) - 5, last: 'Read a.rs', tools: 2 }) });
    expect(el.textContent).toContain('Agent: Look around');
    expect(el.textContent).toContain('Read a.rs · 2 tool calls');
  });
});

describe('mergedDraft', () => {
  it('puts the draft at the end, after a blank line', () => {
    expect(mergedDraft('', 'and the tests')).toBe('and the tests');
    expect(mergedDraft('  \n', 'and the tests')).toBe('and the tests');
    expect(mergedDraft('Fix the build', 'and the tests')).toBe('Fix the build\n\nand the tests');
    expect(mergedDraft('Fix the build\n\n', 'and the tests')).toBe('Fix the build\n\nand the tests');
  });
});
