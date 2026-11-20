import { ChangeDetectionStrategy, Component, computed, inject, input } from '@angular/core';
import type { LimitWindow, Limits } from '../api/gen/api';
import { t } from '../i18n/i18n';
import { Clock, ago, clock, span } from './time';
import { Tooltip } from './tooltip';

interface Bar {
  key: string;
  label: string;
  used: number;
  pct: number;
  level: 'ok' | 'warn' | 'danger';
  /** Where a steady pace through the window would be now, 0 to 1. */
  pace: number | null;
  left: string;
  /** At this pace the limit comes before the reset. */
  tooFast: boolean;
  tip: string;
}

const FIVE_HOURS = 5 * 3600;
const WEEK = 7 * 86400;

export function limitBar(key: string, label: string, w: LimitWindow, length: number, now: number, seen: number): Bar {
  const used = Math.max(0, Math.min(1, w.used));
  const remaining = Math.max(0, w.resets - now);
  const elapsed = w.resets ? Math.max(0, Math.min(1, 1 - remaining / length)) : null;
  const tooFast = elapsed !== null && elapsed > 0.02 && used / elapsed > 1 && used < 1;
  const tipLines = [
    t('limits.tip', { label, pct: Math.round(used * 100) }),
    w.resets ? t('limits.resets', { at: clock(w.resets, now) }) : '',
  ];
  if (tooFast && elapsed) {
    const hitIn = ((1 - used) / (used / elapsed)) * length;
    tipLines.push(t('limits.hitAt', { at: clock(now + Math.round(hitIn), now) }));
  } else if (elapsed && elapsed > 0.02) {
    tipLines.push(t('limits.atPace', { pct: Math.round(Math.min(1, used / elapsed) * 100) }));
  }
  if (seen && now - seen > 1800) tipLines.push(t('limits.seen', { ago: ago(seen, now) }));
  return {
    key,
    label,
    used,
    pct: Math.round(used * 100),
    level: used >= 0.9 ? 'danger' : used >= 0.7 ? 'warn' : 'ok',
    pace: elapsed,
    left: w.resets ? t('limits.left', { span: span(remaining) }) : '',
    tooFast,
    tip: tipLines.filter(Boolean).join('\n'),
  };
}

/** Claude's usage limits: the 5-hour window and the week, as small bars. */
@Component({
  selector: 'shed-limits',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [Tooltip],
  template: `
    @for (b of bars(); track b.key) {
      <div class="limit" [shedTip]="b.tip" tabindex="0" [attr.aria-label]="b.tip">
        <div class="top">
          <span>{{ b.label }}</span><span class="pct">{{ b.pct }}%</span>
        </div>
        <div class="bar">
          <div class="fill" [class]="b.level" [style.width.%]="b.pct"></div>
          @if (b.pace !== null) {
            <div class="pace" [style.left.%]="b.pace * 100"></div>
          }
        </div>
        <div class="left" [class.fast]="b.tooFast">{{ b.left }}</div>
      </div>
    }
  `,
  styles: `
    :host {
      display: flex;
      gap: 12px;
      align-items: center;
    }
    .limit {
      width: 84px;
      font-size: 11px;
      line-height: 1.2;
      color: var(--text-dim);
      border-radius: 4px;
    }
    .top {
      display: flex;
      justify-content: space-between;
    }
    .pct {
      color: var(--text);
    }
    .bar {
      position: relative;
      height: 4px;
      margin: 3px 0;
      border-radius: 2px;
      background: var(--divider);
    }
    .fill {
      height: 100%;
      border-radius: 2px;
    }
    .fill.ok {
      background: var(--ok);
    }
    .fill.warn {
      background: var(--warn);
    }
    .fill.danger {
      background: var(--danger);
    }
    .pace {
      position: absolute;
      top: -2px;
      width: 2px;
      height: 8px;
      margin-left: -1px;
      background: var(--text);
      opacity: 0.55;
    }
    .left.fast {
      color: var(--warn);
    }
  `,
})
export class LimitsBars {
  readonly limits = input<Limits | null | undefined>(null);
  private readonly clockSvc = inject(Clock);

  protected readonly bars = computed<Bar[]>(() => {
    const l = this.limits();
    const now = this.clockSvc.now();
    if (!l) return [];
    const out: Bar[] = [];
    if (l.fiveHour) out.push(limitBar('5h', t('limits.fiveHour'), l.fiveHour, FIVE_HOURS, now, l.seen ?? 0));
    if (l.week) out.push(limitBar('week', t('limits.week'), l.week, WEEK, now, l.seen ?? 0));
    return out;
  });
}
