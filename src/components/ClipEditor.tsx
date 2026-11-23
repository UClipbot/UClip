import { useEffect, useRef, useState, type FormEvent, type KeyboardEvent } from 'react';
import type { ClipVersion, EditableContent, EditClip } from '../generated/contracts';
import { t } from '../i18n';
import { safeError } from '../platform/client';
import { Modal } from './Modal';

type Draft = { original: EditableContent; entries: EditableContent[]; cursor: number };

function equal(left: EditableContent, right: EditableContent) {
  if (left.kind === 'text') return right.kind === 'text' && left.value === right.value;
  return (
    right.kind === 'files' &&
    left.value.length === right.value.length &&
    left.value.every((value, index) => value === right.value[index])
  );
}

export function ClipEditor({
  clip,
  load,
  save,
  onClose,
  onSaved,
}: {
  clip: ClipVersion;
  load: (clip: ClipVersion) => Promise<EditableContent>;
  save: (request: EditClip) => Promise<void>;
  onClose: () => void;
  onSaved: () => void;
}) {
  const [draft, setDraft] = useState<Draft | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const [confirmDiscard, setConfirmDiscard] = useState(false);
  const committing = useRef(false);
  const loader = useRef(load);
  loader.current = load;
  const firstField = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    let active = true;
    void loader
      .current(clip)
      .then((original) => {
        if (active) setDraft({ original, entries: [original], cursor: 0 });
      })
      .catch((failure) => {
        if (active) setError(safeError(failure).message);
      });
    return () => {
      active = false;
    };
  }, [clip.id, clip.expectedRevision]);

  const content = draft?.entries[draft.cursor];
  const loaded = !!content;
  useEffect(() => {
    if (loaded) firstField.current?.focus();
  }, [loaded]);
  const dirty = draft && content && !equal(content, draft.original);
  const valid =
    content?.kind === 'text' ? !!content.value : content?.value.every((path) => path.length > 0);

  function change(next: EditableContent) {
    setConfirmDiscard(false);
    setDraft((current) => {
      if (!current || equal(current.entries[current.cursor]!, next)) return current;
      const entries = [
        ...current.entries.slice(Math.max(0, current.cursor - 7), current.cursor + 1),
        next,
      ];
      return { ...current, entries, cursor: entries.length - 1 };
    });
  }
  function step(delta: number) {
    setConfirmDiscard(false);
    setDraft(
      (current) =>
        current && {
          ...current,
          cursor: Math.min(current.entries.length - 1, Math.max(0, current.cursor + delta)),
        },
    );
  }
  function handleUndo(event: KeyboardEvent<HTMLFormElement>) {
    if (
      committing.current ||
      event.nativeEvent.isComposing ||
      event.keyCode === 229 ||
      event.altKey ||
      !(event.ctrlKey || event.metaKey)
    )
      return;
    if (event.code !== 'KeyZ' && event.code !== 'KeyY') return;
    event.preventDefault();
    step(event.code === 'KeyY' || event.shiftKey ? 1 : -1);
  }
  function close() {
    if (committing.current) return;
    if (dirty) setConfirmDiscard(true);
    else onClose();
  }
  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!content || !dirty || !valid || committing.current) return;
    committing.current = true;
    setSaving(true);
    setError('');
    try {
      await save({ clip, content });
      onSaved();
    } catch (failure) {
      setError(safeError(failure).message);
    } finally {
      committing.current = false;
      setSaving(false);
    }
  }
  return (
    <Modal title={t('edit_record')} onClose={close} wide>
      <form className="new-form clip-editor" onSubmit={submit} onKeyDown={handleUndo}>
        {content?.kind === 'text' && (
          <label>
            {t('record_text')}
            <textarea
              ref={firstField}
              rows={8}
              value={content.value}
              readOnly={saving}
              onChange={(event) => change({ kind: 'text', value: event.target.value })}
            />
          </label>
        )}
        {content?.kind === 'files' && (
          <>
            <p>{t('edit_file_references_hint')}</p>
            <div className="file-path-editor">
              {content.value.map((path, index) => (
                <div className="file-path-entry" key={index}>
                  <label>
                    {t('file_path_number', { 0: index + 1 })}
                    <textarea
                      ref={index === 0 ? firstField : undefined}
                      rows={1}
                      value={path}
                      readOnly={saving}
                      onChange={(event) => {
                        const paths = [...content.value];
                        paths[index] = event.target.value;
                        change({ kind: 'files', value: paths });
                      }}
                    />
                  </label>
                  <button
                    type="button"
                    className="text-button"
                    disabled={saving || content.value.length <= 1}
                    onClick={() =>
                      change({
                        kind: 'files',
                        value: content.value.filter((_, at) => at !== index),
                      })
                    }
                  >
                    {t('remove_file_path', { 0: index + 1 })}
                  </button>
                </div>
              ))}
              <button
                type="button"
                className="secondary-button"
                disabled={saving || content.value.length >= 256}
                onClick={() => change({ kind: 'files', value: [...content.value, ''] })}
              >
                {t('add_file_path')}
              </button>
            </div>
          </>
        )}
        {!content && !error && <p role="status">{t('loading_full_content')}</p>}
        {error && (
          <p role="alert" className="settings-feedback">
            {t(error)}
          </p>
        )}
        {confirmDiscard && (
          <div className="editor-discard">
            <p role="alert">{t('discard_unsaved_edit')}</p>
            <button
              type="button"
              className="secondary-button"
              onClick={() => setConfirmDiscard(false)}
            >
              {t('keep_editing')}
            </button>
            <button type="button" className="secondary-button" onClick={onClose}>
              {t('discard_changes')}
            </button>
          </div>
        )}
        <div className="form-actions">
          {draft && (
            <>
              <button
                type="button"
                className="text-button"
                disabled={saving || draft.cursor === 0}
                onClick={() => step(-1)}
              >
                {t('undo')}
              </button>
              <button
                type="button"
                className="text-button"
                disabled={saving || draft.cursor === draft.entries.length - 1}
                onClick={() => step(1)}
              >
                {t('redo_edit')}
              </button>
              <button
                type="button"
                className="text-button"
                disabled={saving || !dirty}
                onClick={() => change(draft.original)}
              >
                {t('reset_edit')}
              </button>
            </>
          )}
          <button type="button" className="secondary-button" disabled={saving} onClick={close}>
            {t('cancel')}
          </button>
          <button type="submit" className="primary-button" disabled={saving || !dirty || !valid}>
            {t(saving ? 'saving' : 'save_edit')}
          </button>
        </div>
      </form>
    </Modal>
  );
}
