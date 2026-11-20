/**
 * Strings, by key. English only for now: `en.json` is flat, keys are dotted
 * by feature (`ai.send`). `{name}` in a string is filled from the params.
 * A key missing from the dictionary shows as itself, so it is easy to spot.
 *
 * Plurals: `t('x', { n })` looks up `x.one` when n is 1 and `x.other`
 * otherwise, if those exist, before `x`.
 */

import { Pipe, PipeTransform } from '@angular/core';
import en from './en.json';

export type Params = Record<string, string | number | null | undefined>;

let dict: Record<string, string> = en as Record<string, string>;

/** Replaces the dictionary (tests, and later other languages). */
export function setDictionary(next: Record<string, string>): void {
  dict = next;
}

export function t(key: string, params?: Params): string {
  let text: string | undefined;
  const n = params?.['n'];
  if (typeof n === 'number') {
    text = dict[n === 1 ? `${key}.one` : `${key}.other`];
  }
  text ??= dict[key];
  if (text === undefined) return key;
  if (!params) return text;
  return text.replace(/\{(\w+)\}/g, (whole, name: string) => {
    const v = params[name];
    return v === undefined || v === null ? whole : String(v);
  });
}

/** `{{ 'ai.send' | t }}`, `{{ 'ai.drafts' | t: { n: 3 } }}`. */
@Pipe({ name: 't' })
export class TPipe implements PipeTransform {
  transform(key: string, params?: Params): string {
    return t(key, params);
  }
}
