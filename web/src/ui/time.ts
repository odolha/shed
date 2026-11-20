/** Times as Shed writes them: `14:32`, `5 min ago`, `2h 14m left`. */

import { Injectable, Pipe, PipeTransform, signal } from '@angular/core';
import { t } from '../i18n/i18n';

/** Now, in Unix seconds, ticking every 30 s so "ago" texts stay right. */
@Injectable({ providedIn: 'root' })
export class Clock {
  readonly now = signal(Math.floor(Date.now() / 1000));
  constructor() {
    setInterval(() => this.now.set(Math.floor(Date.now() / 1000)), 30_000);
  }
}

export function nowSecs(): number {
  return Math.floor(Date.now() / 1000);
}

/** `5 min ago`, `3 h ago`, `2 days ago`; `just now`. */
export function ago(at: number, now = nowSecs()): string {
  if (!at) return '';
  const d = Math.max(0, now - at);
  if (d < 45) return t('time.justNow');
  if (d < 3600) return t('time.minAgo', { n: Math.round(d / 60) });
  if (d < 86400) return t('time.hAgo', { n: Math.round(d / 3600) });
  return t('time.daysAgo', { n: Math.round(d / 86400) });
}

/** `14:32`, or with the date on another day. */
export function clock(at: number, now = nowSecs()): string {
  if (!at) return '';
  const date = new Date(at * 1000);
  const today = new Date(now * 1000);
  const hm = date.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  if (date.toDateString() === today.toDateString()) return hm;
  return `${date.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })} ${hm}`;
}

/** A length of time: `45s`, `12m`, `2h 14m`, `3d 4h`. */
export function span(secs: number): string {
  const s = Math.max(0, Math.round(secs));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return m % 60 ? `${h}h ${m % 60}m` : `${h}h`;
  const d = Math.floor(h / 24);
  return h % 24 ? `${d}d ${h % 24}h` : `${d}d`;
}

@Pipe({ name: 'ago' })
export class AgoPipe implements PipeTransform {
  transform(at: number | null | undefined, now?: number): string {
    return at ? ago(at, now) : '';
  }
}

@Pipe({ name: 'clock' })
export class ClockPipe implements PipeTransform {
  transform(at: number | null | undefined): string {
    return at ? clock(at) : '';
  }
}
