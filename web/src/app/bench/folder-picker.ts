import { ChangeDetectionStrategy, Component, computed, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import type { FolderEntry } from '../../api/gen/api';
import { errorText } from '../../api/errors';
import { WorkspacesStore } from '../../api/workspaces.store';
import { TPipe } from '../../i18n/i18n';
import { DialogRef } from '../../ui/dialog';
import { Icon } from '../../ui/icon';
import { tildify } from '../shared/tabs';

export interface FolderPickerData {
  workspace: string;
  title: string;
  /** Where to start: an absolute path. */
  start: string;
  /** Folders to offer at once (the bench's). */
  suggestions: string[];
}

/** Choose a folder on the workspace, through `Browse` (which says where it is, absolute, and its parent). */
@Component({
  selector: 'shed-folder-picker',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [FormsModule, TPipe, Icon],
  template: `
    <div class="dialog-body">
      <h2>{{ d.title }}</h2>
      @if (d.suggestions.length) {
        <div class="col" style="gap: 2px">
          <span class="dim small">{{ 'folder.bench' | t }}</span>
          @for (s of d.suggestions; track s) {
            <button class="menu-item" (click)="ref.close(s)">
              <shed-icon name="folder" />
              <span class="ellipsis">{{ tilde(s) }}</span>
            </button>
          }
        </div>
      }
      <form class="row" (submit)="$event.preventDefault(); go(typed)">
        <input class="input mono" name="path" [(ngModel)]="typed" [attr.aria-label]="'folder.path' | t" />
        <button class="btn" type="submit">{{ 'folder.go' | t }}</button>
      </form>
      <div class="list card">
        <button class="menu-item" (click)="up()" [disabled]="!parent()">
          <shed-icon name="arrow-up" />..
        </button>
        @if (loading()) {
          <div class="empty">{{ 'common.loading' | t }}</div>
        } @else if (error(); as e) {
          <div class="error-box">{{ e }}</div>
        } @else {
          @for (f of folders(); track f.name) {
            <button class="menu-item" [class.dim]="f.ignored" (click)="go(join(f.name))">
              <shed-icon name="folder" />
              <span class="ellipsis">{{ f.name }}</span>
            </button>
          } @empty {
            <div class="empty small">{{ 'folder.none' | t }}</div>
          }
        }
      </div>
      <div class="dialog-actions">
        <button class="btn ghost" (click)="ref.close(null)">{{ 'common.cancel' | t }}</button>
        <button class="btn primary" [disabled]="!path()" (click)="ref.close(path())">
          {{ 'folder.choose' | t: { name: tilde(path()) } }}
        </button>
      </div>
    </div>
  `,
  styles: `
    .list {
      max-height: 40vh;
      overflow: auto;
      padding: 4px;
    }
  `,
})
export class FolderPicker {
  protected readonly ref = inject<DialogRef<FolderPickerData, string>>(DialogRef);
  private readonly store = inject(WorkspacesStore);
  protected readonly d = this.ref.data;
  /** Where we are, absolute, as the workspace said. */
  protected readonly path = signal(this.d.start || '');
  protected readonly parent = signal<string | null>(null);
  protected typed = this.d.start;
  protected readonly entries = signal<FolderEntry[]>([]);
  protected readonly loading = signal(false);
  protected readonly error = signal<string | null>(null);
  protected readonly folders = computed(() => this.entries().filter((e) => e.dir));
  private readonly home = computed(() => this.store.state(this.d.workspace)?.home ?? '');

  constructor() {
    void this.load(this.path());
  }

  protected tilde(p: string): string {
    return tildify(p, this.home());
  }

  protected join(name: string): string {
    const p = this.path();
    return p.endsWith('/') ? p + name : `${p}/${name}`;
  }

  protected up(): void {
    const parent = this.parent();
    if (parent) this.go(parent);
  }

  /** A path as typed: absolute, `~/…`, or empty for home (the workspace makes it absolute). */
  protected go(p: string): void {
    void this.load(p.trim());
  }

  private async load(path: string): Promise<void> {
    this.loading.set(true);
    this.error.set(null);
    try {
      const b = await this.store.call(this.d.workspace, { type: 'Browse', content: { path } }, 'Browsed');
      this.entries.set([...b.entries].sort((a, c) => a.name.localeCompare(c.name)));
      this.path.set(b.path);
      this.parent.set(b.parent ?? null);
      this.typed = b.path;
    } catch (e) {
      this.error.set(errorText(e));
    } finally {
      this.loading.set(false);
    }
  }
}
