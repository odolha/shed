import { ChangeDetectionStrategy, Component, computed, input } from '@angular/core';
import { ICONS } from './icons';

/** One of Shed's icons, drawn inline in the current text colour. */
@Component({
  selector: 'shed-icon',
  changeDetection: ChangeDetectionStrategy.OnPush,
  host: { 'aria-hidden': 'true', class: 'shed-icon' },
  template: `
    <svg
      viewBox="0 0 24 24"
      [attr.width]="size()"
      [attr.height]="size()"
      fill="none"
      stroke="currentColor"
      stroke-width="1.75"
      stroke-linecap="round"
      stroke-linejoin="round"
      focusable="false"
    >
      @for (el of els(); track $index) {
        @switch (el.t) {
          @case ('path') {
            <svg:path [attr.d]="el.a.d" [attr.fill]="el.a.fill ?? null" />
          }
          @case ('circle') {
            <svg:circle [attr.cx]="el.a.cx" [attr.cy]="el.a.cy" [attr.r]="el.a.r" [attr.fill]="el.a.fill ?? null" />
          }
          @case ('rect') {
            <svg:rect
              [attr.x]="el.a.x"
              [attr.y]="el.a.y"
              [attr.width]="el.a.width"
              [attr.height]="el.a.height"
              [attr.rx]="el.a.rx ?? null"
              [attr.fill]="el.a.fill ?? null"
            />
          }
        }
      }
    </svg>
  `,
  styles: `
    :host {
      display: inline-flex;
      flex: none;
      line-height: 0;
    }
  `,
})
export class Icon {
  readonly name = input.required<string>();
  readonly size = input<number>(16);
  protected readonly els = computed(() => ICONS[this.name()] ?? []);
}
