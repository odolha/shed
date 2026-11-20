import { ChangeDetectionStrategy, Component, inject } from '@angular/core';
import { TPipe } from '../../i18n/i18n';
import { DialogRef } from '../../ui/dialog';
import { Icon } from '../../ui/icon';
import { Logbook } from './logbook';

export interface LogbookDialogData {
  workspace: string;
  tab: string | null;
}

/** The logbook over a terminal: picking a command closes it with that command. */
@Component({
  selector: 'shed-logbook-dialog',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [Logbook, TPipe, Icon],
  template: `
    <div class="dialog-body">
      <div class="row">
        <shed-icon name="logbook" [size]="18" />
        <h2 class="grow">{{ 'logbook.title' | t }}</h2>
        <button class="icon-btn" (click)="ref.close(null)" [attr.aria-label]="'common.close' | t">
          <shed-icon name="close" />
        </button>
      </div>
      <p class="dim small" style="margin: 0">{{ 'logbook.pickHint' | t }}</p>
      <shed-logbook
        class="box"
        [workspace]="ref.data.workspace"
        [tab]="ref.data.tab"
        [pickable]="true"
        (pick)="ref.close($event)"
      />
    </div>
  `,
  styles: `
    .box {
      height: min(60dvh, 560px);
    }
  `,
})
export class LogbookDialog {
  protected readonly ref = inject<DialogRef<LogbookDialogData, string>>(DialogRef);
}
