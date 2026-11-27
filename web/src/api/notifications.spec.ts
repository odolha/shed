import { TestBed } from '@angular/core/testing';
import { Router } from '@angular/router';
import type { NotificationFrame, ServerFrame, WorkspaceInfo } from './gen/api';
import { LiveNotifications, notificationLink, notificationToast } from './notifications';
import { NexusSocket } from './socket';
import { Notices } from '../ui/notices';

const devbox: WorkspaceInfo = { id: 'w1', name: 'devbox', online: true, seen: 0, policy: 'full' };

const waiting = (bench: string | null, tab: string | null): NotificationFrame => ({
  workspace: 'w1',
  notification: { kind: 'aiWaiting', title: 'Claude waits for an answer', body: 'api · claude', bench, tab },
});

const alert: NotificationFrame = {
  workspace: '',
  notification: { kind: 'security', title: 'New sign-in: Pixel', body: 'A phone from 203.0.113.5' },
};

describe('notificationToast', () => {
  it('opens the tab, as far as it is known', () => {
    expect(notificationLink(waiting('b1', 't1'))).toBe('/w/w1/b/b1/t/t1');
    expect(notificationLink(waiting('b1', null))).toBe('/w/w1/b/b1');
    expect(notificationLink(waiting(null, 't1'))).toBe('/w/w1');
    expect(notificationLink({ ...waiting('b 1', 't/1'), workspace: 'w?' })).toBe('/w/w%3F/b/b%201/t/t%2F1');
  });

  it('says the workspace and where in it', () => {
    expect(notificationToast(waiting('b1', 't1'), [devbox])).toEqual({
      title: 'Claude waits for an answer',
      detail: 'devbox · api · claude',
      link: '/w/w1/b/b1/t/t1',
    });
    const offline: NotificationFrame = { workspace: 'w1', notification: { kind: 'workspaceOffline', title: 'devbox went offline', body: '' } };
    expect(notificationToast(offline, [devbox]).detail).toBe('devbox');
    expect(notificationToast(waiting('b1', 't1'), []).detail).toBe('api · claude');
  });

  it('opens the Devices for a security alert', () => {
    expect(notificationToast(alert, [devbox])).toEqual({
      title: 'New sign-in: Pixel',
      detail: 'A phone from 203.0.113.5',
      link: '/devices',
    });
  });
});

describe('LiveNotifications', () => {
  let listener: (frame: ServerFrame) => void;
  let url: string;
  const navigated: string[] = [];

  beforeEach(() => {
    url = '/';
    navigated.length = 0;
    TestBed.configureTestingModule({
      providers: [
        { provide: NexusSocket, useValue: { listen: (fn: (frame: ServerFrame) => void) => (listener = fn) } },
        {
          provide: Router,
          useValue: {
            get url() {
              return url;
            },
            navigateByUrl: (to: string) => navigated.push(to),
          },
        },
      ],
    });
    TestBed.inject(LiveNotifications);
    listener({ type: 'Welcome', content: { api: 1, nexus: 'test', workspaces: [devbox], states: [] } });
  });

  it('shows a live notification as a toast that opens its tab', () => {
    listener({ type: 'Notification', content: waiting('b1', 't1') });
    const [notice] = TestBed.inject(Notices).list();
    expect(notice).toMatchObject({ kind: 'notification', text: 'Claude waits for an answer', detail: 'devbox · api · claude', link: '/w/w1/b/b1/t/t1' });
  });

  it('says nothing about the tab already open', () => {
    url = '/w/w1/b/b1/t/t1';
    listener({ type: 'Notification', content: waiting('b1', 't1') });
    expect(TestBed.inject(Notices).list()).toEqual([]);
  });

  it('always shows a security alert, even on the Devices', () => {
    url = '/devices';
    listener({ type: 'Notification', content: alert });
    expect(TestBed.inject(Notices).list()[0]).toMatchObject({ text: 'New sign-in: Pixel', link: '/devices' });
  });

  it('keeps the workspaces’ names as they change', () => {
    listener({ type: 'Workspaces', content: [{ ...devbox, name: 'renamed' }] });
    listener({ type: 'Notification', content: waiting('b1', 't1') });
    expect(TestBed.inject(Notices).list()[0]?.detail).toBe('renamed · api · claude');
  });
});
