import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from 'react';
import { createPortal } from 'react-dom';
import type { Clip } from '../demo/clips';
import { typeNames } from '../demo/clips';
import { t, formatTimestamp } from '../i18n';
import { useClipDetail } from '../platform/useClipDetail';
import { hideHoverPreview, onHoverPreviewClosed, showHoverPreview } from '../platform/hoverPreview';
import { safeError } from '../platform/client';

type Target = { id: string; revision?: string; anchor: HTMLElement };
type HoveredRow = { id: string; anchor: HTMLElement; enteredAt: number };

const OPEN_DELAY_MS = 1500;
const SWITCH_DELAY_MS = 100;

function sameRow(first: HoveredRow | null, second: HoveredRow) {
  return first?.id === second.id && first.anchor === second.anchor;
}

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
    let closeTimer: ReturnType<typeof setTimeout> | undefined;
    let candidate: HoveredRow | null = null;
    let displayed: { row: HoveredRow; token?: string } | null = null;
    let opening: { row: HoveredRow; token: string } | null = null;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const scheduleOpen = () => {
      clearTimeout(openTimer);
      const row = candidate;
      if (!row || opening || (displayed && sameRow(row, displayed.row))) return;
      const delay = (native ? displayed : panel.current) ? SWITCH_DELAY_MS : OPEN_DELAY_MS;
      openTimer = setTimeout(
        () => {
          const { anchor, id } = row;
          const clip = currentClips.current.find((item) => item.id === id);
          if (
            !clip ||
            !list.contains(anchor) ||
            anchor.closest('[data-clip-id]')?.getAttribute('data-clip-id') !== id
          )
            return;
          if (!native) {
            displayed = { row };
            setTarget({ id, revision: clip.revision, anchor });
            return;
          }
          const token = crypto.randomUUID();
          const previousToken = displayed?.token;
          opening = { row, token };
          void showHoverPreview(clip, anchor, token, previousToken)
            .then((visible) => {
              if (disposed || opening?.token !== token) return;
              opening = null;
              displayed = visible ? { row, token } : null;
              // The native window may have closed just before a quick switch reached it.
              if (!visible && !previousToken && sameRow(candidate, row)) candidate = null;
              scheduleOpen();
            })
            .catch((error) => {
              if (disposed || opening?.token !== token) return;
              opening = null;
              if (sameRow(candidate, row)) candidate = null;
              report.current?.(safeError(error).message);
              scheduleOpen();
            });
        },
        Math.max(0, delay - (performance.now() - row.enteredAt)),
      );
    };
    const hideNative = (unlessHovered = false) => {
      if (displayed?.token) void hideHoverPreview(displayed.token, unlessHovered).catch(() => {});
      if (opening) void hideHoverPreview(opening.token, unlessHovered).catch(() => {});
    };
    const dismiss = (force = true) => {
      clearTimeout(openTimer);
      clearTimeout(closeTimer);
      candidate = null;
      if (!force && native) {
        // The preview can keep this request alive while it has the pointer or focus.
        hideNative(true);
        return;
      }
      hideNative();
      displayed = null;
      opening = null;
      setTarget(null);
    };
    if (native)
      void onHoverPreviewClosed((token) => {
        const closed = opening?.token === token ? opening : displayed;
        if (disposed || closed?.token !== token) return;
        if (closed === opening) opening = null;
        displayed = null;
        if (sameRow(candidate, closed.row)) candidate = null;
        scheduleOpen();
      })
        .then((stop) => {
          if (disposed) stop();
          else unlisten = stop;
        })
        .catch(() => {});
    keep.current = () => clearTimeout(closeTimer);
    leave.current = () => {
      clearTimeout(openTimer);
      candidate = null;
      clearTimeout(closeTimer);
      closeTimer = setTimeout(() => dismiss(false), native ? 450 : 180);
    };
    const enter = (event: PointerEvent) => {
      if (event.pointerType === 'touch') return;
      const anchor = (event.target as Element).closest<HTMLElement>('.row-main');
      if (!anchor || !list.contains(anchor)) return;
      if (event.relatedTarget instanceof Node && anchor.contains(event.relatedTarget)) return;
      const id = anchor.closest<HTMLElement>('[data-clip-id]')?.dataset.clipId;
      if (!id) return;
      clearTimeout(closeTimer);
      if (candidate?.anchor === anchor && candidate.id === id) return;
      candidate = { id, anchor, enteredAt: performance.now() };
      scheduleOpen();
    };
    const exit = (event: PointerEvent) => {
      if (event.pointerType === 'touch' || !candidate) return;
      const next = event.relatedTarget as Node | null;
      if (next && candidate.anchor.contains(next)) return;
      leave.current();
      if (next && panel.current?.contains(next)) keep.current();
    };
    const scroll = (event: Event) => {
      if (!panel.current?.contains(event.target as Node)) dismiss();
    };
    const key = (event: KeyboardEvent) => {
      if (event.isComposing || event.keyCode === 229 || event.defaultPrevented) return;
      if (event.key === 'Escape' && (candidate || displayed || opening)) {
        if (displayed) event.stopPropagation();
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
      hideNative();
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
