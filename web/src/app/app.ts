import { ChangeDetectionStrategy, Component, inject } from '@angular/core';
import { Router, RouterOutlet } from '@angular/router';
import { DialogHost } from '../ui/dialog';
import { NoticesHost } from '../ui/notices';

@Component({
  selector: 'shed-root',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [RouterOutlet, DialogHost, NoticesHost],
  template: `
    <router-outlet />
    <shed-dialog-host />
    <shed-notices />
  `,
})
export class App {
  private readonly router = inject(Router);

  constructor() {
    // A notification clicked while the app is open: the service worker asks
    // us to go to its tab (when it cannot navigate the window itself).
    navigator.serviceWorker?.addEventListener('message', (e: MessageEvent) => {
      const data = e.data as { type?: string; url?: string } | null;
      if (data?.type === 'shed-open' && typeof data.url === 'string' && data.url.startsWith('/')) {
        void this.router.navigateByUrl(data.url);
      }
    });
  }
}
