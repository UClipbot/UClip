import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { Clip } from '../demo/clips';
import type { HoverPreviewRequest } from '../generated/contracts';

export function showHoverPreview(clip: Clip, anchor: HTMLElement, token: string) {
  const box = anchor.getBoundingClientRect();
  const request: HoverPreviewRequest = {
    token,
    id: clip.id,
    kind: clip.type === 'image' ? 'image' : clip.type === 'file' ? 'files' : 'text',
    source: clip.source,
    savedAt: clip.savedAt ?? '0',
    size: clip.size ?? null,
    anchor: { x: box.x, y: box.y, width: box.width, height: box.height },
  };
  return invoke<void>('show_hover_preview', { request });
}
export const hideHoverPreview = (token: string, unlessHovered = false) =>
  invoke<void>('hide_hover_preview', { token, unlessHovered });
export const readHoverPreview = () => invoke<HoverPreviewRequest | null>('get_hover_preview');
export const hoverPreviewPresence = (token: string, hovered: boolean) =>
  invoke<void>('hover_preview_presence', { token, hovered });
export const onHoverPreviewChanged = (refresh: () => void) =>
  listen('hover-preview-changed', refresh);
export const onHoverPreviewClosed = (close: (token: string) => void) =>
  listen<string>('hover-preview-closed', (event) => close(event.payload));
