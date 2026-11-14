import { t } from '../i18n';
import { useEffect, useRef } from 'react';
import { Icon } from './Icon';
import { collectionNames, typeNames, type Clip } from '../demo/clips';

export function ClipPreview({
  clip,
  onBack,
  onCopy,
  onPin,
  onMore,
  error,
  pinDisabled = false,
  collectionName,
}: {
  clip: Clip;
  error?: string;
  pinDisabled?: boolean;
  collectionName?: string;
  onBack: () => void;
  onCopy: () => void;
  onPin: () => void;
  onMore: (anchor: HTMLElement) => void;
}) {
  const back = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    back.current?.focus();
  }, []);
  return (
    <section className="detail-panel" aria-label={t('content_preview')}>
      <header className="detail-toolbar">
        <button
          ref={back}
          className="text-button"
          onClick={onBack}
          aria-label={t('back_to_history')}
        >
          <Icon name="back" size={16} />
          {t('back')}
        </button>
        <span className="preview-type">{clip.language ?? t(typeNames[clip.type])}</span>
        <button
          className="icon-button"
          onClick={onPin}
          disabled={pinDisabled || clip.deleted}
          aria-label={clip.pinned ? t('unpin') : t('pin_content')}
          aria-pressed={clip.pinned}
        >
          <Icon name="pin" size={17} />
        </button>
        <button
          className="icon-button"
          aria-label={t('more_content_actions')}
          onClick={(event) => onMore(event.currentTarget)}
        >
          <Icon name="more" />
        </button>
      </header>
      <div className="detail-scroll">
        {error && (
          <p role="alert" className="detail-error">
            {t(error)}
          </p>
        )}
        <p className="record-context">
          {clip.source} · {t(clip.day)} {clip.time}
          {collectionName
            ? ` · ${collectionName}`
            : clip.collection
              ? ` · ${t(collectionNames[clip.collection])}`
              : ''}
        </p>
        {clip.type === 'file' && <h2 className="record-title">{clip.title}</h2>}
        {clip.type === 'image' ? (
          <>
            <img className="preview-image" src={clip.image} alt={clip.content || t('image')} />
            <p className="record-context">{clip.size}</p>
          </>
        ) : (
          <pre className={`full-content ${clip.language ? 'code-content' : ''}`}>
            {clip.content}
          </pre>
        )}
      </div>
      <footer className="detail-footer">
        <span>
          {clip.type === 'file'
            ? t(clip.native ? 'file_references' : 'file_reference_sample')
            : t(typeNames[clip.type])}
        </span>
        <button className="primary-button" onClick={onCopy}>
          <Icon name={clip.deleted ? 'restore' : 'copy'} size={15} />
          {clip.deleted ? t('restore') : t('copy')}
        </button>
      </footer>
    </section>
  );
}
