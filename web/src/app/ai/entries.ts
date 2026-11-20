/** The parts of a conversation: messages, steps, sub-agents, notes, permission and question cards. */

import { ChangeDetectionStrategy, Component, computed, inject, input, output, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import type { AiEntry, AiNote, AiPermission, AiTextEdit, AiTool } from '../../api/gen/api';
import { WorkspacesStore } from '../../api/workspaces.store';
import { TPipe, t } from '../../i18n/i18n';
import { Clipboard } from '../../ui/clipboard';
import { Icon } from '../../ui/icon';
import { Notices } from '../../ui/notices';
import { AgoPipe, ClockPipe } from '../../ui/time';
import { Tooltip } from '../../ui/tooltip';
import { MarkdownDirective } from './markdown';
import { AiPictures } from './pictures';
import { AiLive, isAgent, runLine, runState, timerWhen, tokens } from './runs';

/** An edit as diff lines: what it took out, then what it put in. */
export function editLines(edit: AiTextEdit): { kind: 'removed' | 'added'; text: string }[] {
  const out: { kind: 'removed' | 'added'; text: string }[] = [];
  if (edit.old) for (const l of edit.old.split('\n')) out.push({ kind: 'removed', text: l });
  if (edit.new) for (const l of edit.new.split('\n')) out.push({ kind: 'added', text: l });
  return out;
}

@Component({
  selector: 'shed-ai-picture',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [TPipe],
  template: `
    @let p = state();
    @if (p && 'url' in p) {
      <a [href]="p.url" target="_blank" rel="noopener" class="thumb">
        <img [src]="p.url" [alt]="name()" loading="lazy" />
      </a>
    } @else if (p) {
      <span class="thumb missing small dim">{{ 'ai.pictureMissing' | t }}</span>
    } @else {
      <span class="thumb loading"></span>
    }
  `,
  styles: `
    .thumb {
      display: inline-flex;
      align-items: center;
      justify-content: center;
      width: 96px;
      height: 72px;
      border: 1px solid var(--divider);
      border-radius: var(--radius-sm);
      overflow: hidden;
      background: var(--surface);
    }
    img {
      width: 100%;
      height: 100%;
      object-fit: cover;
    }
    .missing {
      padding: 4px;
      text-align: center;
    }
  `,
})
export class AiPictureThumb {
  readonly workspace = input.required<string>();
  readonly tab = input.required<string>();
  readonly name = input.required<string>();
  private readonly pictures = inject(AiPictures);
  protected readonly state = computed(() => this.pictures.get(this.workspace(), this.tab(), this.name())());
}

/** A step's or tool's mark: how it is doing. */
type Mark = 'turning' | 'still' | 'pending' | 'done' | 'failed' | 'stopped';

const GLYPHS: Record<Exclude<Mark, 'turning'>, string> = {
  still: '◐',
  pending: '○',
  done: '✓',
  failed: '✗',
  stopped: '■',
};

/** The icon of one of Claude Code's notes, by its kind (one it does not know: a task's). */
export function noteIcon(note: AiNote): string {
  if (note.error) return 'alert';
  switch (note.kind) {
    case 'wakeup':
      return 'clock';
    case 'command':
      return 'terminal';
    case 'compacted':
      return 'history';
    case 'message':
      return 'send';
    case 'hook':
      return 'settings';
    default:
      return 'tool';
  }
}

@Component({
  selector: 'shed-ai-mark',
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    @if (mark() === 'turning') {
      <span class="spinner"></span>
    } @else {
      {{ glyph() }}
    }
  `,
  host: {
    '[class]': 'mark()',
    'aria-hidden': 'true',
  },
  styles: `
    :host {
      display: inline-flex;
      align-items: center;
      justify-content: center;
      flex: none;
      width: 14px;
      font-size: 12px;
      line-height: 1;
      color: var(--text-dim);
    }
    :host(.done),
    :host(.turning),
    :host(.still) {
      color: var(--accent);
    }
    :host(.failed) {
      color: var(--danger);
    }
    .spinner {
      width: 10px;
      height: 10px;
      border: 2px solid var(--divider);
      border-top-color: var(--accent);
      border-radius: 50%;
      animation: spin 0.8s linear infinite;
    }
    @keyframes spin {
      to {
        transform: rotate(360deg);
      }
    }
  `,
})
export class AiMark {
  readonly mark = input.required<Mark>();
  protected readonly glyph = computed(() => {
    const m = this.mark();
    return m === 'turning' ? '' : GLYPHS[m];
  });
}

/** A tool call; a sub-agent's or a background task's with how it runs, its steps and report. */
@Component({
  selector: 'shed-ai-tool',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [TPipe, Icon, Tooltip, AiPictureThumb, AiMark, MarkdownDirective],
  template: `
    @let tool = entry();
    @let run = tool.run;
    <div class="line" [class.err]="!run && tool.error">
      <button class="summary" (click)="open.set(!open())" [attr.aria-expanded]="open()">
        <shed-icon [name]="open() ? 'chevron-down' : 'chevron-right'" [size]="14" />
        @if (run) {
          <shed-ai-mark [mark]="mark()" [attr.title]="state()" />
        } @else {
          <shed-icon name="tool" [size]="14" />
        }
        <span class="ellipsis mono name" [class.agent]="agent()" [class.shrink]="!!info()">{{ tool.summary || tool.name }}</span>
        @if (run?.background) {
          <span class="tag">{{ 'ai.run.background' | t }}</span>
        }
        @if (info(); as i) {
          <span class="ellipsis info" [class.live]="state() === 'running'">{{ i }}</span>
        }
        @if (!run) {
          @if (tool.result === null || tool.result === undefined) {
            <span class="spinner" [attr.aria-label]="'ai.running' | t"></span>
          } @else if (tool.error) {
            <span class="chip danger">{{ 'ai.toolFailed' | t }}</span>
          }
        }
      </button>
      <button class="icon-btn small copy" (click)="clip.copy(tool.detail || tool.summary)" [shedTip]="'common.copy' | t">
        <shed-icon name="copy" [size]="14" />
      </button>
    </div>
    @if (open()) {
      <div class="detail">
        @if (run && agent()) {
          @if (tool.detail) {
            <pre class="code">{{ tool.detail }}</pre>
          }
          @if (agentMeta(); as m) {
            <div class="dim small">{{ m }}</div>
          }
          <div class="steps">
            @if ((run.skipped ?? 0) > 0) {
              <div class="dim small">{{ 'ai.run.skipped' | t: { n: run.skipped ?? 0 } }}</div>
            }
            @for (s of run.steps ?? []; track $index) {
              @switch (s.type) {
                @case ('Tool') {
                  @let key = s.content.id || 'step-' + $index;
                  @let more = stepHasMore(s.content);
                  <button class="step" [class.plain]="!more" (click)="more && toggleStep(key)" [attr.aria-expanded]="more ? stepOpen(key) : null">
                    @if (more) {
                      <shed-icon [name]="stepOpen(key) ? 'chevron-down' : 'chevron-right'" [size]="12" />
                    } @else {
                      <span class="pad"></span>
                    }
                    <shed-ai-mark [mark]="stepMark(s.content)" />
                    <span class="ellipsis mono">{{ s.content.summary || s.content.name }}</span>
                  </button>
                  @if (more && stepOpen(key)) {
                    <div class="step-more">
                      @if (s.content.edit; as e) {
                        <div class="block">
                          @for (l of editLines(e); track $index) {
                            <div class="diff-line nonum" [class.added]="l.kind === 'added'" [class.removed]="l.kind === 'removed'">
                              <span class="sign">{{ l.kind === 'added' ? '+' : '−' }}</span><span>{{ l.text }}</span>
                            </div>
                          }
                        </div>
                      } @else if (s.content.detail) {
                        <pre class="code">{{ s.content.detail }}</pre>
                      }
                      @if (s.content.result) {
                        <pre class="code" [class.danger-text]="s.content.error">{{ s.content.result }}</pre>
                      }
                    </div>
                  }
                }
                @case ('Assistant') {
                  <div class="said" [shedMarkdown]="s.content.text"></div>
                }
              }
            } @empty {
              <div class="dim small">{{ (state() === 'running' ? 'ai.run.noSteps' : 'ai.run.stepsUnknown') | t }}</div>
            }
          </div>
          @if (report(); as r) {
            <div class="dim small label">{{ 'ai.run.report' | t }}</div>
            <div [shedMarkdown]="r"></div>
          }
        } @else {
          @if (tool.edit; as e) {
            <div class="block">
              @for (l of editLines(e); track $index) {
                <div class="diff-line nonum" [class.added]="l.kind === 'added'" [class.removed]="l.kind === 'removed'">
                  <span class="sign">{{ l.kind === 'added' ? '+' : '−' }}</span><span>{{ l.text }}</span>
                </div>
              }
            </div>
          } @else if (tool.detail) {
            <pre class="code">{{ tool.detail }}</pre>
          }
          @if (tool.result) {
            <div class="dim small label">{{ 'ai.result' | t }}</div>
            <pre class="code" [class.danger-text]="tool.error">{{ tool.result }}</pre>
          }
          @if (tool.pictures?.length) {
            <div class="row wrap">
              @for (p of tool.pictures ?? []; track p) {
                <shed-ai-picture [workspace]="workspace()" [tab]="tab()" [name]="p" />
              }
            </div>
          }
        }
        <button class="btn ghost small" (click)="open.set(false)">
          <shed-icon name="arrow-up" [size]="13" />{{ 'ai.collapse' | t }}
        </button>
      </div>
    }
  `,
  styles: `
    .line {
      display: flex;
      align-items: center;
      gap: 4px;
    }
    .summary {
      display: flex;
      align-items: center;
      gap: 6px;
      flex: 1;
      min-width: 0;
      padding: 3px 6px;
      border: 0;
      border-radius: var(--radius-sm);
      background: none;
      color: var(--text-dim);
      font: inherit;
      font-size: 13px;
      text-align: left;
      cursor: pointer;
    }
    .summary:hover {
      background: var(--hover);
      color: var(--text);
    }
    .err .summary {
      color: var(--danger);
    }
    .name.agent {
      color: var(--text);
    }
    .name.shrink {
      flex: 0 1 auto;
      max-width: 60%;
    }
    .info {
      flex: 1;
      min-width: 0;
      font-size: 12px;
    }
    .info.live {
      color: var(--accent);
    }
    .tag {
      flex: none;
      padding: 0 4px;
      border: 1px solid var(--divider);
      border-radius: var(--radius-sm);
      font-size: 11px;
    }
    .copy {
      visibility: hidden;
    }
    .line:hover .copy,
    .copy:focus-visible {
      visibility: visible;
    }
    .detail {
      display: flex;
      flex-direction: column;
      gap: 6px;
      margin: 4px 0 8px 22px;
    }
    .detail .code {
      max-height: 360px;
    }
    .block {
      border: 1px solid var(--divider);
      border-radius: var(--radius-sm);
      overflow: auto;
      max-height: 360px;
      background: var(--surface);
    }
    .diff-line.nonum {
      grid-template-columns: 1.4em 1fr;
    }
    .label {
      margin-top: 2px;
    }
    .detail > .btn {
      align-self: flex-start;
    }
    .steps {
      display: flex;
      flex-direction: column;
      gap: 2px;
      padding-left: 10px;
      border-left: 2px solid var(--divider);
    }
    .step {
      display: flex;
      align-items: center;
      gap: 6px;
      min-width: 0;
      padding: 2px 6px;
      border: 0;
      border-radius: var(--radius-sm);
      background: none;
      color: var(--text-dim);
      font: inherit;
      font-size: 12.5px;
      text-align: left;
      cursor: pointer;
    }
    .step:hover:not(.plain) {
      background: var(--hover);
      color: var(--text);
    }
    .step.plain {
      cursor: default;
    }
    .pad {
      width: 12px;
      flex: none;
    }
    .step-more {
      display: flex;
      flex-direction: column;
      gap: 4px;
      margin: 2px 0 6px 20px;
    }
    .step-more .code,
    .step-more .block {
      max-height: 280px;
    }
    .said {
      margin: 2px 0 2px 6px;
      font-size: 13.5px;
    }
    .spinner {
      width: 10px;
      height: 10px;
      border: 2px solid var(--divider);
      border-top-color: var(--accent);
      border-radius: 50%;
      animation: spin 0.8s linear infinite;
      flex: none;
    }
    @keyframes spin {
      to {
        transform: rotate(360deg);
      }
    }
  `,
})
export class AiToolEntry {
  readonly entry = input.required<AiTool>();
  readonly workspace = input.required<string>();
  readonly tab = input.required<string>();
  protected readonly open = signal(false);
  protected readonly clip = inject(Clipboard);
  protected readonly editLines = editLines;
  private readonly live = inject(AiLive, { optional: true }) ?? new AiLive();
  private readonly openSteps = signal<ReadonlySet<string>>(new Set());

  protected readonly agent = computed(() => isAgent(this.entry()));
  protected readonly state = computed(() => {
    const run = this.entry().run;
    return run ? runState(run) : null;
  });
  protected readonly mark = computed<Mark>(() => {
    switch (this.state()) {
      case 'running':
        // Still while Claude is idle: a server left running would keep it turning.
        return this.live.busy() ? 'turning' : 'still';
      case 'failed':
        return 'failed';
      case 'stopped':
        return 'stopped';
      default:
        return 'done';
    }
  });
  /** After its line: how a run is doing; when a timer it set fires. */
  protected readonly info = computed(() => {
    const tool = this.entry();
    const now = this.live.now();
    if (tool.run) return runLine(tool, tool.run, now);
    const timer = tool.id ? this.live.timers().find((x) => x.tool === tool.id) : undefined;
    return timer ? t('ai.timer.fires', { when: timerWhen(timer, now) }) : null;
  });
  protected readonly agentMeta = computed(() => {
    const run = this.entry().run;
    if (!run) return '';
    const parts: string[] = [];
    if (run.agent) parts.push(run.agent);
    if ((run.tokens ?? 0) > 0) parts.push(t('ai.run.tokens', { count: tokens(run.tokens ?? 0) }));
    return parts.join(' · ');
  });
  protected readonly report = computed(() => {
    const tool = this.entry();
    const run = tool.run;
    if (!run) return '';
    return run.report || (runState(run) !== 'running' ? (tool.result ?? '') : '');
  });

  protected stepMark(s: AiTool): Mark {
    if (s.result === null || s.result === undefined) {
      if (this.state() !== 'running') return 'pending';
      return this.live.busy() ? 'turning' : 'still';
    }
    return s.error ? 'failed' : 'done';
  }

  protected stepHasMore(s: AiTool): boolean {
    return !!(s.result || s.edit || s.detail);
  }

  protected stepOpen(key: string): boolean {
    return this.openSteps().has(key);
  }

  protected toggleStep(key: string): void {
    this.openSteps.update((old) => {
      const next = new Set(old);
      if (!next.delete(key)) next.add(key);
      return next;
    });
  }
}

/** One of Claude Code's own notes: a line of its kind, its detail folded. */
@Component({
  selector: 'shed-ai-note',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [Icon, MarkdownDirective, ClockPipe],
  template: `
    @let n = entry();
    <button class="line" [class.err]="n.error" [class.plain]="!n.detail" (click)="n.detail && open.set(!open())" [attr.aria-expanded]="n.detail ? open() : null">
      @if (n.detail) {
        <shed-icon [name]="open() ? 'chevron-down' : 'chevron-right'" [size]="14" />
      }
      <shed-icon [name]="icon()" [size]="14" [class.accent]="!n.error && n.kind === 'wakeup'" />
      <span class="ellipsis grow">{{ n.title }}</span>
      @if (n.at) {
        <span class="at">{{ n.at | clock }}</span>
      }
    </button>
    @if (open() && n.detail) {
      @if (n.kind === 'command' || n.kind === 'hook') {
        <pre class="code more">{{ n.detail }}</pre>
      } @else {
        <div class="more" [shedMarkdown]="n.detail"></div>
      }
    }
  `,
  styles: `
    :host {
      display: block;
      margin: 2px 0;
    }
    .line {
      display: flex;
      align-items: center;
      gap: 6px;
      width: 100%;
      min-width: 0;
      padding: 3px 6px;
      border: 0;
      border-radius: var(--radius-sm);
      background: none;
      color: var(--text-dim);
      font: inherit;
      font-size: 13px;
      text-align: left;
      cursor: pointer;
    }
    .line:hover:not(.plain) {
      background: var(--hover);
      color: var(--text);
    }
    .line.plain {
      cursor: default;
    }
    .line.err {
      color: var(--danger);
    }
    .accent {
      color: var(--accent);
    }
    .at {
      flex: none;
      font-size: 12px;
      opacity: 0.85;
    }
    .more {
      margin: 4px 0 8px 22px;
      font-size: 13.5px;
    }
    pre.more {
      max-height: 360px;
    }
  `,
})
export class AiNoteEntry {
  readonly entry = input.required<AiNote>();
  protected readonly open = signal(false);
  protected readonly icon = computed(() => noteIcon(this.entry()));
}

/** Claude asks to use a tool, or asks questions. */
@Component({
  selector: 'shed-ai-permission',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [TPipe, Icon, FormsModule],
  template: `
    @let p = entry();
    <div class="card perm" [class.pending]="pending()">
      @if (p.questions?.length) {
        <div class="title"><shed-icon name="question" />{{ 'ai.questions' | t }}</div>
        @for (q of p.questions; track $index; let qi = $index) {
          <fieldset class="q" [disabled]="!pending() || !canChange()">
            <legend>
              @if (q.header) {
                <span class="chip accent">{{ q.header }}</span>
              }
              <span class="qtext">{{ q.question }}</span>
            </legend>
            @for (o of q.options; track $index) {
              <label class="opt">
                <input
                  [type]="q.multi ? 'checkbox' : 'radio'"
                  [name]="'q' + qi + '-' + p.request"
                  [checked]="isChosen(qi, o.label)"
                  (change)="choose(qi, o.label, q.multi ?? false)"
                />
                <span class="col" style="gap: 0">
                  <span>{{ o.label }}</span>
                  @if (o.description) {
                    <span class="dim small">{{ o.description }}</span>
                  }
                </span>
              </label>
            }
            <label class="opt other">
              <span class="small dim">{{ 'ai.other' | t }}</span>
              <input
                class="input"
                [ngModel]="others()[qi] ?? ''"
                (ngModelChange)="setOther(qi, $event)"
                (keydown.enter)="$event.preventDefault(); sendAnswers()"
                [placeholder]="'ai.otherPlaceholder' | t"
              />
            </label>
          </fieldset>
        }
        @if (pending()) {
          @if (canChange()) {
            <div class="actions">
              <button class="btn ghost" (click)="answer(false, false)">{{ 'ai.skip' | t }}</button>
              <button class="btn primary" [disabled]="!ready()" (click)="sendAnswers()">
                <shed-icon name="send" />{{ 'ai.sendAnswers' | t }}
              </button>
            </div>
          }
        } @else {
          <div class="answered small">
            @if (p.answers?.length) {
              @for (a of p.answers; track $index) {
                <div><span class="dim">{{ 'ai.answered' | t }}</span> {{ a }}</div>
              }
            } @else {
              <span class="dim">{{ 'ai.skipped' | t }}</span>
            }
          </div>
        }
      } @else {
        <div class="title">
          <shed-icon name="tool-waiting" />
          <span>{{ 'ai.permission' | t: { tool: p.tool } }}</span>
        </div>
        <div class="mono summary">{{ p.summary }}</div>
        @if (p.edit; as e) {
          <div class="block">
            @for (l of editLines(e); track $index) {
              <div class="diff-line nonum" [class.added]="l.kind === 'added'" [class.removed]="l.kind === 'removed'">
                <span class="sign">{{ l.kind === 'added' ? '+' : '−' }}</span><span>{{ l.text }}</span>
              </div>
            }
          </div>
        } @else if (p.detail && p.detail !== p.summary) {
          <pre class="code">{{ p.detail }}</pre>
        }
        @if (pending()) {
          @if (canChange()) {
            <div class="actions">
              <button class="btn danger" (click)="answer(false, false)">{{ 'ai.deny' | t }}</button>
              <button class="btn" (click)="answer(true, true)">{{ 'ai.allowSession' | t }}</button>
              <button class="btn primary" (click)="answer(true, false)"><shed-icon name="check" />{{ 'ai.allow' | t }}</button>
            </div>
          } @else {
            <div class="dim small">{{ 'ai.waitsForAnswer' | t }}</div>
          }
        } @else {
          <div class="small" [class.danger-text]="p.answer === false" [class.dim]="p.answer !== false">
            {{ (p.answer ? 'ai.allowed' : 'ai.denied') | t }}
          </div>
        }
      }
    </div>
  `,
  styles: `
    .perm {
      padding: 12px;
      display: flex;
      flex-direction: column;
      gap: 8px;
    }
    .perm.pending {
      border-color: color-mix(in srgb, var(--accent) 60%, var(--divider));
      box-shadow: 0 0 0 1px color-mix(in srgb, var(--accent) 30%, transparent);
    }
    .title {
      display: flex;
      align-items: center;
      gap: 8px;
      font-weight: 600;
    }
    .summary {
      font-size: 13px;
      word-break: break-word;
    }
    .code {
      max-height: 240px;
    }
    .block {
      border: 1px solid var(--divider);
      border-radius: var(--radius-sm);
      overflow: auto;
      max-height: 280px;
    }
    .diff-line.nonum {
      grid-template-columns: 1.4em 1fr;
    }
    .actions {
      display: flex;
      flex-wrap: wrap;
      justify-content: flex-end;
      gap: 8px;
    }
    fieldset {
      border: 0;
      margin: 0;
      padding: 0;
      display: flex;
      flex-direction: column;
      gap: 4px;
    }
    legend {
      display: flex;
      flex-wrap: wrap;
      gap: 6px;
      align-items: center;
      padding: 0;
      margin-bottom: 4px;
    }
    .qtext {
      font-weight: 500;
    }
    .opt {
      display: flex;
      align-items: flex-start;
      gap: 8px;
      padding: 6px 8px;
      border-radius: var(--radius-sm);
      cursor: pointer;
    }
    .opt:hover {
      background: var(--hover);
    }
    .opt input[type='radio'],
    .opt input[type='checkbox'] {
      accent-color: var(--accent);
      margin-top: 3px;
    }
    .opt.other {
      flex-direction: column;
      gap: 4px;
      cursor: default;
    }
  `,
})
export class AiPermissionCard {
  readonly entry = input.required<AiPermission>();
  readonly workspace = input.required<string>();
  readonly tab = input.required<string>();
  readonly canChange = input(false);
  private readonly store = inject(WorkspacesStore);
  private readonly notices = inject(Notices);
  protected readonly editLines = editLines;
  protected readonly chosen = signal<string[][]>([]);
  protected readonly others = signal<string[]>([]);
  /** Answered here, before the transcript says so. */
  private readonly sent = signal(false);

  protected readonly pending = computed(() => {
    const p = this.entry();
    if (this.sent()) return false;
    return p.questions?.length ? !p.answers?.length && (p.answer === null || p.answer === undefined) : p.answer === null || p.answer === undefined;
  });

  protected readonly ready = computed(() => {
    const n = this.entry().questions?.length ?? 0;
    for (let i = 0; i < n; i++) {
      if (!(this.chosen()[i]?.length || this.others()[i]?.trim())) return false;
    }
    return n > 0;
  });

  protected isChosen(q: number, label: string): boolean {
    return this.chosen()[q]?.includes(label) ?? false;
  }

  protected choose(q: number, label: string, multi: boolean): void {
    this.chosen.update((all) => {
      const next = [...all];
      const cur = next[q] ?? [];
      next[q] = multi ? (cur.includes(label) ? cur.filter((l) => l !== label) : [...cur, label]) : [label];
      return next;
    });
    // One question with one answer wanted: choosing is answering.
    const qs = this.entry().questions ?? [];
    if (qs.length === 1 && !multi && !this.others()[0]?.trim()) void this.sendAnswers();
  }

  protected setOther(q: number, text: string): void {
    this.others.update((all) => {
      const next = [...all];
      next[q] = text;
      return next;
    });
  }

  protected async sendAnswers(): Promise<void> {
    if (!this.ready() || !this.pending()) return;
    const answers = (this.entry().questions ?? []).map((_, i) => {
      const parts = [...(this.chosen()[i] ?? [])];
      const other = this.others()[i]?.trim();
      if (other) parts.push(other);
      return parts.join(', ');
    });
    this.sent.set(true);
    const r = await this.notices.run(
      this.store.request(this.workspace(), {
        type: 'AiAnswerQuestions',
        content: { tab: this.tab(), request: this.entry().request, answers },
      }),
    );
    if (r === undefined) this.sent.set(false);
  }

  protected async answer(allow: boolean, always: boolean): Promise<void> {
    this.sent.set(true);
    const r = await this.notices.run(
      this.store.request(this.workspace(), {
        type: 'AiAnswer',
        content: { tab: this.tab(), request: this.entry().request, allow, always },
      }),
    );
    if (r === undefined) this.sent.set(false);
  }
}

export interface UserAction {
  kind: 'edit' | 'resend' | 'revert';
  index: number;
  text: string;
}

/** One entry of the conversation. */
@Component({
  selector: 'shed-ai-entry',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [TPipe, Icon, MarkdownDirective, AiToolEntry, AiNoteEntry, AiPermissionCard, AiPictureThumb, AgoPipe, ClockPipe],
  template: `
    @let e = entry();
    @switch (e.type) {
      @case ('User') {
        <div class="user">
          <div class="bubble">
            <div class="text">{{ e.content.text }}</div>
            @if (e.content.pictures?.length) {
              <div class="row wrap pics">
                @for (p of e.content.pictures ?? []; track p) {
                  <shed-ai-picture [workspace]="workspace()" [tab]="tab()" [name]="p" />
                }
              </div>
            } @else if (e.content.images) {
              <div class="dim small">{{ 'ai.images' | t: { n: e.content.images } }}</div>
            }
          </div>
          <div class="meta">
            @if (e.content.at) {
              <span class="dim small">{{ e.content.at | clock }} · {{ e.content.at | ago }}</span>
            }
            <span class="grow"></span>
            <span class="acts">
              <button class="btn ghost small" (click)="clip.copy(e.content.text)">
                <shed-icon name="copy" [size]="13" />{{ 'common.copy' | t }}
              </button>
              @if (canChange()) {
                <button class="btn ghost small" (click)="act.emit({ kind: 'edit', index: index(), text: e.content.text })">
                  <shed-icon name="pencil" [size]="13" />{{ 'common.edit' | t }}
                </button>
                <button class="btn ghost small" (click)="act.emit({ kind: 'resend', index: index(), text: e.content.text })">
                  <shed-icon name="refresh" [size]="13" />{{ 'ai.sendAgain' | t }}
                </button>
                <button class="btn ghost small danger" (click)="act.emit({ kind: 'revert', index: index(), text: e.content.text })">
                  <shed-icon name="discard" [size]="13" />{{ 'ai.revert' | t }}
                </button>
              }
            </span>
          </div>
        </div>
      }
      @case ('Assistant') {
        <div class="assistant">
          @if (e.content.at) {
            <div class="dim small meta">{{ e.content.at | clock }} · {{ e.content.at | ago }}</div>
          }
          <div [shedMarkdown]="e.content.text"></div>
          <div class="acts">
            <button class="btn ghost small" (click)="clip.copy(e.content.text)">
              <shed-icon name="copy" [size]="13" />{{ 'common.copy' | t }}
            </button>
          </div>
        </div>
      }
      @case ('Thinking') {
        <div class="thinking">
          <button class="fold" (click)="open.set(!open())" [attr.aria-expanded]="open()">
            <shed-icon [name]="open() ? 'chevron-down' : 'chevron-right'" [size]="14" />
            <shed-icon name="thinking" [size]="14" />{{ 'ai.thinking' | t }}
          </button>
          @if (open()) {
            <div class="think-text">{{ e.content.text }}</div>
          }
        </div>
      }
      @case ('Tool') {
        <shed-ai-tool [entry]="e.content" [workspace]="workspace()" [tab]="tab()" />
      }
      @case ('Permission') {
        <shed-ai-permission [entry]="e.content" [workspace]="workspace()" [tab]="tab()" [canChange]="canChange()" />
      }
      @case ('Notice') {
        <div class="notice small" [class.danger-text]="e.content.error">{{ e.content.text }}</div>
      }
      @case ('Note') {
        <shed-ai-note [entry]="e.content" />
      }
      @default {
        <!-- An entry of a newer workspace: it keeps its place and shows as nothing. -->
      }
    }
  `,
  styles: `
    :host {
      display: block;
    }
    .user {
      margin: 14px 0 6px;
    }
    .bubble {
      padding: 10px 12px;
      border-radius: var(--radius);
      background: color-mix(in srgb, var(--accent) 9%, var(--surface));
      border: 1px solid color-mix(in srgb, var(--accent) 22%, var(--divider));
    }
    .text {
      white-space: pre-wrap;
      overflow-wrap: anywhere;
    }
    .pics {
      margin-top: 8px;
    }
    .meta {
      display: flex;
      align-items: center;
      gap: 4px;
      min-height: 26px;
      flex-wrap: wrap;
    }
    .acts {
      display: inline-flex;
      flex-wrap: wrap;
      gap: 2px;
      opacity: 0;
      transition: opacity 0.1s;
    }
    :host:hover .acts,
    .acts:focus-within {
      opacity: 1;
    }
    @media (pointer: coarse) {
      .acts {
        opacity: 1;
      }
    }
    .assistant {
      margin: 6px 0;
    }
    .assistant .meta {
      min-height: 0;
      margin-bottom: 2px;
    }
    .assistant .acts {
      margin-top: 2px;
    }
    .fold {
      display: flex;
      align-items: center;
      gap: 6px;
      padding: 3px 6px;
      border: 0;
      border-radius: var(--radius-sm);
      background: none;
      color: var(--text-dim);
      font: inherit;
      font-size: 13px;
      cursor: pointer;
    }
    .fold:hover {
      background: var(--hover);
    }
    .think-text {
      margin: 4px 0 8px 22px;
      color: var(--text-dim);
      font-style: italic;
      white-space: pre-wrap;
    }
    .notice {
      margin: 8px 0 14px;
      text-align: center;
      color: var(--text-dim);
    }
  `,
})
export class AiEntryView {
  readonly entry = input.required<AiEntry>();
  readonly index = input.required<number>();
  readonly workspace = input.required<string>();
  readonly tab = input.required<string>();
  readonly canChange = input(false);
  readonly act = output<UserAction>();
  protected readonly open = signal(false);
  protected readonly clip = inject(Clipboard);
}
