import { t } from '../i18n';
import { useEffect, useState } from 'react';
import type { KeyBindings, KeyboardSnapshot, UpdateKeyBindings } from '../generated/contracts';
import { DEFAULT_KEYS, bindingError, formatKey, keyLabels } from '../platform/keyboard';
import { safeError } from '../platform/client';

export function KeyboardSettings({
  snapshot,
  busy,
  primary,
  save,
  expanded = false,
}: {
  expanded?: boolean;
  snapshot?: KeyboardSnapshot;
  busy: boolean;
  primary: string;
  save: (request: UpdateKeyBindings) => Promise<unknown>;
}) {
  const [draft, setDraft] = useState<KeyBindings>(DEFAULT_KEYS);
  const [recording, setRecording] = useState<keyof KeyBindings | null>(null);
  const [feedback, setFeedback] = useState('');
  useEffect(() => {
    if (snapshot) setDraft(snapshot.bindings);
  }, [snapshot?.revision]);
  const error = bindingError(draft);
  return (
    <details className="about-section" open={expanded || undefined}>
      <summary>{t('window_shortcuts')}</summary>
      <p className="settings-feedback">
        {t(
          'click_a_shortcut_and_press_new_keys_input_methods_menus_and_text_editing_take_priority_esc_always_provides_a_way_out',
        )}
      </p>
      {(Object.keys(keyLabels) as (keyof KeyBindings)[]).map((action) => (
        <div className="setting-row" key={action}>
          <span>{t(keyLabels[action])}</span>
          <button
            className="secondary-button"
            type="button"
            disabled={!snapshot || busy}
            aria-label={t('change_shortcut_for_0', { 0: t(keyLabels[action]) })}
            aria-pressed={recording === action}
            onClick={() => {
              setRecording(action);
              setFeedback('');
            }}
            onKeyDownCapture={(event) => {
              if (recording !== action) return;
              event.preventDefault();
              event.stopPropagation();
              if (event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) {
                setFeedback('finish_composing_text_first');
                return;
              }
              if (event.code === 'Escape') {
                setRecording(null);
                return;
              }
              if (/^(Control|Shift|Alt|Meta)(Left|Right)$/.test(event.code)) return;
              const mac = primary === '⌘';
              if (mac ? event.ctrlKey : event.metaKey) {
                setFeedback('use_the_ctrl_cmd_modifier_for_this_platform');
                return;
              }
              setDraft({
                ...draft,
                [action]: {
                  code: event.code === 'NumpadEnter' ? 'Enter' : event.code,
                  primary: mac ? event.metaKey : event.ctrlKey,
                  alt: event.altKey,
                  shift: event.shiftKey,
                },
              });
              setRecording(null);
            }}
          >
            {recording === action ? t('press_keys') : formatKey(draft[action], primary)}
          </button>
        </div>
      ))}
      <div className="form-actions">
        <button
          className="secondary-button"
          type="button"
          disabled={busy}
          onClick={() => {
            setDraft(DEFAULT_KEYS);
            setRecording(null);
            setFeedback('default_shortcuts_have_not_been_saved_yet');
          }}
        >
          {t('restore_default_shortcuts')}
        </button>
        <button
          className="primary-button"
          type="button"
          data-testid="save-window-keys"
          disabled={!snapshot || busy || !!error || !!recording}
          onClick={() => {
            if (snapshot)
              void save({ bindings: draft, expectedRevision: snapshot.revision })
                .then(() => setFeedback('window_shortcuts_saved'))
                .catch((error) => setFeedback(safeError(error).message));
          }}
        >
          {t('save_window_shortcuts')}
        </button>
      </div>
      <p className="settings-feedback" role="status">
        {t(error || feedback)}
      </p>
    </details>
  );
}
