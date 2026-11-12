import { Icon } from './Icon';
import { collectionNames, typeNames, type Clip, type Collection } from '../demo/clips';

export function ClipPreview({
  clip,
  onPin,
  onDelete,
  onCollection,
  onCopy,
  onBack,
  menuOpen,
  setMenuOpen,
}: {
  clip: Clip;
  onPin: () => void;
  onDelete: () => void;
  onCollection: (collection: Collection) => void;
  onCopy: () => void;
  onBack: () => void;
  menuOpen: boolean;
  setMenuOpen: (value: boolean) => void;
}) {
  const length = Array.from(clip.content).length;
  const domain = clip.type === 'link' ? new URL(clip.content).hostname : '';
  return (
    <section className="detail-panel" aria-label="内容预览">
      <div className="detail-toolbar">
        <button className="icon-button mobile-back" aria-label="返回历史列表" onClick={onBack}>
          <Icon name="back" />
        </button>
        <span>
          <Icon name={clip.language ? 'file' : clip.type} size={15} />
          {clip.language ?? typeNames[clip.type]}
        </span>
        <div className="detail-tools">
          <button
            className={`icon-button ${clip.pinned ? 'is-pinned' : ''}`}
            title={clip.pinned ? '取消置顶' : '置顶内容'}
            aria-label={clip.pinned ? '取消置顶' : '置顶内容'}
            aria-pressed={clip.pinned}
            onClick={onPin}
          >
            <Icon name="pin" size={17} />
          </button>
          <div className="menu-anchor">
            <button
              className="icon-button"
              aria-label="更多内容操作"
              aria-expanded={menuOpen}
              onClick={() => setMenuOpen(!menuOpen)}
            >
              <Icon name="more" />
            </button>
            {menuOpen && (
              <div className="action-menu" aria-label="内容操作">
                <p>移入分类</p>
                {(Object.keys(collectionNames) as Collection[]).map((key) => (
                  <button
                    key={key}
                    onClick={() => {
                      onCollection(key);
                      setMenuOpen(false);
                    }}
                  >
                    <span className={`collection-dot ${key}`} />
                    {collectionNames[key]}
                    {clip.collection === key && <Icon name="check" size={15} />}
                  </button>
                ))}
                <div className="menu-separator" />
                <button
                  className={clip.deleted ? '' : 'destructive'}
                  onClick={() => {
                    onDelete();
                    setMenuOpen(false);
                  }}
                >
                  <Icon name={clip.deleted ? 'restore' : 'trash'} size={15} />
                  {clip.deleted ? '恢复内容' : '移入回收站'}
                </button>
              </div>
            )}
          </div>
        </div>
      </div>
      <div className={`detail-scroll ${clip.type === 'image' ? 'image-detail' : ''}`} key={clip.id}>
        <div className="record-context">
          <span className={`source-mark ${clip.type}`}>
            {clip.language ? <Icon name="file" size={13} /> : clip.source.slice(0, 1)}
          </span>
          <span>{clip.source}</span>
          <span className="separator-dot">·</span>
          <span>
            {clip.day} {clip.time}
          </span>
          {clip.pinned && (
            <span className="pinned-label">
              <Icon name="pin" size={11} />
              已置顶
            </span>
          )}
        </div>
        <h2 className="record-title">{clip.title}</h2>
        {clip.type === 'text' &&
          (clip.language ? (
            <div className="code-surface">
              <div>
                <span className="code-dot" />
                {clip.language}
                <span>示例代码</span>
              </div>
              <pre>
                <code>{clip.content}</code>
              </pre>
            </div>
          ) : (
            <div className="prose">
              {clip.content.split('\n\n').map((paragraph, index) => (
                <p className={/^0[1-3]/.test(paragraph) ? 'numbered-paragraph' : ''} key={index}>
                  {paragraph}
                </p>
              ))}
            </div>
          ))}
        {clip.type === 'image' && (
          <>
            <div className="image-surface">
              <img
                src={clip.image}
                alt="鼠尾草色山野插画，画有远山、日光与山间小径"
                width="1200"
                height="780"
              />
            </div>
            <p className="preview-caption">{clip.content}</p>
          </>
        )}
        {clip.type === 'link' && (
          <>
            <div className={`link-surface ${domain.includes('linear') ? 'linear-surface' : ''}`}>
              <span className="link-monogram">
                {domain.includes('linear') ? 'L' : domain.includes('github') ? 'G' : 'T'}
              </span>
              <p>
                {domain.includes('linear')
                  ? 'Ideas into momentum.'
                  : domain.includes('github')
                    ? 'Good things are built together.'
                    : 'Build something small. Make it matter.'}
              </p>
              <span className="link-domain">
                {domain}
                <Icon name="link" size={15} />
              </span>
            </div>
            <div className="url-block">
              <Icon name="link" size={15} />
              <span>{clip.content}</span>
            </div>
            <p className="preview-caption">链接卡片为本地示例，不会请求网站预览或图标。</p>
          </>
        )}
        {clip.type === 'file' && (
          <>
            <div className="file-surface">
              <div className="large-file">
                <Icon name="file" size={45} />
                <span>{clip.title.split('.').pop()?.toUpperCase()}</span>
              </div>
              <strong>{clip.title}</strong>
              <span>{clip.size} · 文件引用示例</span>
            </div>
            <div className="url-block">
              <Icon name="folder" size={15} />
              <span>{clip.content}</span>
            </div>
            <p className="preview-caption">仅展示示例路径，不会读取或传输此文件。</p>
          </>
        )}
        <div className="record-metadata">
          <div>
            <span>内容类型</span>
            <span>{clip.language ? '代码片段' : typeNames[clip.type]}</span>
          </div>
          <div>
            <span>
              {clip.type === 'image' ? '图片尺寸' : clip.type === 'file' ? '文件大小' : '内容长度'}
            </span>
            <span>{clip.size ?? `${length} 个字符`}</span>
          </div>
          <div>
            <span>所属分类</span>
            <span className="metadata-collection">
              {clip.collection && <span className={`collection-dot ${clip.collection}`} />}
              {clip.collection ? collectionNames[clip.collection] : '未分类'}
            </span>
          </div>
        </div>
      </div>
      <div className="detail-footer">
        <span>
          <span className="preview-dot" />
          示例内容
        </span>
        <button className="primary-button" onClick={onCopy}>
          <Icon name="copy" size={15} />
          {clip.type === 'file'
            ? '复制示例路径'
            : clip.type === 'image'
              ? '复制图片说明'
              : '复制示例'}
          <kbd>↵</kbd>
        </button>
      </div>
    </section>
  );
}
