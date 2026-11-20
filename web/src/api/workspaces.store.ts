/**
 * The workspaces the nexus knows and the last state of each, as signals,
 * kept from `Welcome`, `Workspaces` and `State` frames.
 */

import { Injectable, computed, inject, signal } from '@angular/core';
import type {
  Bench,
  Call,
  ClosedBench,
  Reply,
  Tab,
  Welcome,
  WorkspaceInfo,
  WorkspaceState,
} from './gen/api';
import { NexusSocket, type ReplyOf } from './socket';

@Injectable({ providedIn: 'root' })
export class WorkspacesStore {
  readonly socket = inject(NexusSocket);

  readonly workspaces = signal<WorkspaceInfo[]>([]);
  readonly states = signal<Record<string, WorkspaceState>>({});
  /** Welcome came at least once (the lists are not just empty for lack of it). */
  readonly loaded = signal(false);
  readonly connected = computed(() => this.socket.state() === 'open');

  constructor() {
    const welcome = this.socket.welcome();
    if (welcome) this.welcome(welcome);
    this.socket.listen((frame) => {
      switch (frame.type) {
        case 'Welcome':
          this.welcome(frame.content);
          break;
        case 'Workspaces':
          this.workspaces.set(frame.content);
          break;
        case 'State':
          this.states.update((all) => ({ ...all, [frame.content.workspace]: frame.content.state }));
          break;
        default:
          break;
      }
    });
  }

  private welcome(w: Welcome): void {
    this.workspaces.set(w.workspaces);
    const states: Record<string, WorkspaceState> = {};
    for (const s of w.states) states[s.workspace] = s.state;
    this.states.set(states);
    this.loaded.set(true);
  }

  info(workspace: string): WorkspaceInfo | undefined {
    return this.workspaces().find((w) => w.id === workspace);
  }

  state(workspace: string): WorkspaceState | undefined {
    return this.states()[workspace];
  }

  bench(workspace: string, bench: string): Bench | undefined {
    return this.state(workspace)?.benches.find((b) => b.id === bench);
  }

  closedBench(workspace: string, name: string): ClosedBench | undefined {
    return this.state(workspace)?.closed.find((b) => b.name === name);
  }

  /** A running tab, with its bench. */
  tab(workspace: string, tab: string): { bench: Bench; tab: Tab } | undefined {
    for (const bench of this.state(workspace)?.benches ?? []) {
      const found = bench.tabs.find((t) => t.id === tab);
      if (found) return { bench, tab: found };
    }
    return undefined;
  }

  /** The workspace is reachable now. */
  online(workspace: string): boolean {
    return this.connected() && (this.info(workspace)?.online ?? false);
  }

  /** Connectors may change things there (online, full control). */
  canChange(workspace: string): boolean {
    const info = this.info(workspace);
    return this.connected() && !!info?.online && info.policy === 'full';
  }

  request(workspace: string, call: Call, timeout?: number): Promise<Reply> {
    return this.socket.request(workspace, call, timeout);
  }

  call<K extends Reply['type']>(workspace: string, call: Call, expect: K, timeout?: number): Promise<ReplyOf<K>> {
    return this.socket.call(workspace, call, expect, timeout);
  }
}
