import { Injectable, Signal, inject, signal } from '@angular/core';
import type { Picture } from '../../api/gen/api';
import { WorkspacesStore } from '../../api/workspaces.store';

export type PictureState = { url: string } | { missing: true } | null;

/** Pictures of conversations (`AiPicture`), fetched once each and kept. */
@Injectable({ providedIn: 'root' })
export class AiPictures {
  private readonly store = inject(WorkspacesStore);
  private readonly cache = new Map<string, ReturnType<typeof signal<PictureState>>>();

  get(workspace: string, tab: string, name: string): Signal<PictureState> {
    const key = `${workspace}\n${tab}\n${name}`;
    let s = this.cache.get(key);
    if (!s) {
      s = signal<PictureState>(null);
      this.cache.set(key, s);
      const target = s;
      this.store
        .call(workspace, { type: 'AiPicture', content: { tab, name } }, 'Picture')
        .then((p) => target.set({ url: dataUrl(p) }))
        .catch(() => target.set({ missing: true }));
    }
    return s;
  }
}

export function dataUrl(p: Picture): string {
  return `data:${p.mediaType};base64,${p.data}`;
}

/** A file the user picked, pasted or dropped, as a picture to send. */
export async function readPicture(file: File, maxSide = 2000): Promise<Picture> {
  const raw = await readAsDataUrl(file);
  let url = raw;
  // Large photos (a phone's camera) are scaled down before they go.
  if (file.size > 1_500_000 && typeof createImageBitmap === 'function') {
    try {
      const bmp = await createImageBitmap(file);
      const scale = Math.min(1, maxSide / Math.max(bmp.width, bmp.height));
      const canvas = document.createElement('canvas');
      canvas.width = Math.round(bmp.width * scale);
      canvas.height = Math.round(bmp.height * scale);
      canvas.getContext('2d')?.drawImage(bmp, 0, 0, canvas.width, canvas.height);
      url = canvas.toDataURL('image/jpeg', 0.88);
    } catch {
      url = raw;
    }
  }
  const comma = url.indexOf(',');
  const mediaType = /^data:([^;,]+)/.exec(url)?.[1] ?? file.type ?? 'image/png';
  return { mediaType, data: url.slice(comma + 1) };
}

function readAsDataUrl(file: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result));
    r.onerror = () => reject(r.error);
    r.readAsDataURL(file);
  });
}
