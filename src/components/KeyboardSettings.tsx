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
      <summary>窗口内快捷键</summary>
      <p className="settings-feedback">
        点击组合后按下新按键。输入法、菜单与输入框的编辑行为优先；Esc 始终保留退出路径。
      </p>
      {(Object.keys(keyLabels) as (keyof KeyBindings)[]).map((action) => (
        <div className="setting-row" key={action}>
          <span>{keyLabels[action]}</span>
          <button
            className="secondary-button"
            type="button"
            disabled={!snapshot || busy}
            aria-label={`修改${keyLabels[action]}快捷键`}
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
                setFeedback('请先结束输入法组合。');
                return;
              }
              if (event.code === 'Escape') {
                setRecording(null);
                return;
              }
              if (/^(Control|Shift|Alt|Meta)(Left|Right)$/.test(event.code)) return;
              const mac = primary === '⌘';
              if (mac ? event.ctrlKey : event.metaKey) {
                setFeedback('请使用当前平台的 Ctrl/Cmd 修饰键。');
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
            {recording === action ? '请按键…' : formatKey(draft[action], primary)}
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
            setFeedback('默认组合尚未保存。');
          }}
        >
          恢复默认组合
        </button>
        <button
          className="primary-button"
          type="button"
          data-testid="save-window-keys"
          disabled={!snapshot || busy || !!error || !!recording}
          onClick={() => {
            if (snapshot)
              void save({ bindings: draft, expectedRevision: snapshot.revision })
                .then(() => setFeedback('窗口内快捷键已保存。'))
                .catch((error) => setFeedback(safeError(error).message));
          }}
        >
          保存窗口快捷键
        </button>
      </div>
      <p className="settings-feedback" role="status">
        {error || feedback}
      </p>
    </details>
  );
}
