/**
 * Sub-agents and background tasks (`AiTool.run`), and what Claude set to
 * happen later (`AiStatus.timers`), as the desktop words them.
 */

import { Injectable, signal } from '@angular/core';
import type { AiEntry, AiRun, AiTimer, AiTool } from '../../api/gen/api';
import { t } from '../../i18n/i18n';
import { nowSecs } from '../../ui/time';

/** A wake-up past its time by more than this is not counted down any more. */
const OVERDUE = 60;

/** Now, and whether Claude works, shared by an AI tab's entries (one clock per tab). */
@Injectable()
export class AiLive {
  readonly now = signal(nowSecs());
  readonly busy = signal(false);
  readonly timers = signal<AiTimer[]>([]);
}

/**
 * A span still going, coarse past a minute (redrawn every second only in the
 * first minute): `40 s`, `3 min`, `2 h 5 min`; whole minutes rounded down
 * (time gone) or up (time left).
 */
export function coarse(secs: number, up: boolean): string {
  const s = Math.max(0, Math.floor(secs));
  if (s < 60) return `${s} s`;
  const minutes = up ? Math.ceil(s / 60) : Math.floor(s / 60);
  if (minutes < 60) return `${minutes} min`;
  if (minutes % 60 === 0) return `${minutes / 60} h`;
  return `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}

/** How long something took: `40 s`, `2 min 5 s`, `12 min`, `1 h 3 min`. */
export function took(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  if (s < 60) return `${s} s`;
  if (s < 300 && s % 60 !== 0) return `${Math.floor(s / 60)} min ${s % 60} s`;
  if (s < 3600) return `${Math.floor((s + 30) / 60)} min`;
  if (Math.floor(s / 60) % 60 === 0) return `${Math.floor(s / 3600)} h`;
  return `${Math.floor(s / 3600)} h ${Math.floor(s / 60) % 60} min`;
}

/** Tokens, short: `12k`, `1.5M`. */
export function tokens(n: number): string {
  if (n >= 1_000_000)
    return n % 1_000_000 === 0 ? `${n / 1_000_000}M` : `${(n / 1_000_000).toFixed(1)}M`;
  return `${Math.floor(n / 1000)}k`;
}

export function isAgent(tool: AiTool): boolean {
  return tool.name === 'Agent' || tool.name === 'Task';
}

export function runState(run: AiRun): 'running' | 'done' | 'failed' | 'stopped' {
  const s = run.state ?? 'running';
  return s === 'done' || s === 'failed' || s === 'stopped' ? s : 'running';
}

/** A run's state in a word, and for how long: `running 3 min`, `done in 40 s`. */
export function runTime(run: AiRun, now = nowSecs()): string {
  const started = run.started ?? 0;
  const ended = run.ended ?? 0;
  switch (runState(run)) {
    case 'running':
      return started > 0
        ? t('ai.run.runningFor', { time: coarse(now - started, false) })
        : t('ai.run.running');
    case 'done':
      return started > 0 && ended >= started
        ? t('ai.run.doneIn', { time: took(ended - started) })
        : t('ai.run.done');
    case 'failed':
      return t('ai.run.failed');
    case 'stopped':
      return t('ai.run.stopped');
  }
}

/** The line after a sub-agent's or background task's tool: how it is doing, what it does now, how many tool calls. */
export function runLine(tool: AiTool, run: AiRun, now = nowSecs()): string {
  const parts = [runTime(run, now)];
  if (runState(run) === 'running' && run.last && run.last !== 'Starting') parts.push(run.last);
  if (isAgent(tool) && (run.tools ?? 0) > 0) parts.push(t('ai.run.tools', { n: run.tools ?? 0 }));
  return parts.join(' · ');
}

/** When a timer fires, as a person says it: `in 18 min`, `every minute, next in 1 min`. */
export function timerWhen(timer: AiTimer, now = nowSecs()): string {
  const at = timer.at ?? 0;
  const left = (a: number) =>
    a > now ? t('ai.timer.in', { time: coarse(a - now, true) }) : t('ai.timer.dueNow');
  const every = (timer.every ?? '').toLowerCase();
  if (timer.kind === 'watch')
    return at > 0 ? t('ai.timer.ends', { when: left(at) }) : t('ai.timer.watching');
  if (every && at > now) return t('ai.timer.everyNext', { every, when: left(at) });
  if (every) return every;
  if (at > 0) return left(at);
  return t('ai.timer.later');
}

/** A timer's chip: `Wakes up in 18 min`, `Scheduled job, every minute, next in 1 min`, `Watching, ends in 5 min`. */
export function timerLabel(timer: AiTimer, now = nowSecs()): string {
  const when = timerWhen(timer, now);
  switch (timer.kind) {
    case 'job':
      return t('ai.timer.job', { when });
    case 'watch':
      return timer.at ? t('ai.timer.watch', { when }) : t('ai.timer.watchOnly');
    default:
      return t('ai.timer.wakeup', { when });
  }
}

/** A timer's tooltip: what it is for, what it tells Claude, what cancels it. */
export function timerTip(timer: AiTimer): string {
  const what = timer.prompt
    ? `${timer.what}\n\n${t('ai.timer.tells', { prompt: timer.prompt })}`
    : timer.what;
  return `${what}\n\n${t('ai.timer.note')}`;
}

/** When runs going on started (Unix seconds, 0: not known). */
export function runningStarts(entries: AiEntry[]): number[] {
  const out: number[] = [];
  for (const e of entries) {
    if (e?.type === 'Tool' && e.content.run && runState(e.content.run) === 'running')
      out.push(e.content.run.started ?? 0);
  }
  return out;
}

/**
 * How often to redraw times and countdowns, in ms: every second while Claude
 * works, a run is in its first minute or a timer in its last; every 30 s
 * while something else is counted; null when nothing is.
 */
export function tickEvery(
  busy: boolean,
  starts: number[],
  timers: AiTimer[],
  now = nowSecs(),
): number | null {
  if (busy) return 1000;
  const counted = timers.filter((x) => (x.at ?? 0) > 0 && (x.at ?? 0) + OVERDUE > now);
  const fresh = starts.some((s) => s + 60 > now);
  if (fresh || counted.some((x) => (x.at ?? 0) <= now + 60)) return 1000;
  return starts.length || counted.length ? 30_000 : null;
}

/** A prompt with the paths of the files attached to it, as the desktop sends them. */
export function withAttachedFiles(text: string, paths: string[]): string {
  if (!paths.length) return text;
  return `${text}${text ? '\n\n' : ''}Attached files:\n${paths.map((p) => `- ${p}`).join('\n')}`;
}
