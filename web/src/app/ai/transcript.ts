import type { AiEntry, AiTranscript } from '../../api/gen/api';
import { t } from '../../i18n/i18n';

/** A transcript event replaces the entries from `from` on (the first: all of them). */
export function applyTranscript(old: AiEntry[], ev: AiTranscript): AiEntry[] {
  const from = Math.max(0, Math.min(ev.from, old.length));
  if (from === old.length && ev.entries.length === 0) return old;
  return [...old.slice(0, from), ...ev.entries];
}

/** What Claude is doing now, from the last entries: the tool running, thinking, writing. */
export function workingOn(entries: AiEntry[]): string {
  for (let i = entries.length - 1; i >= 0; i--) {
    const e = entries[i];
    if (!e) continue;
    switch (e.type) {
      case 'Tool':
        return e.content.result === null || e.content.result === undefined
          ? e.content.summary || e.content.name
          : t('ai.doing.working');
      case 'Thinking':
        return t('ai.doing.thinking');
      case 'Assistant':
        return t('ai.doing.writing');
      case 'User':
        return t('ai.doing.starting');
      default:
        return t('ai.doing.working');
    }
  }
  return t('ai.doing.working');
}
