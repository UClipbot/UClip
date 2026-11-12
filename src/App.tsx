import { useEffect, useRef, useState, type FormEvent } from 'react';
import { BrandMark, Icon, type IconName } from './components/Icon';
import { Modal } from './components/Modal';
import { ClipPreview } from './components/ClipPreview';
import {
  initialClips,
  collectionNames,
  typeNames,
  type Clip,
  type ClipType,
  type Collection,
  type View,
} from './demo/clips';
import { useClipQuery } from './demo/useClipQuery';
import { useAppearance } from './platform/useAppearance';
import type { Theme } from './generated/contracts';

type Toast = { message: string; undo?: () => void; info?: boolean };
const filters: (ClipType | 'all')[] = ['all', 'text', 'link', 'image', 'file'];
const viewTitles: Record<View, string> = {
  all: '全部记录',
  pinned: '已置顶',
  trash: '回收站',
  ...collectionNames,
};
const stateNames = {
  available: '可用',
  unverified: '待验证',
  restricted: '受限',
  permission_required: '需要权限',
  not_implemented: '待接入',
};
const capabilityNames: Record<string, string> = {
  clipboard_capture: '剪贴板采集',
  clipboard_write: '系统剪贴板写入',
  global_shortcut: '全局快捷键',
  paste_to_app: '原应用回粘',
  tray: '系统托盘',
  sync: '云同步',
};

export function App() {
  const appearance = useAppearance();
  const [clips, setClips] = useState<Clip[]>(() => initialClips.map((clip) => ({ ...clip })));
  const [view, setView] = useState<View>('all');
  const [type, setType] = useState<ClipType | 'all'>('all');
  const [query, setQuery] = useState('');
  const [regex, setRegex] = useState(false);
  const [oldest, setOldest] = useState(false);
  const [grid, setGrid] = useState(false);
  const [selectedId, setSelectedId] = useState('note-1');
  const [mobileDetail, setMobileDetail] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [modal, setModal] = useState<'settings' | 'new' | 'shortcuts' | null>(null);
  const [toast, setToast] = useState<Toast | null>(null);
  const [newTitle, setNewTitle] = useState('');
  const [newContent, setNewContent] = useState('');
  const [settingsTab, setSettingsTab] = useState<'appearance' | 'about'>('appearance');
  const search = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const result = useClipQuery(clips, view, type, query, regex, oldest);
  const selected = result.items.find((clip) => clip.id === selectedId) ?? result.items[0];
  const isMac =
    appearance.snapshot?.platform.os === 'macos' ||
    (!appearance.native && /Mac|iPhone|iPad/.test(navigator.platform));
  const modifier = isMac ? '⌘' : 'Ctrl';
  const activeClips = clips.filter((clip) => !clip.deleted);

  function navigate(next: View) {
    setView(next);
    setType('all');
    setQuery('');
    setMobileDetail(false);
    setMenuOpen(false);
  }
  function changeClip(id: string, update: Partial<Clip>) {
    setClips((current) => current.map((clip) => (clip.id === id ? { ...clip, ...update } : clip)));
  }
  function togglePin(clip: Clip) {
    changeClip(clip.id, { pinned: !clip.pinned });
    setToast({ message: clip.pinned ? '已取消示例置顶' : '已置顶，在侧栏中快速找到它' });
  }
  function removeClip(clip: Clip) {
    changeClip(clip.id, { deleted: !clip.deleted });
    setMobileDetail(false);
    setToast({
      message: clip.deleted ? '示例内容已恢复' : '示例内容已移入回收站',
      undo: () => changeClip(clip.id, { deleted: !!clip.deleted }),
    });
  }
  async function copyClip() {
    if (!selected) return;
    if (appearance.native) {
      setToast({ message: '系统复制尚未接入，当前为界面交互预览。', info: true });
      return;
    }
    try {
      await navigator.clipboard.writeText(selected.content);
      setToast({ message: '示例文本已复制' });
    } catch {
      setToast({ message: '浏览器未允许复制，请在内容中选择文本复制。', info: true });
    }
  }
  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => setToast(null), toast.undo ? 6000 : 4000);
    return () => clearTimeout(timer);
  }, [toast]);
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.isComposing) return;
      if ((event.metaKey || event.ctrlKey) && event.key === ',') {
        event.preventDefault();
        setSettingsTab('appearance');
        setModal(modal === 'settings' ? null : 'settings');
        return;
      }
      if (modal) return;
      const target = event.target as HTMLElement;
      const editable = target.matches('input, textarea, select, [contenteditable="true"]');
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault();
        search.current?.focus();
        return;
      }
      if (event.key === 'Escape') {
        if (menuOpen) setMenuOpen(false);
        else if (query) setQuery('');
        else if (mobileDetail) setMobileDetail(false);
        return;
      }
      if (editable && target !== search.current) return;
      if (
        (event.key === 'ArrowDown' || event.key === 'ArrowUp') &&
        !event.altKey &&
        !event.ctrlKey &&
        !event.metaKey
      ) {
        if (!result.items.length) return;
        event.preventDefault();
        const index = result.items.findIndex((clip) => clip.id === selected?.id);
        const next = Math.max(
          0,
          Math.min(result.items.length - 1, index + (event.key === 'ArrowDown' ? 1 : -1)),
        );
        const item = result.items[next];
        if (item) {
          setSelectedId(item.id);
          setMenuOpen(false);
          list.current
            ?.querySelector(`[data-clip-id="${item.id}"]`)
            ?.scrollIntoView({ block: 'nearest' });
        }
      }
      if (event.key === 'Enter' && target === search.current) {
        event.preventDefault();
        if (selected) setMobileDetail(true);
      }
      if (event.key === 'Enter' && !editable && target === list.current) {
        event.preventDefault();
        void copyClip();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });
  useEffect(() => {
    if (!menuOpen) return;
    const dismiss = (event: MouseEvent) => {
      if (!(event.target as HTMLElement).closest('.menu-anchor')) setMenuOpen(false);
    };
    window.addEventListener('click', dismiss);
    return () => window.removeEventListener('click', dismiss);
  }, [menuOpen]);

  function createClip(event: FormEvent) {
    event.preventDefault();
    if (!newTitle.trim() || !newContent.trim()) return;
    const clip: Clip = {
      id: `preview-${Date.now()}`,
      type: 'text',
      title: newTitle.trim(),
      content: newContent.trim(),
      source: '手动创建',
      time: new Date().toLocaleTimeString('zh-CN', {
        hour: '2-digit',
        minute: '2-digit',
        hour12: false,
      }),
      day: '今天',
      pinned: false,
      collection: ['work', 'inspiration', 'later'].includes(view) ? (view as Collection) : null,
    };
    setClips((current) => [clip, ...current]);
    setSelectedId(clip.id);
    setView('all');
    setType('all');
    setQuery('');
    setOldest(false);
    setNewTitle('');
    setNewContent('');
    setModal(null);
    setToast({ message: '已添加到本次示例预览，刷新后还原' });
  }
  async function chooseTheme(theme: Theme) {
    if (await appearance.saveTheme(theme))
      setToast({ message: appearance.native ? '外观已保存' : '预览外观已更新' });
  }

  const navButton = (key: View, label: string, icon: IconName, count: number) => (
    <button
      className={`nav-item ${view === key ? 'active' : ''}`}
      aria-current={view === key ? 'page' : undefined}
      onClick={() => navigate(key)}
      title={label}
      key={key}
    >
      <Icon name={icon} size={18} />
      <span>{label}</span>
      <span className="nav-count">{count}</span>
    </button>
  );
  return (
    <>
      <div
        className={`app-shell ${mobileDetail && selected ? 'show-mobile-detail' : ''}`}
        aria-hidden={modal ? true : undefined}
        data-testid="app-ready"
      >
        <a className="skip-link" href="#history-search">
          跳到搜索
        </a>
        <aside className="sidebar" aria-label="工作区导航">
          <div className="brand">
            <BrandMark />
            <span>
              UClip<span className="brand-period">.</span>
            </span>
            <span className="brand-next">NEXT</span>
          </div>
          <button
            className="workspace-switch"
            onClick={() =>
              setToast({ message: '当前是独立示例工作区，未读取真实剪贴板。', info: true })
            }
          >
            <span className="workspace-avatar">
              C<span />
            </span>
            <span>
              <strong>我的空间</strong>
              <small>个人工作区</small>
            </span>
            <Icon name="chevron" size={15} />
          </button>
          <div className="nav-caption">工作台</div>
          <nav className="primary-nav" aria-label="历史范围">
            {navButton('all', '全部记录', 'history', activeClips.length)}
            {navButton('pinned', '已置顶', 'pin', activeClips.filter((clip) => clip.pinned).length)}
          </nav>
          <div className="nav-caption collection-caption">
            我的分类<span>03</span>
          </div>
          <nav className="collection-nav" aria-label="内容分类">
            {(Object.keys(collectionNames) as Collection[]).map((key) => (
              <button
                className={`nav-item ${view === key ? 'active' : ''}`}
                key={key}
                onClick={() => navigate(key)}
                aria-current={view === key ? 'page' : undefined}
                title={collectionNames[key]}
              >
                <span className={`collection-dot ${key}`} />
                <span>{collectionNames[key]}</span>
                <span className="nav-count">
                  {activeClips.filter((clip) => clip.collection === key).length}
                </span>
              </button>
            ))}
          </nav>
          <div className="sidebar-divider" />
          {navButton('trash', '回收站', 'trash', clips.filter((clip) => clip.deleted).length)}
          <div className="sidebar-bottom">
            <div className="workspace-note">
              <span className="workspace-note-icon">
                <Icon name="shield" size={16} />
              </span>
              <div>
                <strong>留住灵感，保持专注</strong>
                <p>让好内容随手可得。</p>
              </div>
            </div>
            <button
              className="nav-item settings-trigger"
              data-testid="open-settings"
              onClick={() => {
                setSettingsTab('appearance');
                setModal('settings');
              }}
              title="设置"
            >
              <Icon name="settings" size={18} />
              <span>设置</span>
              <kbd>{modifier} ,</kbd>
            </button>
            <div className="sidebar-signature">
              <span className="preview-dot" />
              设计预览<span>v0.1</span>
            </div>
          </div>
        </aside>
        <div className="workspace-main">
          <header className="topbar">
            <div className={`search-box ${result.error ? 'has-error' : ''}`}>
              <Icon name="search" size={19} />
              <label className="sr-only" htmlFor="history-search">
                搜索剪贴板示例
              </label>
              <input
                ref={search}
                id="history-search"
                value={query}
                placeholder="搜索你的剪贴板…"
                onChange={(event) => {
                  setQuery(event.target.value);
                  setMenuOpen(false);
                }}
                autoComplete="off"
                spellCheck={false}
                aria-invalid={!!result.error}
                aria-describedby={result.error ? 'search-error' : undefined}
              />
              <button
                className={`regex-toggle ${regex ? 'active' : ''}`}
                title="正则表达式搜索"
                aria-label="正则表达式搜索"
                aria-pressed={regex}
                onClick={() => setRegex(!regex)}
              >
                .*
              </button>
              {query ? (
                <button
                  className="icon-button search-clear"
                  aria-label="清空搜索"
                  onClick={() => {
                    setQuery('');
                    search.current?.focus();
                  }}
                >
                  <Icon name="close" size={15} />
                </button>
              ) : (
                <kbd>{modifier} K</kbd>
              )}
            </div>
            <div className="topbar-actions">
              <span className="preview-badge">
                <span />
                示例工作区
              </span>
              <button
                className="icon-button theme-toggle"
                onClick={() => {
                  void chooseTheme(appearance.dark ? 'light' : 'dark');
                }}
                aria-label={appearance.dark ? '切换浅色主题' : '切换深色主题'}
                disabled={
                  appearance.saving ||
                  appearance.loading ||
                  (appearance.native && !appearance.snapshot)
                }
              >
                <Icon name={appearance.dark ? 'sun' : 'moon'} size={18} />
              </button>
            </div>
          </header>
          <div className="workspace-heading">
            <div>
              <div className="heading-breadcrumb">
                我的空间
                <Icon name="chevron" size={12} />
                {view === 'all' ? '剪贴板' : '内容管理'}
              </div>
              <div className="page-heading">
                <h1>{viewTitles[view]}</h1>
                <span>{result.items.length}</span>
              </div>
            </div>
            <button className="new-button" onClick={() => setModal('new')}>
              <Icon name="plus" size={16} />
              <span>新建片段</span>
            </button>
          </div>
          {appearance.error && (
            <div className="service-error" role="alert" data-testid="service-error">
              <Icon name="info" size={17} />
              <span>
                <strong>本地服务连接失败</strong> · {appearance.error}
              </span>
              <button
                onClick={() => {
                  void appearance.refresh();
                }}
              >
                重试
              </button>
            </div>
          )}
          <div className="filter-bar">
            <div className="type-filters" aria-label="内容类型">
              {filters.map((key) => (
                <button
                  className={type === key ? 'active' : ''}
                  key={key}
                  onClick={() => setType(key)}
                  aria-pressed={type === key}
                >
                  {key !== 'all' && <Icon name={key} size={14} />}
                  {key === 'all' ? '全部' : typeNames[key]}
                </button>
              ))}
            </div>
            <div className="view-controls">
              <button
                className="icon-button sort-control"
                onClick={() => setOldest(!oldest)}
                title={oldest ? '切换为最近复制优先' : '切换为最早复制优先'}
                aria-label={oldest ? '最早复制优先' : '最近复制优先'}
              >
                <Icon name="sort" size={16} />
                <span>{oldest ? '最早复制' : '最近复制'}</span>
              </button>
              <span className="control-divider" />
              <button
                className={`icon-button ${!grid ? 'active' : ''}`}
                aria-label="列表视图"
                aria-pressed={!grid}
                onClick={() => setGrid(false)}
              >
                <Icon name="list" size={16} />
              </button>
              <button
                className={`icon-button ${grid ? 'active' : ''}`}
                aria-label="网格视图"
                aria-pressed={grid}
                onClick={() => setGrid(true)}
              >
                <Icon name="grid" size={15} />
              </button>
            </div>
          </div>
          <main className={`content-workspace ${grid ? 'grid-mode' : ''}`}>
            <section className="history-panel" aria-label="剪贴板历史">
              <div className="list-heading">
                <span>{query ? '搜索结果' : view === 'trash' ? '已删除的示例' : '最近的灵感'}</span>
                <span>{result.items.length} 条内容</span>
              </div>
              <div
                className={`history-scroll ${grid ? 'clip-grid' : ''}`}
                ref={list}
                tabIndex={0}
                aria-label="使用上下方向键选择内容"
              >
                {result.pending ? (
                  <div className="empty-state" role="status">
                    <Icon name="search" size={30} />
                    <strong>正在筛选示例…</strong>
                    <p>复杂表达式会自动停止，可随时修改搜索。</p>
                  </div>
                ) : result.error ? (
                  <div className="empty-state" role="alert" id="search-error">
                    <Icon name="search" size={30} />
                    <strong>检查一下搜索条件</strong>
                    <p>{result.error}</p>
                    <button onClick={() => setRegex(false)}>使用普通搜索</button>
                  </div>
                ) : !result.items.length ? (
                  <div className="empty-state">
                    <Icon name={view === 'trash' ? 'trash' : 'search'} size={32} />
                    <strong>{view === 'trash' ? '这里暂时很干净' : '还没有匹配的内容'}</strong>
                    <p>
                      {view === 'trash'
                        ? '删除的示例会出现在这里，可以随时恢复。'
                        : '试试其他关键词，或换一种内容类型。'}
                    </p>
                    {(query || type !== 'all') && (
                      <button
                        onClick={() => {
                          setQuery('');
                          setType('all');
                        }}
                      >
                        清除筛选
                      </button>
                    )}
                  </div>
                ) : (
                  result.items.map((clip, index) => (
                    <div className="clip-group" key={clip.id}>
                      {!grid && (index === 0 || result.items[index - 1]?.day !== clip.day) && (
                        <div className="day-label">
                          {clip.day}
                          <span />
                        </div>
                      )}
                      <button
                        className={`clip-row ${selected?.id === clip.id ? 'selected' : ''} type-${clip.type}`}
                        data-clip-id={clip.id}
                        aria-pressed={selected?.id === clip.id}
                        onClick={() => {
                          setSelectedId(clip.id);
                          setMobileDetail(true);
                          setMenuOpen(false);
                        }}
                      >
                        {grid && clip.image ? (
                          <img
                            className="grid-image"
                            src={clip.image}
                            width="1200"
                            height="780"
                            alt="山野插画示例"
                          />
                        ) : (
                          <span className={`clip-type-icon ${clip.language ? 'code' : clip.type}`}>
                            <Icon name={clip.language ? 'file' : clip.type} size={18} />
                          </span>
                        )}
                        <span className="clip-row-content">
                          <span className="clip-title">{clip.title}</span>
                          <span className="clip-excerpt">
                            {clip.type === 'image'
                              ? `${clip.size} · 图片`
                              : clip.type === 'file'
                                ? `${clip.size} · 文件引用`
                                : clip.content.replace(/\n/g, ' ')}
                          </span>
                          <span className="clip-row-meta">
                            <span>{clip.source}</span>
                            <span className="separator-dot">·</span>
                            <span>{clip.time}</span>
                            {clip.collection && (
                              <span className={`tiny-collection ${clip.collection}`} />
                            )}
                          </span>
                        </span>
                        <span className="clip-row-trailing">
                          {clip.pinned && <Icon name="pin" size={12} />}
                          {selected?.id === clip.id && <span className="selected-arrow">↵</span>}
                        </span>
                      </button>
                    </div>
                  ))
                )}
              </div>
              <div className="list-footnote">
                <Icon name="info" size={13} />
                <span>示例数据 · 刷新后还原</span>
              </div>
            </section>
            {selected ? (
              <ClipPreview
                clip={selected}
                onPin={() => togglePin(selected)}
                onDelete={() => removeClip(selected)}
                onCollection={(collection) => {
                  changeClip(selected.id, { collection });
                  setToast({ message: `已将示例移入「${collectionNames[collection]}」` });
                }}
                onCopy={() => {
                  void copyClip();
                }}
                onBack={() => setMobileDetail(false)}
                menuOpen={menuOpen}
                setMenuOpen={setMenuOpen}
              />
            ) : (
              <div className="detail-empty">
                <BrandMark />
                <p>让每一段内容，都有一个落点。</p>
                <span>选择一条记录，在这里查看完整内容</span>
              </div>
            )}
          </main>
          <footer className="statusbar">
            <div>
              <span
                className={`status-dot ${appearance.error ? 'error' : appearance.loading ? 'pending' : ''}`}
              />
              <span>
                {appearance.native
                  ? appearance.loading
                    ? '正在连接本地服务'
                    : appearance.error
                      ? '本地服务需要处理'
                      : '本地服务已就绪'
                  : '界面交互预览'}
              </span>
              <span className="statusbar-separator">/</span>
              <span className="statusbar-hint">未读取系统剪贴板</span>
            </div>
            <button onClick={() => setModal('shortcuts')} className="shortcut-hints">
              <span>
                <kbd>↑</kbd>
                <kbd>↓</kbd>选择
              </span>
              <span>
                <kbd>↵</kbd>查看 / 复制
              </span>
              <Icon name="keyboard" size={16} />
            </button>
          </footer>
        </div>
      </div>
      {toast && (
        <div className={`toast ${toast.info ? 'toast-info' : ''}`} role="status">
          <span className="toast-icon">
            <Icon name={toast.info ? 'info' : 'check'} size={16} />
          </span>
          <span>{toast.message}</span>
          {toast.undo && (
            <button
              onClick={() => {
                toast.undo?.();
                setToast(null);
              }}
            >
              撤销
            </button>
          )}
          <button className="icon-button" aria-label="关闭提示" onClick={() => setToast(null)}>
            <Icon name="close" size={14} />
          </button>
        </div>
      )}
      {modal === 'settings' && (
        <Modal
          title="让这里，更像你的空间。"
          description="细微的偏好，刚刚好的工作节奏。"
          onClose={() => setModal(null)}
          wide
        >
          <div className="settings-layout">
            <nav aria-label="设置分类">
              <button
                className={settingsTab === 'appearance' ? 'active' : ''}
                onClick={() => setSettingsTab('appearance')}
              >
                <Icon name="sun" size={16} />
                外观
              </button>
              <button
                className={settingsTab === 'about' ? 'active' : ''}
                onClick={() => setSettingsTab('about')}
              >
                <Icon name="info" size={16} />
                关于与状态
              </button>
            </nav>
            <div className="settings-content">
              {settingsTab === 'appearance' ? (
                <>
                  <h3>界面主题</h3>
                  <p className="setting-help">Moss · 苔序，让内容成为视觉的中心。</p>
                  <div className="theme-options">
                    {(['light', 'dark', 'system'] as Theme[]).map((theme) => (
                      <button
                        className={`theme-option theme-${theme} ${appearance.theme === theme ? 'chosen' : ''}`}
                        key={theme}
                        aria-pressed={appearance.theme === theme}
                        onClick={() => {
                          void chooseTheme(theme);
                        }}
                        disabled={
                          appearance.saving ||
                          appearance.loading ||
                          (appearance.native && !appearance.snapshot)
                        }
                      >
                        <span className="mini-window">
                          <i />
                          <span>
                            <b />
                            <b />
                            <b />
                          </span>
                        </span>
                        <span>
                          {theme === 'light' ? '纸白' : theme === 'dark' ? '深墨' : '跟随系统'}
                          {appearance.theme === theme && <Icon name="check" size={14} />}
                        </span>
                      </button>
                    ))}
                  </div>
                  <div className="setting-row">
                    <label htmlFor="theme">显示主题</label>
                    <select
                      id="theme"
                      value={appearance.theme}
                      onChange={(event) => {
                        void chooseTheme(event.target.value as Theme);
                      }}
                      disabled={
                        appearance.saving ||
                        appearance.loading ||
                        (appearance.native && !appearance.snapshot)
                      }
                    >
                      <option value="system">跟随系统</option>
                      <option value="light">纸白 · 浅色</option>
                      <option value="dark">深墨 · 深色</option>
                    </select>
                  </div>
                  <p className="settings-feedback" role="status">
                    {appearance.saving
                      ? '正在保存外观…'
                      : appearance.error ||
                        (appearance.native
                          ? '外观保存在此设备，重新打开后仍然生效。'
                          : '浏览器只记住外观偏好；示例内容刷新后还原。')}
                  </p>
                  <div className="design-note">
                    <Icon name="shield" size={18} />
                    <div>
                      <strong>一个安静的本地空间</strong>
                      <p>界面资源随应用提供，无需联网加载字体、图片或网站图标。</p>
                    </div>
                  </div>
                </>
              ) : (
                <>
                  <div className="about-brand">
                    <BrandMark />
                    <strong>UClip</strong>
                    <span>0.1.0 · Moss preview</span>
                  </div>
                  <p className="setting-help">
                    这是一版可交互的界面设计。采集、原应用回粘和云同步尚未接入；当前列表全部为示例内容。
                  </p>
                  {appearance.snapshot && (
                    <div className="capability-list">
                      {appearance.snapshot.platform.capabilities.map((item) => (
                        <div key={item.id}>
                          <span>{capabilityNames[item.id] ?? item.id}</span>
                          <span>{stateNames[item.state]}</span>
                        </div>
                      ))}
                    </div>
                  )}
                  <div className="about-meta">
                    <span>
                      {appearance.native
                        ? (appearance.snapshot?.platform.os ?? '桌面服务连接中')
                        : '浏览器预览'}
                    </span>
                    <span>主题设计与业务数据独立</span>
                  </div>
                  {appearance.native && (
                    <button
                      className="secondary-button"
                      disabled={appearance.loading || appearance.saving}
                      onClick={() => {
                        void appearance.refresh();
                      }}
                    >
                      刷新服务状态
                    </button>
                  )}
                </>
              )}
            </div>
          </div>
        </Modal>
      )}
      {modal === 'new' && (
        <Modal
          title="留下一段新想法。"
          description="添加到本次示例预览，刷新后还原。"
          onClose={() => setModal(null)}
        >
          <form className="new-form" onSubmit={createClip}>
            <label htmlFor="clip-title">标题</label>
            <input
              id="clip-title"
              data-autofocus
              value={newTitle}
              onChange={(event) => setNewTitle(event.target.value)}
              placeholder="给这段内容起个名字"
              maxLength={96}
              required
            />
            <label htmlFor="clip-content">内容</label>
            <textarea
              id="clip-content"
              value={newContent}
              onChange={(event) => setNewContent(event.target.value)}
              placeholder="一段文字，一个念头，或者下次要用的片段…"
              maxLength={8000}
              rows={7}
              required
            />
            <div className="form-actions">
              <button className="secondary-button" type="button" onClick={() => setModal(null)}>
                取消
              </button>
              <button className="primary-button" disabled={!newTitle.trim() || !newContent.trim()}>
                添加示例
                <Icon name="arrow" size={16} />
              </button>
            </div>
          </form>
        </Modal>
      )}
      {modal === 'shortcuts' && (
        <Modal
          title="让双手，留在键盘上。"
          description="当前窗口内的快捷操作，不注册系统全局快捷键。"
          onClose={() => setModal(null)}
        >
          <div className="shortcut-list">
            <div>
              <span>聚焦搜索</span>
              <span>
                <kbd>{modifier}</kbd>
                <kbd>K</kbd>
              </span>
            </div>
            <div>
              <span>选择上一条 / 下一条</span>
              <span>
                <kbd>↑</kbd>
                <kbd>↓</kbd>
              </span>
            </div>
            <div>
              <span>搜索框内查看选中内容</span>
              <kbd>Enter</kbd>
            </div>
            <div>
              <span>关闭面板 / 清空搜索 / 返回列表</span>
              <kbd>Esc</kbd>
            </div>
            <div>
              <span>在控件之间移动</span>
              <kbd>Tab</kbd>
            </div>
          </div>
          <p className="setting-help">桌面系统复制尚未接入。浏览器预览可通过按钮复制示例文本。</p>
        </Modal>
      )}
    </>
  );
}
