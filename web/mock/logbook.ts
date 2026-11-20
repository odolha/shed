// The logbook: commands run in the mock's tabs, searched by scope, words and tags.

import type { LogEntry } from '../src/api/gen/api.ts';
import { now } from './util.ts';

interface Row extends LogEntry {
  tags: string[];
}

const COMMANDS: [string, number | null, number][] = [
  ['cargo build', 0, 14.2],
  ['cargo test', 0, 31.8],
  ['cargo test -p shed-save', 101, 6.1],
  ['git status', 0, 0.04],
  ['git diff --stat', 0, 0.06],
  ['git log --oneline -5', 0, 0.03],
  ['npm run build', 0, 22.4],
  ['npm start', 130, 640],
  ['ssh buildbox', 0, 3600],
  ['ls -la', 0, 0.01],
  ['cargo clippy --all-targets', 0, 48.3],
  ['tail -f /var/log/app.log', 130, 210],
  ['docker compose up -d', 0, 8.7],
  ['make deploy', 2, 54.1],
  ['rg "parse_args" src', 0, 0.2],
];

let rows: Row[] | null = null;

/** Where each tab's commands ran: bench name, tab title, folder, tags. */
export interface Place {
  bench: string;
  tab: string;
  cwd: string;
  tags: string[];
}

function build(places: Place[]): Row[] {
  const out: Row[] = [];
  const t = now();
  for (let i = 0; i < 40; i++) {
    const [command, exit, duration] = COMMANDS[(i * 7) % COMMANDS.length]!;
    const place = places[i % places.length]!;
    out.push({ command, exit, duration, cwd: place.cwd, started: t - i * 1700 - 60, bench: place.bench, tab: place.tab, tags: place.tags });
  }
  return out;
}

export function logSearch(places: Place[], scope: 'tab' | 'bench' | 'all', here: Place | null, query: string, limit: number): LogEntry[] {
  rows ??= build(places);
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const tags = words.filter((w) => w.startsWith('tag:')).map((w) => w.slice(4));
  const plain = words.filter((w) => !w.startsWith('tag:'));
  return rows
    .filter((r) => scope === 'all' || !here || (r.bench === here.bench && (scope === 'bench' || r.tab === here.tab)))
    .filter((r) => tags.every((tag) => r.tags.some((x) => x.toLowerCase() === tag)))
    .filter((r) => plain.every((w) => r.command.toLowerCase().includes(w) || r.cwd.toLowerCase().includes(w)))
    .slice(0, limit || 200)
    .map(({ tags: _tags, ...entry }) => entry);
}
