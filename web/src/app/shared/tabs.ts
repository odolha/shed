/** Small helpers about benches and tabs, shared by the views. */

import type { AiTab, BenchRef, Layout, Pane, Tab } from '../../api/gen/api';

export type TabKindName = Tab['kind']['type'];

export function running(bench: string): BenchRef {
  return { type: 'Running', content: bench };
}

export function closed(name: string): BenchRef {
  return { type: 'Closed', content: name };
}

export function aiOf(tab: Tab | undefined | null): AiTab | null {
  return tab?.kind.type === 'Ai' ? tab.kind.content : null;
}

export function activePane(tab: Tab): Pane | undefined {
  return tab.panes?.find((p) => p.id === tab.activePane) ?? tab.panes?.[0];
}

/** The icon for a tab, as on Shed's tab bar: what it is, or that it waits. */
export function tabIcon(tab: Tab): string {
  if (tab.private) return 'private';
  switch (tab.kind.type) {
    case 'Project':
      return 'project';
    case 'Ai':
      if (tab.kind.content.activity === 'waiting') return 'tool-waiting';
      return tab.kind.content.browser ? 'globe' : 'ai';
    case 'Remote':
      return 'server';
    case 'Terminal': {
      const pane = activePane(tab);
      if (tab.panes?.some((p) => p.waiting)) return 'tool-waiting';
      return pane?.tool ? 'tool' : 'terminal';
    }
  }
}

/** Working / waiting, for AI tabs; terminals with a command waiting for Enter. */
export function tabActivity(tab: Tab): 'working' | 'waiting' | null {
  if (tab.kind.type === 'Ai') {
    const a = tab.kind.content.activity;
    return a === 'idle' ? null : a;
  }
  if (tab.panes?.some((p) => p.waiting)) return 'waiting';
  return null;
}

/** What runs in a tab or where it works, for a card's second line. */
export function tabDetail(tab: Tab, home: string): string {
  switch (tab.kind.type) {
    case 'Project':
      return tildify(tab.kind.content.root, home);
    case 'Ai':
      return tildify(tab.kind.content.folder, home);
    case 'Remote':
      return tab.kind.content.host;
    case 'Terminal': {
      const pane = activePane(tab);
      if (!pane) return '';
      return pane.tool ?? tildify(pane.cwd, home);
    }
  }
}

/** `/home/you/shed` → `~/shed`. */
export function tildify(path: string, home: string): string {
  if (!home || !path) return path;
  if (path === home) return '~';
  if (path.startsWith(home + '/')) return '~' + path.slice(home.length);
  return path;
}

/** The panes of a layout, in order. */
export function layoutPanes(layout: Layout | null | undefined): string[] {
  if (!layout) return [];
  if (layout.type === 'Pane') return [layout.content];
  return [...layoutPanes(layout.content.first), ...layoutPanes(layout.content.second)];
}

/** A colour readable as text on the background (bench colours on light themes). */
export function inkFor(color: string): string {
  return `color-mix(in srgb, ${color} 78%, var(--text))`;
}
