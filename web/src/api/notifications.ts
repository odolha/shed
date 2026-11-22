/**
 * Notifications the nexus sends live (`ServerFrame` `Notification`) while
 * some Shed app is in view, shown as a toast that opens the tab. When no app
 * is in view the nexus pushes instead, and the service worker shows it
 * (`public/sw.js`).
 */

import { Injectable, inject } from '@angular/core';
import { Router } from '@angular/router';
import type { NotificationFrame, WorkspaceInfo } from './gen/api';
import { NexusSocket } from './socket';
import { Notices } from '../ui/notices';

/** Where a notification leads: its tab, as far as it is known. */
export function notificationLink(frame: NotificationFrame): string {
  const seg = (s: string) => encodeURIComponent(s);
  const { bench, tab } = frame.notification;
  let link = `/w/${seg(frame.workspace)}`;
  if (bench) {
    link += `/b/${seg(bench)}`;
    if (tab) link += `/t/${seg(tab)}`;
  }
  return link;
}

/** What the toast says: the title, then the workspace and where in it. */
export function notificationToast(frame: NotificationFrame, workspaces: WorkspaceInfo[]): { title: string; detail: string; link: string } {
  const name = workspaces.find((w) => w.id === frame.workspace)?.name ?? '';
  const body = frame.notification.body ?? '';
  const detail = [name, body].filter(Boolean).join(' · ');
  return { title: frame.notification.title, detail, link: notificationLink(frame) };
}

/** Shows live notifications as toasts; started by the app. */
@Injectable({ providedIn: 'root' })
export class LiveNotifications {
  private readonly socket = inject(NexusSocket);
  private readonly notices = inject(Notices);
  private readonly router = inject(Router);
  private workspaces: WorkspaceInfo[] = [];

  constructor() {
    this.socket.listen((frame) => {
      if (frame.type === 'Welcome') this.workspaces = frame.content.workspaces;
      else if (frame.type === 'Workspaces') this.workspaces = frame.content;
      else if (frame.type === 'Notification') this.show(frame.content);
    });
  }

  show(frame: NotificationFrame): void {
    const toast = notificationToast(frame, this.workspaces);
    // Already looking at it: nothing to say.
    if (this.router.url === toast.link) return;
    this.notices.notification(toast.title, toast.detail, toast.link);
  }
}
