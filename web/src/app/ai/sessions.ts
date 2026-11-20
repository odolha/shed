import { ChangeDetectionStrategy, Component, computed, effect, inject, input, output, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import type { AiSession } from '../../api/gen/api';
import { errorText } from '../../api/errors';
import { WorkspacesStore } from '../../api/workspaces.store';
import { TPipe, t } from '../../i18n/i18n';
import { Dialogs } from '../../ui/dialog';
import { Icon } from '../../ui/icon';
import { ContextMenuTrigger, Menu } from '../../ui/menu';
import { Notices } from '../../ui/notices';
import { AgoPipe } from '../../ui/time';
import { Tooltip } from '../../ui/tooltip';

/** The folder's Claude Code sessions, newest first, draft sessions on top. */
@Component({
  selector: 'shed-ai-sessions',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [FormsModule, TPipe, Icon, AgoPipe, Tooltip, Menu, ContextMenuTrigger],
  template: `
    <div class="top">
      <input
        class="input"
        type="search"
        [placeholder]="'ai.sessions.filter' | t"
        [attr.aria-label]="'ai.sessions.filter' | t"
        [ngModel]="filter()"
        (ngModelChange)="filter.set($event)"
      />
      @if (canChange()) {
        <button class="btn" (click)="open(null)"><shed-icon name="plus" />{{ 'ai.sessions.new' | t }}</button>
      }
    </div>
    <div class="list" role="list">
      @if (error(); as e) {
        <div class="error-box small">{{ e }}</div>
      }
      @for (s of shown(); track s.id) {
        <button
          class="session"
          role="listitem"
          [class.current]="s.id === current()"
          [style.padding-left.px]="10 + (s.depth ?? 0) * 14"
          (click)="open(s.id)"
          (dblclick)="rename(s)"
          [shedTip]="s.title"
          [shedContextMenu]="canChange() ? menu : null"
          (contextmenu)="menuFor = s"
          [disabled]="!canChange() && s.id !== current()"
        >
          <span class="row" style="gap: 6px">
            @if (s.draft) {
              <shed-icon name="pencil" [size]="13" />
            } @else if (s.depth) {
              <span class="dim">↳</span>
            }
            <span class="ellipsis title">{{ s.title || ('ai.sessions.untitled' | t) }}</span>
          </span>
          <span class="dim small ellipsis">
            @if (s.draft) {
              {{ 'ai.sessions.draft' | t: { n: s.drafts ?? 0 } }}
            } @else {
              {{ s.modified | ago }}
              @if (s.branch) {
                · {{ s.branch }}
              }
              · {{ 'ai.sessions.prompts' | t: { n: s.prompts } }}
              @if (s.drafts) {
                · {{ 'ai.sessions.drafts' | t: { n: s.drafts } }}
              }
            }
          </span>
        </button>
      } @empty {
        @if (!loading()) {
          <div class="empty small">{{ 'ai.sessions.none' | t }}</div>
        }
      }
      @if (loading()) {
        <div class="empty small">{{ 'common.loading' | t }}</div>
      }
    </div>
    <shed-menu #menu>
      @if (menuFor; as s) {
        <button class="menu-item" (click)="open(s.id)"><shed-icon name="go" />{{ 'ai.sessions.show' | t }}</button>
        <button class="menu-item" (click)="rename(s)"><shed-icon name="pencil" />{{ 'common.rename' | t }}</button>
      }
    </shed-menu>
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      min-height: 0;
      gap: 8px;
      padding: 8px;
    }
    .top {
      display: flex;
      gap: 6px;
    }
    .list {
      flex: 1;
      overflow: auto;
      display: flex;
      flex-direction: column;
      gap: 2px;
    }
    .session {
      display: flex;
      flex-direction: column;
      align-items: stretch;
      gap: 1px;
      padding: 6px 10px;
      border: 0;
      border-radius: var(--radius-sm);
      background: none;
      color: var(--text);
      font: inherit;
      text-align: left;
      cursor: pointer;
      min-width: 0;
    }
    .session:hover:not(:disabled) {
      background: var(--hover);
    }
    .session:disabled {
      cursor: default;
    }
    .session.current {
      background: color-mix(in srgb, var(--accent) 14%, var(--surface));
    }
    .session.current .title {
      font-weight: 600;
    }
  `,
})
export class AiSessionsPanel {
  readonly workspace = input.required<string>();
  readonly tab = input.required<string>();
  /** The session (or draft session) shown. */
  readonly current = input<string | null>(null);
  readonly canChange = input(false);
  /** Bumped by the tab to reload (a turn ended, drafts changed). */
  readonly reload = input(0);
  readonly opened = output<void>();

  private readonly store = inject(WorkspacesStore);
  private readonly dialogs = inject(Dialogs);
  private readonly notices = inject(Notices);
  protected readonly sessions = signal<AiSession[]>([]);
  protected readonly loading = signal(false);
  protected readonly error = signal<string | null>(null);
  protected readonly filter = signal('');
  protected menuFor: AiSession | null = null;

  protected readonly shown = computed(() => {
    const q = this.filter().trim().toLowerCase();
    const all = this.sessions();
    const list = q ? all.filter((s) => s.title.toLowerCase().includes(q) || (s.branch ?? '').toLowerCase().includes(q)) : all;
    return [...list.filter((s) => s.draft), ...list.filter((s) => !s.draft)];
  });

  constructor() {
    effect(() => {
      this.reload();
      this.current();
      void this.load(this.workspace(), this.tab());
    });
  }

  private async load(ws: string, tab: string): Promise<void> {
    this.loading.set(true);
    try {
      this.sessions.set(await this.store.call(ws, { type: 'AiSessions', content: { tab } }, 'Sessions'));
      this.error.set(null);
    } catch (e) {
      this.error.set(errorText(e));
    } finally {
      this.loading.set(false);
    }
  }

  protected async open(session: string | null): Promise<void> {
    if (session !== null && session === this.current()) {
      this.opened.emit();
      return;
    }
    const r = await this.notices.run(
      this.store.request(this.workspace(), { type: 'AiOpenSession', content: { tab: this.tab(), session } }),
    );
    if (r !== undefined) this.opened.emit();
  }

  protected async rename(s: AiSession): Promise<void> {
    if (!this.canChange() || s.draft) return;
    const name = await this.dialogs.prompt({
      title: t('ai.sessions.rename'),
      body: t('ai.sessions.renameBody'),
      value: s.title,
      ok: t('common.save'),
      allowEmpty: true,
    });
    if (name === null) return;
    const r = await this.notices.run(
      this.store.request(this.workspace(), { type: 'AiRenameSession', content: { tab: this.tab(), session: s.id, name } }),
    );
    if (r !== undefined) void this.load(this.workspace(), this.tab());
  }
}
