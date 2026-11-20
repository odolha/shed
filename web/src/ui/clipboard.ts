import { Injectable, inject } from '@angular/core';
import { t } from '../i18n/i18n';
import { Notices } from './notices';

/** Copies text, saying so. */
@Injectable({ providedIn: 'root' })
export class Clipboard {
  private readonly notices = inject(Notices);

  async copy(text: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(text);
      this.notices.info(t('common.copied'));
    } catch (e) {
      this.notices.error(e);
    }
  }
}
