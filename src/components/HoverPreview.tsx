import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from 'react';
import { createPortal } from 'react-dom';
import type { Clip } from '../demo/clips';
import { typeNames } from '../demo/clips';
import { t, formatTimestamp } from '../i18n';
import { useClipDetail } from '../platform/useClipDetail';
import { hideHoverPreview, onHoverPreviewClosed, showHoverPreview } from '../platform/hoverPreview';
import { safeError } from '../platform/client';

type Target = { id: string; revision?: string; anchor: HTMLElement };

export function HoverPreview({
  listRef,
  clips,
  native,
  onError,
}: {
  onError?: (message: string) => void;
  listRef: RefObject<HTMLDivElement | null>;
  clips: Clip[];
  native: boolean;
}) {
  const [target, setTarget] = useState<Target | null>(null);
  const report = useRef(onError);
  report.current = onError;
  const currentClips = useRef(clips);
  currentClips.current = clips;
  const panel = useRef<HTMLDivElement>(null);
  const keep = useRef<() => void>(() => {});
  const leave = useRef<() => void>(() => {});
  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    let openTimer: ReturnType<typeof setTimeout> | undefined;
    let closeTimer: ReturnType<typeof setTimeout>;
    let shown = false;
    let nativeToken: string | undefined;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    if (native)
      void onHoverPreviewClosed((token) => {
        if (token === nativeToken) {
          nativeToken = undefined;
          candidate = null;
          shown = false;
        }
      })
        .then((stop) => {
          if (disposed) stop();
          else unlisten = stop;
        })
        .catch(() => {});
    let candidate: HTMLElement | null = null;
    const dismiss = (force = true) => {
      clearTimeout(openTimer);
      clearTimeout(closeTimer);
      openTimer = undefined;
      shown = false;
      candidate = null;
      setTarget(null);
      if (nativeToken) void hideHoverPreview(nativeToken, !force).catch(() => {});
    };
    keep.current = () => clearTimeout(closeTimer);
    leave.current = () => {
      clearTimeout(openTimer);
      openTimer = undefined;
      clearTimeout(closeTimer);
      closeTimer = setTimeout(() => dismiss(false), native ? 450 : 180);
    };
    const enter = (event: PointerEvent) => {
      if (event.pointerType === 'touch') return;
      const anchor = (event.target as Element).closest<HTMLElement>('.row-main');
      if (!anchor || !list.contains(anchor)) return;
      clearTimeout(closeTimer);
      if (candidate === anchor && (shown || openTimer !== undefined)) return;
      clearTimeout(openTimer);
      candidate = anchor;
      shown = false;
      setTarget(null);
      openTimer = setTimeout(() => {
        openTimer = undefined;
        shown = true;
        const id = anchor.closest<HTMLElement>('[data-clip-id]')?.dataset.clipId;
        const clip = currentClips.current.find((item) => item.id === id);
        if (clip && anchor.isConnected) {
          if (native) {
            const token = crypto.randomUUID();
            nativeToken = token;
            void showHoverPreview(clip, anchor, token).catch((error) => {
              if (!disposed && token === nativeToken) report.current?.(safeError(error).message);
            });
          } else setTarget({ id: clip.id, revision: clip.revision, anchor });
        }
      }, 400);
    };
    const exit = (event: PointerEvent) => {
      const next = event.relatedTarget as Node | null;
      if (next && (candidate?.contains(next) || panel.current?.contains(next))) return;
      leave.current();
    };
    const scroll = (event: Event) => {
      if (!panel.current?.contains(event.target as Node)) dismiss();
    };
    const key = (event: KeyboardEvent) => {
      if (event.isComposing || event.keyCode === 229 || event.defaultPrevented) return;
      if (event.key === 'Escape' && (candidate || nativeToken)) {
        if (shown) event.stopPropagation();
        dismiss();
      }
    };
    const pointerDown = (event: PointerEvent) => {
      if (!panel.current?.contains(event.target as Node)) dismiss();
    };
    list.addEventListener('pointerover', enter);
    list.addEventListener('pointerout', exit);
    window.addEventListener('scroll', scroll, true);
    const blur = () => (native ? leave.current() : dismiss());
    const resize = () => dismiss();
    window.addEventListener('resize', resize);
    window.addEventListener('blur', blur);
    window.addEventListener('keydown', key, true);
    window.addEventListener('pointerdown', pointerDown, true);
    return () => {
      disposed = true;
      unlisten?.();
      if (nativeToken) void hideHoverPreview(nativeToken).catch(() => {});
      clearTimeout(openTimer);
      clearTimeout(closeTimer);
      list.removeEventListener('pointerover', enter);
      list.removeEventListener('pointerout', exit);
      window.removeEventListener('scroll', scroll, true);
      window.removeEventListener('resize', resize);
      window.removeEventListener('blur', blur);
      window.removeEventListener('keydown', key, true);
      window.removeEventListener('pointerdown', pointerDown, true);
    };
  }, [listRef, native]);
  const clip =
    target && clips.find((item) => item.id === target.id && item.revision === target.revision);
  return clip && target?.anchor.isConnected ? (
    <FloatingContent
      key={`${clip.id}:${clip.revision}`}
      clip={clip}
      native={native}
      anchor={target.anchor}
      panelRef={panel}
      onEnter={() => keep.current()}
      onLeave={() => leave.current()}
    />
  ) : null;
}

function FloatingContent({
  clip,
  native,
  anchor,
  panelRef,
  onEnter,
  onLeave,
}: {
  clip: Clip;
  native: boolean;
  anchor: HTMLElement;
  panelRef: RefObject<HTMLDivElement | null>;
  onEnter: () => void;
  onLeave: () => void;
}) {
  const { detail, detailError } = useClipDetail(native, clip.id, clip.revision);
  useLayoutEffect(() => {
    const panel = panelRef.current!;
    const position = () => {
      const box = anchor.getBoundingClientRect();
      const width = panel.offsetWidth;
      const height = panel.offsetHeight;
      const below = innerHeight - box.bottom - 16;
      const above = box.top - 16;
      const top = below >= height || below >= above ? box.bottom + 6 : box.top - height - 6;
      panel.style.left = `${Math.max(8, Math.min(box.left + 24, innerWidth - width - 8))}px`;
      panel.style.top = `${Math.max(8, Math.min(top, innerHeight - height - 8))}px`;
    };
    position();
    const observer = new ResizeObserver(position);
    observer.observe(panel);
    const previous = anchor.getAttribute('aria-describedby');
    anchor.setAttribute(
      'aria-describedby',
      [previous, 'clip-hover-preview'].filter(Boolean).join(' '),
    );
    return () => {
      observer.disconnect();
      if (previous === null) anchor.removeAttribute('aria-describedby');
      else anchor.setAttribute('aria-describedby', previous);
    };
  }, [anchor, panelRef]);
  const loading = native && !detail && !detailError;
  const content = native ? detail?.text : clip.content;
  const image = native ? detail?.imageDataUrl : clip.image;
  return createPortal(
    <div
      ref={panelRef}
      id="clip-hover-preview"
      className="hover-preview"
      role="tooltip"
      aria-label={t('content_preview')}
      onPointerEnter={onEnter}
      onPointerLeave={onLeave}
    >
      <header className="hover-preview-header">
        <span>{clip.language ?? t(typeNames[clip.type])}</span>
        <span className="hover-preview-source">{clip.source}</span>
      </header>
      <div className="hover-preview-content" aria-busy={loading}>
        {loading ? (
          <p className="record-context" role="status">
            {t('loading_full_content')}
          </p>
        ) : detailError ? (
          <p className="detail-error" role="alert">
            {t(detailError)}
          </p>
        ) : clip.type === 'image' ? (
          <>
            {image && <img src={image} alt={t('image')} />}
            <p className="record-context">{clip.size}</p>
          </>
        ) : (
          <pre className={`full-content ${clip.language ? 'code-content' : ''}`}>{content}</pre>
        )}
      </div>
      <footer className="hover-preview-footer">
        <span>{t('modified_time')}</span>
        {clip.savedAt ? (
          <time dateTime={new Date(Number(clip.savedAt)).toISOString()}>
            {formatTimestamp(Number(clip.savedAt))}
          </time>
        ) : (
          <span>
            {t(clip.day)} {clip.time}
          </span>
        )}
        <small>{t('latest_capture_or_save')}</small>
      </footer>
    </div>,
    document.body,
  );
}
