import { invoke } from '@tauri-apps/api/core';
import { useEffect, useRef, useState } from 'react';
import type { HoverPreviewRequest } from '../generated/contracts';
import { formatTimestamp, t } from '../i18n';
import { useAppearance } from '../platform/useAppearance';
import { useClipDetail } from '../platform/useClipDetail';
import { safeError } from '../platform/client';
import { useWindowFrame, windowFrame } from '../platform/windowFrame';
import {
  hideHoverPreview,
  hoverPreviewPresence,
  onHoverPreviewChanged,
  readHoverPreview,
} from '../platform/hoverPreview';
import { WindowTitlebar } from './WindowTitlebar';
import { WindowResizeHandles } from './WindowResizeHandles';

export function HoverPreviewWindow() {
  const appearance = useAppearance();
  const [request, setRequest] = useState<HoverPreviewRequest | null>(null);
  const [error, setError] = useState('');
  const [retry, setRetry] = useState(0);
  const pointerInside = useRef(false);
  const refreshRequest = useRef<() => void>(() => {});
  const current = useRef(request);
  current.current = request;
  useWindowFrame(true);
  const { detail, detailError } = useClipDetail(true, request?.id, `${request?.token}:${retry}`);
  useEffect(() => {
    let disposed = false;
    let generation = 0;
    let unlisten: (() => void) | undefined;
    const refresh = async () => {
      const own = ++generation;
      try {
        const next = await readHoverPreview();
        if (!disposed && own === generation) {
          if (!next) pointerInside.current = false;
          setRequest(next);
          setError('');
        }
      } catch (failure) {
        if (!disposed && own === generation) setError(safeError(failure).message);
      }
    };
    refreshRequest.current = () => void refresh();
    // Subscribe before reading so cold window creation cannot miss a new request.
    void onHoverPreviewChanged(() => void refresh())
      .then((stop) => {
        if (disposed) stop();
        else {
          unlisten = stop;
          void refresh();
        }
      })
      .catch(() => {
        if (!disposed) void refresh();
      });
    const key = (event: KeyboardEvent) => {
      if (event.isComposing || event.keyCode === 229 || event.key !== 'Escape') return;
      const token = current.current?.token;
      if (token) {
        event.preventDefault();
        void hideHoverPreview(token).catch((failure) => setError(safeError(failure).message));
      }
    };
    window.addEventListener('keydown', key);
    return () => {
      disposed = true;
      generation++;
      unlisten?.();
      window.removeEventListener('keydown', key);
    };
  }, []);
  useEffect(() => {
    if (request && pointerInside.current)
      void hoverPreviewPresence(request.token, true).catch(() => {});
  }, [request?.token]);
  const loading = !!request && !detail && !detailError;
  const failure = error || appearance.error || detailError;
  const title = request
    ? t(request.kind === 'files' ? 'file_references' : request.kind)
    : t('content_preview');
  const windowAction = (command: string) =>
    void invoke(command).catch((failure) => setError(safeError(failure).message));
  return (
    <section
      className="native-hover-preview"
      aria-label={t('content_preview')}
      onPointerEnter={() => {
        pointerInside.current = true;
        if (request) void hoverPreviewPresence(request.token, true).catch(() => {});
      }}
      onPointerLeave={() => {
        pointerInside.current = false;
        if (request) void hoverPreviewPresence(request.token, false).catch(() => {});
      }}
    >
      {appearance.snapshot?.platform.os === 'windows' && (
        <WindowResizeHandles os="windows" onError={setError} />
      )}
      {windowFrame ? (
        <WindowTitlebar
          title={title}
          onDrag={() => windowAction('start_window_drag')}
          onToggleMaximized={() => windowAction('toggle_window_maximized')}
          onClose={() => windowAction('close_window')}
        />
      ) : (
        <header className="native-preview-header">{title}</header>
      )}
      <div className="native-preview-scroll" aria-busy={loading}>
        {failure ? (
          <div className="detail-error" role="alert">
            <p>{t(failure)}</p>
            <button
              className="secondary-button"
              onClick={() => {
                setRetry((value) => value + 1);
                refreshRequest.current();
                void appearance.refresh();
              }}
            >
              {t('retry_preview')}
            </button>
          </div>
        ) : loading ? (
          <p className="record-context" role="status">
            {t('loading_full_content')}
          </p>
        ) : request?.kind === 'image' ? (
          <img
            className="native-preview-image"
            src={detail?.imageDataUrl ?? undefined}
            alt={t('image')}
          />
        ) : (
          <pre className="full-content">{detail?.text}</pre>
        )}
      </div>
      {request && (
        <footer className="native-preview-footer">
          <div className="native-preview-source">
            {request.source}
            {request.size ? ` · ${request.size}` : ''}
          </div>
          <div>
            {t('modified_time')} ·{' '}
            <time dateTime={new Date(Number(request.savedAt)).toISOString()}>
              {formatTimestamp(Number(request.savedAt))}
            </time>
          </div>
          <small>{t('latest_capture_or_save')}</small>
          <p>{t('preview_window_hint')}</p>
          {appearance.snapshot?.platform.sessionHint === 'wayland' && (
            <p>{t('preview_position_limited')}</p>
          )}
        </footer>
      )}
    </section>
  );
}
