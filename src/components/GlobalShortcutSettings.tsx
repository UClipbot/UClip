import { useCallback, useEffect, useRef, useState } from 'react';
import { t } from '../i18n';
import { safeError } from '../platform/client';
import {
  formatGlobalShortcut,
  onGlobalShortcutCapture,
  recordGlobalShortcut,
  setGlobalShortcutRecording,
} from '../platform/globalShortcut';
import type { useDesktop } from '../platform/useDesktop';
import type { useQuickPastePreferences } from '../platform/useQuickPastePreferences';

type Field = 'main' | 'quick';
let nextRecordingToken = 0;

export function GlobalShortcutSettings({
  desktop,
  quickPaste,
  os,
  saveMainShortcut,
}: {
  desktop: Pick<ReturnType<typeof useDesktop>, 'status' | 'busy' | 'error'>;
  quickPaste: ReturnType<typeof useQuickPastePreferences>;
  os: string;
  saveMainShortcut: (shortcut: string) => Promise<string | undefined>;
}) {
  const mainBinding = desktop.status?.preferences.shortcut ?? '';
  const quickStatus = quickPaste.snapshot;
  const quickBinding = quickStatus?.preferences.shortcut ?? null;
  const [mainDraft, setMainDraft] = useState<string>();
  const [quickDraft, setQuickDraft] = useState<string | null>();
  const mainShortcut = mainDraft ?? mainBinding;
  const quickShortcut = quickDraft === undefined ? quickBinding : quickDraft;
  const [mainError, setMainError] = useState('');
  const [quickError, setQuickError] = useState('');
  const [captureError, setCaptureError] = useState<{ field: Field; message: string }>();
  const [ready, setReady] = useState(false);
  const [listenerError, setListenerError] = useState('');
  const [recording, setRecording] = useState<Field>();
  const active = useRef<{ field: Field; token: number } | undefined>(undefined);
  useEffect(() => setMainDraft(undefined), [mainBinding]);
  useEffect(() => setQuickDraft(undefined), [quickBinding]);

  const stopRecording = useCallback(() => {
    const current = active.current;
    if (!current) return;
    active.current = undefined;
    setRecording(undefined);
    setCaptureError(undefined);
    void setGlobalShortcutRecording(current.token, false).catch(() => {});
  }, []);

  const acceptBinding = useCallback(
    (field: Field, binding: string) => {
      if (field === 'main') setMainDraft(binding);
      else setQuickDraft(binding);
      stopRecording();
    },
    [stopRecording],
  );

  useEffect(() => {
    let mounted = true;
    let unlisten: (() => void) | undefined;
    void onGlobalShortcutCapture((token, binding) => {
      const current = active.current;
      if (!mounted || current?.token !== token) return;
      if (binding === null) stopRecording();
      else acceptBinding(current.field, binding);
    })
      .then((dispose) => {
        if (mounted) {
          unlisten = dispose;
          setReady(true);
        } else dispose();
      })
      .catch((error) => {
        if (mounted) setListenerError(safeError(error).message);
      });
    return () => {
      mounted = false;
      unlisten?.();
      stopRecording();
    };
  }, [acceptBinding, stopRecording]);

  function startRecording(field: Field) {
    if (active.current?.field === field) {
      stopRecording();
      return;
    }
    const token = ++nextRecordingToken;
    active.current = { field, token };
    setRecording(field);
    setCaptureError(undefined);
    if (field === 'main') setMainError('');
    else setQuickError('');
    void setGlobalShortcutRecording(token, true).catch((error) => {
      if (active.current?.token !== token) return;
      stopRecording();
      setCaptureError({ field, message: safeError(error).message });
    });
  }

  function recorder(
    field: Field,
    id: string,
    label: string,
    binding: string | null,
    disabled: boolean,
  ) {
    const armed = recording === field;
    return (
      <div className="global-shortcut-recorder">
        <label id={id + '-label'} htmlFor={id}>
          {t(label)}
        </label>
        <button
          id={id}
          className="secondary-button"
          type="button"
          disabled={disabled || !ready}
          aria-labelledby={id + '-label ' + id + '-value'}
          aria-describedby={id + '-feedback'}
          aria-pressed={armed}
          onClick={(event) => {
            // Some WebViews leave mouse-clicked buttons unfocused.
            event.currentTarget.focus();
            startRecording(field);
          }}
          onBlur={(event) => {
            if (active.current?.field === field && (event.relatedTarget || document.hasFocus()))
              stopRecording();
          }}
          onKeyDownCapture={(event) => {
            if (active.current?.field !== field) return;
            if (event.code === 'Tab' && !event.ctrlKey && !event.altKey && !event.metaKey) {
              stopRecording();
              return;
            }
            event.stopPropagation();
            const input = event.nativeEvent;
            if (!input.isComposing && input.keyCode !== 229) event.preventDefault();
            if (input.repeat) return;
            if (
              input.code === 'Escape' &&
              !input.isComposing &&
              input.keyCode !== 229 &&
              !input.ctrlKey &&
              !input.altKey &&
              !input.metaKey &&
              !input.shiftKey
            ) {
              stopRecording();
              return;
            }
            const result = recordGlobalShortcut(input, os);
            if (result?.error) setCaptureError({ field, message: result.error });
            if (result?.binding) acceptBinding(field, result.binding);
          }}
        >
          <span id={id + '-value'}>
            {armed
              ? t('press_keys')
              : binding
                ? formatGlobalShortcut(binding, os)
                : t('shortcut_unassigned')}
          </span>
        </button>
      </div>
    );
  }

  const busy = desktop.busy || quickPaste.busy;
  const mainDisabled = !desktop.status || busy;
  const quickDisabled = !quickStatus || busy;
  const mainChanged =
    formatGlobalShortcut(mainShortcut, os) !== formatGlobalShortcut(mainBinding, os) ||
    (mainDraft !== undefined && !desktop.status?.shortcutAvailable);
  const quickChanged =
    formatGlobalShortcut(quickShortcut ?? '', os) !==
      formatGlobalShortcut(quickBinding ?? '', os) ||
    (typeof quickDraft === 'string' && !quickStatus?.available);

  function feedback(field: Field, status: string) {
    if (captureError?.field === field) return captureError.message;
    return recording === field ? 'shortcut_recording_cancel_hint' : status;
  }

  async function saveQuickShortcut() {
    setQuickError('');
    setCaptureError(undefined);
    try {
      await quickPaste.save({ shortcut: quickShortcut });
    } catch (error) {
      setQuickError(safeError(error).message);
    }
  }

  return (
    <section className="settings-section" aria-labelledby="global-shortcuts-heading">
      <h3 id="global-shortcuts-heading">{t('global_shortcuts')}</h3>
      {listenerError && <p role="alert">{t(listenerError)}</p>}
      <form
        onSubmit={(event) => {
          event.preventDefault();
          if (!mainDisabled && !recording && mainChanged) {
            setMainError('');
            setCaptureError(undefined);
            void saveMainShortcut(mainShortcut).then((error) => setMainError(error ?? ''));
          }
        }}
      >
        <div className="global-shortcut-field">
          {recorder(
            'main',
            'main-window-shortcut',
            'show_window_shortcut',
            mainShortcut,
            mainDisabled,
          )}
          <button
            className="secondary-button"
            type="submit"
            disabled={mainDisabled || !!recording || !mainChanged}
          >
            {t('save_shortcut')}
          </button>
        </div>
        <p id="main-window-shortcut-feedback" className="settings-feedback" role="status">
          {t(
            feedback(
              'main',
              mainError ||
                desktop.error ||
                (mainChanged && 'shortcut_changes_not_saved') ||
                (desktop.status &&
                  (desktop.status.shortcutAvailable
                    ? 'show_window_shortcut_available_esc_hides_the_window'
                    : 'the_show_window_shortcut_is_unavailable')) ||
                '',
            ),
          )}
        </p>
      </form>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          if (!quickDisabled && !recording && quickChanged) void saveQuickShortcut();
        }}
      >
        <div className="global-shortcut-field">
          {recorder(
            'quick',
            'quick-paste-shortcut',
            'quick_paste_shortcut',
            quickShortcut,
            quickDisabled,
          )}
          <button
            className="secondary-button"
            type="button"
            disabled={quickDisabled || !!recording || quickShortcut === null}
            onClick={() => {
              setQuickDraft(null);
              setQuickError('');
              setCaptureError(undefined);
            }}
          >
            {t('clear_shortcut')}
          </button>
          <button
            className="secondary-button"
            type="submit"
            disabled={quickDisabled || !!recording || !quickChanged}
          >
            {t('quick_paste_save')}
          </button>
        </div>
        <p id="quick-paste-shortcut-feedback" className="settings-feedback" role="status">
          {t(
            feedback(
              'quick',
              quickError ||
                (quickChanged && 'shortcut_changes_not_saved') ||
                (quickStatus?.error
                  ? safeError(quickStatus.error).message
                  : quickStatus?.available
                    ? 'quick_paste_shortcut_enabled'
                    : quickStatus
                      ? 'quick_paste_shortcut_disabled'
                      : ''),
            ),
          )}
        </p>
      </form>
    </section>
  );
}
