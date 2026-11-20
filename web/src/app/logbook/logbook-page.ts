import { ChangeDetectionStrategy, Component, computed, inject, input } from '@angular/core';
import { RouterLink } from '@angular/router';
import { WorkspacesStore } from '../../api/workspaces.store';
import { TPipe } from '../../i18n/i18n';
import { Icon } from '../../ui/icon';
import { Logbook } from './logbook';

/** `/w/:workspace/logbook?tab=…`: the logbook on a page of its own. */
@Component({
  selector: 'shed-logbook-page',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [Logbook, RouterLink, TPipe, Icon],
  template: `
    <div class="page col">
      <div class="row">
        <a class="icon-btn" [routerLink]="back()" [attr.aria-label]="'common.back' | t">
          <shed-icon name="arrow-left" [size]="18" />
        </a>
        <shed-icon name="logbook" [size]="18" />
        <h1 class="grow ellipsis">{{ 'logbook.title' | t }}</h1>
        <span class="dim ellipsis">{{ where() }}</span>
      </div>
      <shed-logbook class="grow" [workspace]="workspace()" [tab]="tab() ?? null" />
    </div>
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      min-height: 0;
    }
    .page {
      width: 100%;
      flex: 1 1 auto;
      min-height: 0;
      display: flex;
    }
    shed-logbook {
      min-height: 0;
    }
  `,
})
export class LogbookPage {
  /** Route parameter. */
  readonly workspace = input.required<string>();
  /** Query parameter: the tab the tab and bench scopes are about. */
  readonly tab = input<string | null | undefined>(null);

  private readonly store = inject(WorkspacesStore);

  private readonly found = computed(() => {
    const tab = this.tab();
    return tab ? this.store.tab(this.workspace(), tab) : undefined;
  });

  protected readonly back = computed(() => {
    const f = this.found();
    const ws = encodeURIComponent(this.workspace());
    return f ? `/w/${ws}/b/${encodeURIComponent(f.bench.id)}/t/${encodeURIComponent(f.tab.id)}` : `/w/${ws}`;
  });

  protected readonly where = computed(() => {
    const f = this.found();
    const name = this.store.info(this.workspace())?.name ?? '';
    return f ? `${name} › ${f.bench.name} › ${f.tab.title}` : name;
  });
}
