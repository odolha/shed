import type { AiEntriesChanged, AiEntry, AiStatus, AiTranscript, Event } from '../../api/gen/api';
import { t } from '../../i18n/i18n';

/** A transcript event replaces the entries from `from` on (the first: all of them). */
export function applyTranscript(old: AiEntry[], ev: AiTranscript): AiEntry[] {
  const from = Math.max(0, Math.min(ev.from, old.length));
  if (from === old.length && ev.entries.length === 0) return old;
  return [...old.slice(0, from), ...ev.entries];
}

/**
 * Entries changed in place (a sub-agent's progress): each replaces the one at
 * its index; an index past what is here is left out (a transcript brings it).
 */
export function applyEntries(old: AiEntry[], ev: AiEntriesChanged): AiEntry[] {
  let next: AiEntry[] | null = null;
  for (const at of ev.entries ?? []) {
    if (!Number.isInteger(at.index) || at.index < 0 || at.index >= old.length) continue;
    next ??= [...old];
    next[at.index] = at.entry;
  }
  return next ?? old;
}

/** A conversation as an AI tab holds it. */
export interface Conversation {
  entries: AiEntry[];
  status: AiStatus;
}

/**
 * A conversation after an event of its topic; the same object for events
 * that do not change it (`AiInfo`, and those of a newer workspace).
 */
export function applyAiEvent(c: Conversation, e: Event): Conversation {
  switch (e?.type) {
    case 'AiTranscript':
      return { entries: applyTranscript(c.entries, e.content), status: e.content.status };
    case 'AiEntries':
      return { entries: applyEntries(c.entries, e.content), status: e.content.status ?? c.status };
    default:
      return c;
  }
}

const KNOWN = new Set(['User', 'Assistant', 'Thinking', 'Tool', 'Permission', 'Notice', 'Note']);

/** What Claude is doing now, from the last entries: the tool running, thinking, writing. */
export function workingOn(entries: AiEntry[]): string {
  for (let i = entries.length - 1; i >= 0; i--) {
    const e = entries[i];
    // Entries of a newer workspace show as nothing, and say nothing here.
    if (!e || !KNOWN.has(e.type)) continue;
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
