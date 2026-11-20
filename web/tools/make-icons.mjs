// Makes src/ui/icons.ts from Shed's UI icons (assets/icons/ui/*.svg) and
// a few the web app needs besides, drawn the same way (24×24, 1.75 stroke).
// Run with `npm run icons` after adding or changing an icon.

import { readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const source = join(here, '../../assets/icons/ui');
const target = join(here, '../src/ui/icons.ts');

/** Icons of the web app's own, as SVG bodies. */
const extra = {
  'chevron-down': '<path d="M6 9l6 6 6-6"/>',
  'chevron-right': '<path d="M9 6l6 6-6 6"/>',
  'arrow-left': '<path d="M19 12H5"/><path d="M11 6l-6 6 6 6"/>',
  check: '<path d="M5 12.5l4.5 4.5L19 7.5"/>',
  menu: '<path d="M4 6h16"/><path d="M4 12h16"/><path d="M4 18h16"/>',
  image: '<rect x="3.5" y="4.5" width="17" height="15" rx="2"/><circle cx="9" cy="10" r="1.6"/><path d="M20.5 16l-5-5-8.5 8.5"/>',
  camera: '<path d="M4 8h3l1.6-2.5h6.8L17 8h3v11H4z"/><circle cx="12" cy="13.2" r="3.4"/>',
  attach: '<path d="M20 11.5l-7.8 7.8a5 5 0 0 1-7.1-7.1l8.2-8.2a3.4 3.4 0 0 1 4.8 4.8l-8.2 8.2a1.7 1.7 0 0 1-2.4-2.4l7.4-7.4"/>',
  key: '<circle cx="8" cy="15" r="4"/><path d="M11 12l9-9"/><path d="M17 6l2.5 2.5"/><path d="M14.5 8.5l2 2"/>',
  device: '<rect x="7" y="3" width="10" height="18" rx="2"/><path d="M11 18h2"/>',
  bell: '<path d="M6 16V11a6 6 0 0 1 12 0v5l1.5 2h-15z"/><path d="M10 20.5a2.2 2.2 0 0 0 4 0"/>',
  'bell-off': '<path d="M6 16V11a6 6 0 0 1 12 0v5l1.5 2h-15z"/><path d="M10 20.5a2.2 2.2 0 0 0 4 0"/><path d="M3.5 3.5l17 17"/>',
  logout: '<path d="M10 4H5v16h5"/><path d="M15 8l4 4-4 4"/><path d="M19 12H9"/>',
  keyboard: '<rect x="2.5" y="6" width="19" height="12" rx="2"/><path d="M6 10h.01M9.5 10h.01M13 10h.01M16.5 10h.01M7.5 14h9"/>',
  warning: '<path d="M12 4l9 16H3z"/><path d="M12 10v4"/><path d="M12 17h.01"/>',
  resize: '<path d="M4 9V4h5"/><path d="M20 15v5h-5"/><path d="M4 4l6 6"/><path d="M20 20l-6-6"/>',
  question: '<circle cx="12" cy="12" r="9"/><path d="M9.6 9.5a2.5 2.5 0 0 1 4.8.8c0 1.7-2.4 2.2-2.4 3.7"/><path d="M12 17h.01"/>',
  git: '<circle cx="6" cy="6" r="2.2"/><circle cx="6" cy="18" r="2.2"/><circle cx="18" cy="9" r="2.2"/><path d="M6 8.2v7.6"/><path d="M18 11.2c0 3-3 3.6-10.2 5.6"/>',
  thinking: '<path d="M9 18h6"/><path d="M10 21h4"/><path d="M12 3a6 6 0 0 0-3.5 10.9V16h7v-2.1A6 6 0 0 0 12 3z"/>',
  queue: '<path d="M4 6h12"/><path d="M4 12h12"/><path d="M4 18h8"/><path d="M18 15v6"/><path d="M15 18h6"/>',
};

const ATTRS = ['d', 'cx', 'cy', 'r', 'x', 'y', 'width', 'height', 'rx', 'fill'];

function parse(body) {
  const els = [];
  const re = /<(path|circle|rect)\b([^>]*)\/?>/g;
  let m;
  while ((m = re.exec(body))) {
    const a = {};
    for (const name of ATTRS) {
      const v = new RegExp(`\\s${name}="([^"]*)"`).exec(m[2]);
      if (v) a[name] = v[1];
    }
    els.push({ t: m[1], a });
  }
  return els;
}

const icons = {};
for (const file of readdirSync(source).sort()) {
  if (!file.endsWith('.svg')) continue;
  const svg = readFileSync(join(source, file), 'utf8');
  icons[file.slice(0, -4)] = parse(svg.replace(/^<svg[^>]*>/, ''));
}
for (const [name, body] of Object.entries(extra)) icons[name] = parse(body);

const out = `// Made by tools/make-icons.mjs from assets/icons/ui/*.svg: do not edit.

export interface IconEl {
  t: 'path' | 'circle' | 'rect';
  a: Partial<Record<'d' | 'cx' | 'cy' | 'r' | 'x' | 'y' | 'width' | 'height' | 'rx' | 'fill', string>>;
}

export const ICONS: Record<string, IconEl[]> = {
${Object.entries(icons)
  .map(([k, v]) => `  ${JSON.stringify(k)}: ${JSON.stringify(v)},`)
  .join('\n')}
};
`;
writeFileSync(target, out);
console.log(`${Object.keys(icons).length} icons → ${target}`);
