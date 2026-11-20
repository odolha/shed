/**
 * Claude's answers, from Markdown to safe HTML: `marked` parses, DOMPurify
 * cleans (no scripts, no event handlers, no styles, no forms), and links open
 * in a new tab. The only place the app sets HTML, and only this way.
 */

import { Directive, ElementRef, effect, inject, input } from '@angular/core';
import DOMPurify from 'dompurify';
import { Marked } from 'marked';
import { t } from '../../i18n/i18n';

const marked = new Marked({ gfm: true, breaks: false, async: false });

let hooked = false;

function purifier(): typeof DOMPurify {
  if (!hooked) {
    hooked = true;
    DOMPurify.addHook('afterSanitizeAttributes', (node) => {
      if (node.tagName === 'A') {
        const href = node.getAttribute('href') ?? '';
        if (/^(https?:|mailto:)/i.test(href)) {
          node.setAttribute('target', '_blank');
          node.setAttribute('rel', 'noopener noreferrer');
        } else if (!href.startsWith('#')) {
          // Relative links and file paths go nowhere useful in the browser.
          node.removeAttribute('href');
        }
      }
    });
  }
  return DOMPurify;
}

const ALLOWED_TAGS = [
  'a', 'b', 'blockquote', 'br', 'code', 'del', 'em', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'hr', 'i',
  'img', 'input', 'kbd', 'li', 'ol', 'p', 'pre', 's', 'span', 'strong', 'sub', 'sup', 'table',
  'tbody', 'td', 'th', 'thead', 'tr', 'ul', 'details', 'summary',
];
const ALLOWED_ATTR = ['href', 'title', 'alt', 'src', 'class', 'align', 'type', 'checked', 'disabled', 'start', 'colspan', 'rowspan'];

/** Markdown to sanitized HTML. */
export function renderMarkdown(text: string): string {
  const html = marked.parse(text ?? '') as string;
  return sanitize(html);
}

export function sanitize(html: string): string {
  return purifier().sanitize(html, {
    ALLOWED_TAGS,
    ALLOWED_ATTR,
    ALLOWED_URI_REGEXP: /^(?:(?:https?|mailto):|data:image\/(?:png|jpe?g|gif|webp);base64,|#)/i,
    FORBID_ATTR: ['style', 'id', 'name'],
  }) as string;
}

/** `<div class="md" [shedMarkdown]="text"></div>`; code blocks get a Copy button. */
@Directive({
  selector: '[shedMarkdown]',
  host: { class: 'md', '(click)': 'onClick($event)' },
})
export class MarkdownDirective {
  readonly shedMarkdown = input<string>('');
  private readonly el = inject<ElementRef<HTMLElement>>(ElementRef);
  private last: string | null = null;

  constructor() {
    effect(() => {
      const text = this.shedMarkdown();
      if (text === this.last) return;
      this.last = text;
      const host = this.el.nativeElement;
      host.innerHTML = renderMarkdown(text);
      for (const pre of Array.from(host.querySelectorAll('pre'))) {
        const btn = document.createElement('button');
        btn.type = 'button';
        btn.className = 'btn small ghost code-copy';
        btn.textContent = t('common.copy');
        btn.dataset['copy'] = '1';
        pre.prepend(btn);
      }
    });
  }

  protected onClick(e: MouseEvent): void {
    const btn = (e.target as HTMLElement).closest<HTMLElement>('[data-copy]');
    if (!btn) return;
    const code = btn.parentElement?.querySelector('code')?.textContent ?? '';
    void navigator.clipboard?.writeText(code).then(() => {
      btn.textContent = t('common.copied');
      setTimeout(() => (btn.textContent = t('common.copy')), 1500);
    });
  }
}
