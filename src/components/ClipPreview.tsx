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
    <section className="detail-panel" aria-label="内容预览">
      <header className="detail-toolbar">
        <button ref={back} className="text-button" onClick={onBack} aria-label="返回历史列表">
          <Icon name="back" size={16} />
          返回
        </button>
        <span className="preview-type">{clip.language ?? typeNames[clip.type]}</span>
        <button
          className="icon-button"
          onClick={onPin}
          disabled={pinDisabled || clip.deleted}
          aria-label={clip.pinned ? '取消置顶' : '置顶内容'}
          aria-pressed={clip.pinned}
        >
          <Icon name="pin" size={17} />
        </button>
        <button
          className="icon-button"
          aria-label="更多内容操作"
          onClick={(event) => onMore(event.currentTarget)}
        >
          <Icon name="more" />
        </button>
      </header>
      <div className="detail-scroll">
        {error && (
          <p role="alert" className="detail-error">
            {error}
          </p>
        )}
        <p className="record-context">
          {clip.source} · {clip.day} {clip.time}
          {collectionName
            ? ` · ${collectionName}`
            : clip.collection
              ? ` · ${collectionNames[clip.collection]}`
              : ''}
        </p>
        {clip.type === 'file' && <h2 className="record-title">{clip.title}</h2>}
        {clip.type === 'image' ? (
          <>
            <img className="preview-image" src={clip.image} alt={clip.content} />
            <p className="record-context">{clip.size}</p>
          </>
        ) : (
          <pre className={`full-content ${clip.language ? 'code-content' : ''}`}>
            {clip.content}
          </pre>
        )}
      </div>
      <footer className="detail-footer">
        <span>{clip.type === 'file' ? '文件引用示例' : typeNames[clip.type]}</span>
        <button className="primary-button" onClick={onCopy}>
          <Icon name={clip.deleted ? 'restore' : 'copy'} size={15} />
          {clip.deleted ? '恢复' : '复制'}
        </button>
      </footer>
    </section>
  );
}
