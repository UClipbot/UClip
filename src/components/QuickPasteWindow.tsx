import { useEffect, useRef, useState } from 'react';
import type { QuickPasteSnapshot } from '../generated/contracts';
import { t } from '../i18n';
import { safeError } from '../platform/client';
import {
  cancelQuickPaste,
  expandQuickPaste,
  onQuickPasteChanged,
  readQuickPaste,
  selectQuickPaste,
} from '../platform/quickPaste';
import { useAppearance } from '../platform/useAppearance';
import { globalShortcutKeyCode } from '../platform/globalShortcut';
import { WindowResizeGrip } from './WindowResizeGrip';
import { WindowResizeHandles } from './WindowResizeHandles';
import { Icon } from './Icon';

function modifiers(event: KeyboardEvent) {
  return (
    Number(event.ctrlKey) |
    (Number(event.shiftKey) << 1) |
    (Number(event.altKey) << 2) |
    (Number(event.metaKey) << 3)
  );
}

export function QuickPasteWindow() {
  const appearance = useAppearance();
  const [view, setView] = useState<QuickPasteSnapshot | null>(null);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const acting = useRef(false);
  const current = useRef(view);
  current.current = view;
  const os = appearance.snapshot?.platform.os;

  useEffect(() => {
    document.body.classList.add('quick-paste-window');
    let disposed = false;
    let generation = 0;
    let unlisten: (() => void) | undefined;
    const refresh = async () => {
      const request = ++generation;
      try {
        const next = await readQuickPaste();
        if (disposed || request !== generation) return;
        setError('');
        setView(next);
      } catch (failure) {
        if (!disposed && request === generation) setError(safeError(failure).message);
      }
    };
    void onQuickPasteChanged(() => void refresh()).then(
      (stop) => {
        if (disposed) stop();
        else {
          unlisten = stop;
          void refresh();
        }
      },
      () => void refresh(),
    );
    window.addEventListener('focus', refresh);
    return () => {
      disposed = true;
      window.removeEventListener('focus', refresh);
      unlisten?.();
      document.body.classList.remove('quick-paste-window');
    };
  }, []);

  async function act(session: QuickPasteSnapshot, action: () => Promise<unknown>) {
    if (acting.current) return;
    acting.current = true;
    setBusy(true);
    setError('');
    try {
      await action();
    } catch (failure) {
      if (current.current?.token === session.token) setError(safeError(failure).message);
    } finally {
      acting.current = false;
      setBusy(false);
    }
  }

  useEffect(() => {
    const handle = (event: KeyboardEvent) => {
      const session = current.current;
      if (
        !session ||
        session.feedbackOnly ||
        !os ||
        !document.hasFocus() ||
        event.defaultPrevented ||
        event.isComposing ||
        event.keyCode === 229
      )
        return;
      const mask = modifiers(event);
      const keyCode = globalShortcutKeyCode(event, os);
      // A globally registered chord is handled by Rust. Some WebViews also deliver it locally.
      if (
        (keyCode === session.mainCode && mask === session.mainModifiers) ||
        (keyCode === session.quickCode && mask === session.quickModifiers)
      ) {
        event.preventDefault();
        return;
      }
      if (event.key === 'Escape') {
        event.preventDefault();
        if (!event.repeat) void act(session, () => cancelQuickPaste(session.token));
        return;
      }
      if (mask !== 0 && mask !== session.quickModifiers) return;
      if (keyCode === session.finalKey) {
        event.preventDefault();
        if (!event.repeat) void act(session, () => expandQuickPaste(session.token));
        return;
      }
      const digit = /^(?:Digit|Numpad)([1-5])$/.exec(event.code);
      if (!digit) return;
      event.preventDefault();
      const slot = Number(digit[1]) - 1;
      if (!event.repeat && !session.loading && !session.result && session.items[slot]) {
        void act(session, () => selectQuickPaste(session.token, slot));
      }
    };
    window.addEventListener('keydown', handle);
    return () => window.removeEventListener('keydown', handle);
  }, [os]);

  const warning = view?.result?.warning;
  const failure = error || appearance.error || (view?.error && safeError(view.error).message);
  const result = view?.result;
  const finalKey = view?.finalKey?.replace(/^Key/, '') ?? view?.mainShortcut ?? '';
  const feedbackOnly = view?.feedbackOnly ?? false;
  return (
    <main className="quick-paste-panel" aria-label={t('quick_paste')}>
      {!feedbackOnly && os && os !== 'macos' && <WindowResizeGrip os={os} onError={setError} />}
      {!feedbackOnly && os === 'windows' && <WindowResizeHandles os={os} onError={setError} />}
      <header className="quick-paste-header">
        <strong>{t('quick_paste')}</strong>
        {!feedbackOnly && view?.copyOnly && <span>{t('quick_paste_copy_only')}</span>}
      </header>
      {view && !feedbackOnly && (
        <ol className="quick-paste-items" aria-busy={view.loading || busy}>
          {view.items.map((item, index) => (
            <li key={index}>
              <button
                type="button"
                disabled={busy || !!result}
                onClick={() => void act(view, () => selectQuickPaste(view.token, index))}
              >
                <kbd>{index + 1}</kbd>
                {item.thumbnail ? (
                  <img src={item.thumbnail} alt="" />
                ) : (
                  <Icon name={item.kind === 'files' ? 'file' : item.kind} size={17} />
                )}
                <span>
                  {item.kind === 'files'
                    ? t('0_file_references', { 0: item.fileCount ?? 0 })
                    : item.kind === 'image'
                      ? t('image')
                      : item.text}
                </span>
              </button>
            </li>
          ))}
        </ol>
      )}
      <div role="status" className="quick-paste-status">
        {result &&
          t(
            result.pasteRequested
              ? 'copied_and_requested_paste_to_original_app'
              : 'quick_paste_copied_manually',
          )}
        {warning && <div>{t(safeError(warning).message)}</div>}
        {failure && <div>{t(failure)}</div>}
        {!failure && !result && (!view || view.loading) && t('searching')}
        {view &&
          !view.loading &&
          !view.items.length &&
          !failure &&
          !result &&
          t('quick_paste_empty')}
      </div>
      {view && feedbackOnly && (
        <button
          className="secondary-button"
          type="button"
          onClick={() => void act(view, () => cancelQuickPaste(view.token))}
        >
          {t('dismiss_notification')}
        </button>
      )}
      {view && !feedbackOnly && (
        <footer className="quick-paste-footer">
          <span>{t('quick_paste_open_main_0', { 0: finalKey })}</span>
          <span>{t('quick_paste_esc_close')}</span>
        </footer>
      )}
    </main>
  );
}
