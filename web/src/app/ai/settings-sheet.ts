import { ChangeDetectionStrategy, Component, type Signal, computed, inject } from '@angular/core';
import type { AiInfo, AiSettings, AiTab } from '../../api/gen/api';
import { TPipe, t } from '../../i18n/i18n';
import { DialogRef } from '../../ui/dialog';
import { Icon } from '../../ui/icon';
import { tildify } from '../shared/tabs';

/** Claude Code's permission modes: its value, the menu's label key. */
export const MODES: { value: string; key: string }[] = [
  { value: 'default', key: 'ai.mode.default' },
  { value: 'acceptEdits', key: 'ai.mode.acceptEdits' },
  { value: 'auto', key: 'ai.mode.auto' },
  { value: 'plan', key: 'ai.mode.plan' },
];
export const FULL_AUTO = 'bypassPermissions';

/** One choice of the sheet: its value, its name, a line about it, whether it is the tab's. */
export interface SettingChoice {
  value: string;
  name: string;
  detail?: string;
  selected: boolean;
}

/** What the settings sheet offers, from the tab and what Claude Code says it has. */
export interface SettingsChoices {
  models: SettingChoice[];
  /** Empty when the model has no efforts to choose; the first is the model's own (`""`). */
  efforts: SettingChoice[];
  modes: SettingChoice[];
  fullAuto: boolean;
  follow: boolean;
  browser: boolean;
  folders: string[];
}

/** The sheet's choices for a tab: its model (the default when none), effort, mode, switches and folders. */
export function settingsChoices(ai: AiTab, info: AiInfo | null): SettingsChoices {
  const current = ai.model ?? '';
  const all = info?.models ?? [];
  const model = all.find((m) => m.value === current) ?? all.find((m) => m.value === '');
  const models: SettingChoice[] = all.map((m) => ({
    value: m.value,
    name: m.name,
    detail: m.description || undefined,
    selected: m === model,
  }));
  if (!models.length) models.push({ value: current, name: current || t('ai.modelDefault'), selected: true });
  const effort = ai.effort ?? '';
  const list = model?.efforts ?? [];
  const efforts: SettingChoice[] = list.length
    ? [
        { value: '', name: t('ai.effortModel'), selected: effort === '' },
        ...list.map((e) => ({ value: e, name: e, selected: e === effort })),
      ]
    : [];
  const modes: SettingChoice[] = MODES.map((m) => ({ value: m.value, name: t(m.key), selected: m.value === ai.mode }));
  // A mode this version does not know, as the workspace says it.
  if (ai.mode !== FULL_AUTO && !MODES.some((m) => m.value === ai.mode)) {
    modes.push({ value: ai.mode, name: ai.mode, selected: true });
  }
  return {
    models,
    efforts,
    modes,
    fullAuto: ai.mode === FULL_AUTO,
    follow: !!ai.follow,
    browser: !!ai.browser,
    folders: ai.addDirs ?? [],
  };
}

/** What picking a mode does: nothing (it is the tab's), asks first (Full Auto), or the change. */
export type ModePick = { kind: 'same' } | { kind: 'ask' } | { kind: 'change'; mode: string };

export function pickMode(current: string, value: string): ModePick {
  if (value === current) return { kind: 'same' };
  if (value === FULL_AUTO) return { kind: 'ask' };
  return { kind: 'change', mode: value };
}

/** The folders with `path` added (once). */
export function withFolder(folders: string[], path: string): string[] {
  const bare = (p: string) => p.replace(/\/+$/, '');
  if (!path.trim() || folders.some((f) => bare(f) === bare(path))) return folders;
  return [...folders, path];
}

/** The folders without `path`. */
export function withoutFolder(folders: string[], path: string): string[] {
  return folders.filter((f) => f !== path);
}

/** What the sheet works with: the tab as it changes, and what to do. */
export interface SettingsSheetData {
  ai: Signal<AiTab>;
  info: Signal<AiInfo | null>;
  canChange: Signal<boolean>;
  home: Signal<string>;
  change: (change: Omit<AiSettings, 'tab'>) => void;
  /** Full Auto, asked first. */
  fullAuto: () => Promise<void>;
  /** A folder for Claude to work in too, chosen; null: none. */
  pickFolder: () => Promise<string | null>;
}

/**
 * The AI tab's settings, from the icon beside Send on narrow screens: the
 * model, its effort, the mode (Full Auto asks first), following edits, the
 * browser and the folders Claude works in besides the tab's.
 */
@Component({
  selector: 'shed-ai-settings-sheet',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [TPipe, Icon],
  template: `
    @let c = choices();
    @let can = d.canChange();
    <div class="dialog-body sheet-body">
      <div class="row">
        <h2 class="grow">{{ 'ai.settings' | t }}</h2>
        <button class="icon-btn" (click)="ref.close()" [attr.aria-label]="'common.close' | t"><shed-icon name="close" [size]="18" /></button>
      </div>

      <section>
        <h3 class="label">{{ 'ai.model' | t }}</h3>
        @for (m of c.models; track m.value) {
          <button class="choice" role="radio" [attr.aria-checked]="m.selected" [class.on]="m.selected" [disabled]="!can" (click)="d.change({ model: m.value })">
            <span class="col grow">
              <span>{{ m.name }}</span>
              @if (m.detail) {
                <span class="dim small">{{ m.detail }}</span>
              }
            </span>
            @if (m.selected) {
              <shed-icon name="check" />
            }
          </button>
        }
      </section>

      @if (c.efforts.length) {
        <section>
          <h3 class="label">{{ 'ai.effort' | t }}</h3>
          <div class="pills" role="radiogroup" [attr.aria-label]="'ai.effort' | t">
            @for (e of c.efforts; track e.value) {
              <button class="pill" role="radio" [attr.aria-checked]="e.selected" [class.on]="e.selected" [disabled]="!can" (click)="d.change({ effort: e.value })">
                {{ e.name }}
              </button>
            }
          </div>
        </section>
      }

      <section>
        <h3 class="label">{{ 'ai.modeTitle' | t }}</h3>
        @for (m of c.modes; track m.value) {
          <button class="choice" role="radio" [attr.aria-checked]="m.selected" [class.on]="m.selected" [disabled]="!can" (click)="mode(m.value)">
            <span class="grow">{{ m.name }}</span>
            @if (m.selected) {
              <shed-icon name="check" />
            }
          </button>
        }
        <button class="choice danger" role="radio" [attr.aria-checked]="c.fullAuto" [class.on]="c.fullAuto" [disabled]="!can" (click)="mode(FULL_AUTO)">
          <span class="col grow">
            <span>{{ 'ai.mode.full' | t }}</span>
            <span class="small">{{ 'ai.fullAuto.short' | t }}</span>
          </span>
          @if (c.fullAuto) {
            <shed-icon name="check" />
          }
        </button>
      </section>

      <section>
        <h3 class="label">{{ 'ai.whileItWorks' | t }}</h3>
        <label class="switch">
          <span class="col grow">
            <span>{{ 'ai.followShort' | t }}</span>
            <span class="dim small">{{ 'ai.follow' | t }}</span>
          </span>
          <input type="checkbox" role="switch" [checked]="c.follow" [disabled]="!can" (change)="d.change({ follow: !c.follow })" />
        </label>
        <label class="switch">
          <span class="col grow">
            <span>{{ 'ai.browserShort' | t }}</span>
            <span class="dim small">{{ 'ai.browser' | t }}</span>
          </span>
          <input type="checkbox" role="switch" [checked]="c.browser" [disabled]="!can" (change)="d.change({ browser: !c.browser })" />
        </label>
      </section>

      <section>
        <h3 class="label">{{ 'ai.folders' | t }}</h3>
        <div class="folder">
          <shed-icon name="folder" />
          <span class="ellipsis grow mono small">{{ tilde(d.ai().folder) }}</span>
          <span class="dim small">{{ 'ai.folderMain' | t }}</span>
        </div>
        @for (f of c.folders; track f) {
          <div class="folder">
            <shed-icon name="folder" />
            <span class="ellipsis grow mono small">{{ tilde(f) }}</span>
            @if (can) {
              <button class="icon-btn" (click)="dropFolder(f)" [attr.aria-label]="'ai.folderRemove' | t: { folder: tilde(f) }">
                <shed-icon name="close" />
              </button>
            }
          </div>
        }
        @if (can) {
          <button class="btn ghost add" (click)="addFolder()"><shed-icon name="plus" />{{ 'ai.folderAdd' | t }}</button>
        }
        <p class="dim small note">{{ 'ai.foldersNote' | t }}</p>
      </section>
    </div>
  `,
  styles: `
    .sheet-body {
      gap: 14px;
      padding-top: 10px;
    }
    section {
      display: flex;
      flex-direction: column;
      gap: 2px;
    }
    .label {
      margin: 0 0 4px;
      font-size: 11px;
      font-weight: 600;
      letter-spacing: 0.04em;
      text-transform: uppercase;
      color: var(--text-dim);
    }
    .col {
      gap: 0;
    }
    .choice,
    .switch,
    .folder {
      display: flex;
      align-items: center;
      gap: 10px;
      min-height: 44px;
      padding: 4px 10px;
      border: 0;
      border-radius: var(--radius-sm);
      background: transparent;
      color: var(--text);
      font: inherit;
      text-align: left;
    }
    .choice,
    .switch {
      cursor: pointer;
    }
    .choice:hover:not(:disabled),
    .switch:hover {
      background: var(--hover);
    }
    .choice.on {
      background: color-mix(in srgb, var(--accent) 12%, transparent);
    }
    .choice.on shed-icon {
      color: var(--accent);
    }
    .choice.danger,
    .choice.danger shed-icon {
      color: var(--danger);
    }
    .choice.danger.on {
      background: color-mix(in srgb, var(--danger) 12%, transparent);
    }
    .choice:disabled {
      color: var(--text-dim);
      cursor: default;
    }
    .pills {
      display: flex;
      flex-wrap: wrap;
      gap: 6px;
    }
    .pill {
      min-height: 36px;
      min-width: 44px;
      padding: 0 14px;
      border: 1px solid var(--divider);
      border-radius: 18px;
      background: var(--raised);
      color: var(--text);
      font: inherit;
      cursor: pointer;
    }
    .pill.on {
      border-color: var(--accent);
      background: color-mix(in srgb, var(--accent) 16%, var(--raised));
      color: var(--accent);
    }
    .switch input {
      appearance: none;
      position: relative;
      width: 40px;
      height: 24px;
      margin: 0;
      flex: none;
      border-radius: 12px;
      background: var(--divider);
      cursor: pointer;
      transition: background 0.15s;
    }
    .switch input::after {
      content: '';
      position: absolute;
      top: 3px;
      left: 3px;
      width: 18px;
      height: 18px;
      border-radius: 50%;
      background: var(--raised);
      box-shadow: 0 1px 2px rgb(0 0 0 / 30%);
      transition: left 0.15s;
    }
    .switch input:checked {
      background: var(--accent);
    }
    .switch input:checked::after {
      left: 19px;
    }
    .switch input:disabled {
      opacity: 0.5;
      cursor: default;
    }
    .add {
      align-self: flex-start;
      min-height: 40px;
    }
    .note {
      margin: 2px 10px 0;
    }
  `,
})
export class AiSettingsSheet {
  protected readonly ref = inject<DialogRef<SettingsSheetData, null>>(DialogRef);
  protected readonly d = this.ref.data;
  protected readonly FULL_AUTO = FULL_AUTO;
  protected readonly choices = computed(() => settingsChoices(this.d.ai(), this.d.info()));

  protected tilde(path: string): string {
    return tildify(path, this.d.home());
  }

  protected async mode(value: string): Promise<void> {
    const pick = pickMode(this.d.ai().mode, value);
    if (pick.kind === 'ask') await this.d.fullAuto();
    else if (pick.kind === 'change') this.d.change({ mode: pick.mode });
  }

  protected async addFolder(): Promise<void> {
    const path = await this.d.pickFolder();
    if (!path) return;
    const next = withFolder(this.choices().folders, path);
    if (next !== this.choices().folders) this.d.change({ addDirs: next });
  }

  protected dropFolder(path: string): void {
    this.d.change({ addDirs: withoutFolder(this.choices().folders, path) });
  }
}
