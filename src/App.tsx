import { useEffect, useRef, useState, type FormEvent } from 'react';
import { Icon } from './components/Icon';
import { Modal } from './components/Modal';
import { ActionMenu } from './components/ActionMenu';
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
import { useHistory } from './platform/useHistory';
import { KeyboardSettings } from './components/KeyboardSettings';
import { DEFAULT_KEYS, matchesKey, formatKey } from './platform/keyboard';
import { useDesktop } from './platform/useDesktop';
import { safeError } from './platform/client';
import { useAppearance } from './platform/useAppearance';
import type { Theme, CollectionItem, TransferMode } from './generated/contracts';

type Toast = { message: string; undo?: () => void };
type Menu = { anchor: HTMLElement; clipId?: string };
type AppView = View | `collection:${string}`;
type AppModal = 'settings' | 'new' | 'shortcuts' | 'deduplicate' | 'collection' | 'transfer';
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
  const [view, setView] = useState<AppView>('all');
  const [type, setType] = useState<ClipType | 'all'>('all');
  const [query, setQuery] = useState('');
  const [regex, setRegex] = useState(false);
  const [oldest, setOldest] = useState(false);
  const [grid, setGrid] = useState(false);
  const [selectedId, setSelectedId] = useState('note-1');
  const [preview, setPreview] = useState<{ id: string; focus: HTMLElement | null } | null>(null);
  const [menu, setMenu] = useState<Menu | null>(null);
  const [modal, setModal] = useState<AppModal | null>(null);
  const [collectionName, setCollectionName] = useState('');
  const [editingCollection, setEditingCollection] = useState<CollectionItem | null>(null);
  const [collectionError, setCollectionError] = useState('');
  const [transfer, setTransfer] = useState<{ clip: Clip; mode: TransferMode } | null>(null);
  const [transferTarget, setTransferTarget] = useState('');
  const [transferError, setTransferError] = useState('');
  const [dedupRevision, setDedupRevision] = useState('');
  const [dedupError, setDedupError] = useState('');
  const [toast, setToast] = useState<Toast | null>(null);
  const desktop = useDesktop(appearance.native, !!(modal || menu || preview));
  const [shortcut, setShortcut] = useState('CommandOrControl+Shift+V');
  const copying = useRef(false);
  useEffect(() => {
    if (desktop.status) setShortcut(desktop.status.preferences.shortcut);
  }, [desktop.status?.preferences.revision]);
  const [newTitle, setNewTitle] = useState('');
  const [newContent, setNewContent] = useState('');
  const modalFocus = useRef<HTMLElement | null>(null);
  const search = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const nativeCollectionId =
    appearance.native && view.startsWith('collection:') ? view.slice(11) : null;
  const live = useHistory(
    appearance.native,
    query,
    regex,
    type,
    view === 'pinned',
    nativeCollectionId,
    preview?.id,
  );
  const currentCollection = live.collections.find((item) => item.id === nativeCollectionId);
  const demoResult = useClipQuery(
    clips,
    appearance.native ? 'all' : (view as View),
    type,
    appearance.native ? '' : query,
    !appearance.native && regex,
    oldest,
  );
  const result = appearance.native
    ? { items: live.items, pending: live.pending, error: live.error }
    : demoResult;
  const [policyPattern, setPolicyPattern] = useState('');
  const [policyMax, setPolicyMax] = useState(500);
  useEffect(() => {
    if (live.status) {
      setPolicyPattern(live.status.policy.excludePattern);
      setPolicyMax(live.status.policy.maxItems);
    }
  }, [live.status?.policy.revision]);
  const selected = result.items.find((clip) => clip.id === selectedId) ?? result.items[0];
  const previewBase = preview && result.items.find((clip) => clip.id === preview.id);
  const previewClip =
    previewBase && appearance.native
      ? {
          ...previewBase,
          content: live.detail?.id === previewBase.id ? live.detail.text : '正在读取完整内容…',
          image:
            live.detail?.id === previewBase.id
              ? (live.detail.imageDataUrl ?? undefined)
              : undefined,
        }
      : previewBase;
  const menuClip = menu?.clipId ? result.items.find((clip) => clip.id === menu.clipId) : undefined;
  const modifier =
    appearance.snapshot?.platform.os === 'macos' ||
    (!appearance.native && /Mac|iPhone|iPad/.test(navigator.platform))
      ? '⌘'
      : 'Ctrl';
  const activeKeys = desktop.status?.keyboard.bindings ?? DEFAULT_KEYS;
  const ready = !result.pending && !result.error;

  function changeClip(id: string, update: Partial<Clip>) {
    setClips((current) => current.map((clip) => (clip.id === id ? { ...clip, ...update } : clip)));
  }
  async function togglePin(clip: Clip) {
    if (!ready || live.mutating || clip.deleted) return;
    if (appearance.native) {
      if (!clip.revision) return;
      try {
        await live.changePin({
          id: clip.id,
          expectedRevision: clip.revision,
          pinned: !clip.pinned,
        });
        setToast({ message: clip.pinned ? '已取消置顶' : '已置顶' });
      } catch (error) {
        setToast({ message: safeError(error).message });
      }
      return;
    }
    changeClip(clip.id, { pinned: !clip.pinned });
    setToast({ message: clip.pinned ? '已取消示例置顶' : '已置顶示例' });
  }
  async function deduplicatePinned() {
    if (live.mutating) return;
    setDedupError('');
    try {
      const result = await live.deduplicatePinned(dedupRevision);
      closeModal();
      setToast({
        message: result.removed
          ? `去重完成，已删除 ${result.removed} 条非置顶重复记录`
          : '没有需要删除的非置顶重复记录',
      });
    } catch (error) {
      setDedupError(safeError(error).message);
    }
  }
  function removeClip(clip: Clip) {
    if (appearance.native) {
      void deleteNativeClip(clip);
      return;
    }
    const index = result.items.findIndex((item) => item.id === clip.id);
    setSelectedId((result.items[index + 1] ?? result.items[index - 1])?.id ?? '');
    changeClip(clip.id, { deleted: !clip.deleted });
    if (preview) closePreview();
    else requestAnimationFrame(() => list.current?.focus({ preventScroll: true }));
    setToast({
      message: clip.deleted ? '示例已恢复' : '示例已移入回收站',
      undo: () => changeClip(clip.id, { deleted: !!clip.deleted }),
    });
  }
  async function deleteNativeClip(clip: Clip) {
    if (!ready || live.mutating || !clip.revision) return;
    const index = result.items.findIndex((item) => item.id === clip.id);
    const nextId = (result.items[index + 1] ?? result.items[index - 1])?.id ?? '';
    try {
      const deleted = await live.changeDeletion({
        id: clip.id,
        expectedRevision: clip.revision,
        deleted: true,
      });
      setSelectedId(nextId);
      if (preview) closePreview();
      else requestAnimationFrame(() => list.current?.focus({ preventScroll: true }));
      setToast({
        message: '已删除',
        undo: () => {
          void (async () => {
            try {
              await live.changeDeletion({
                id: deleted.id,
                expectedRevision: deleted.revision,
                deleted: false,
              });
              setSelectedId(deleted.id);
              setToast({ message: '已撤销删除' });
            } catch (error) {
              setToast({ message: safeError(error).message });
            }
          })();
        },
      });
    } catch (error) {
      setToast({ message: safeError(error).message });
    }
  }
  async function copyClip(clip: Clip, paste = false) {
    if (!ready || !result.items.some((item) => item.id === clip.id)) return;
    if (clip.deleted) {
      removeClip(clip);
      return;
    }
    if (appearance.native) {
      if (copying.current) return;
      copying.current = true;
      try {
        const outcome = await live.copy(clip.id, paste);
        setToast({
          message: outcome.warning
            ? `${outcome.pasteRequested ? '已复制并请求原应用粘贴' : '已复制'}；${safeError(outcome.warning).message}`
            : outcome.pasteRequested
              ? '已复制并请求原应用粘贴'
              : '已复制',
        });
      } catch (error) {
        setToast({ message: safeError(error).message });
      } finally {
        copying.current = false;
      }
      return;
    }
    if (clip.type === 'image' || clip.type === 'file') {
      setToast({ message: `${typeNames[clip.type]}复制尚未接入。` });
      return;
    }
    try {
      await navigator.clipboard.writeText(clip.content);
      setToast({ message: '示例文本已复制' });
    } catch {
      setToast({ message: '浏览器未允许复制，可在完整内容中选择文本复制。' });
    }
  }
  async function saveCapture(enabled: boolean, draft = false) {
    if (!live.status) return;
    const policy = live.status.policy;
    try {
      await live.updatePolicy({
        enabled,
        excludePattern: draft ? policyPattern : policy.excludePattern,
        maxItems: draft ? policyMax : policy.maxItems,
        expectedRevision: policy.revision,
      });
      setToast({ message: enabled ? '采集规则已生效；仅记录之后的新复制' : '采集已暂停' });
    } catch (error) {
      setToast({ message: safeError(error).message });
    }
  }
  function openPreview(clip: Clip) {
    if (!ready) return;
    setSelectedId(clip.id);
    setPreview({ id: clip.id, focus: menu?.anchor ?? (document.activeElement as HTMLElement) });
  }
  function closePreview() {
    const previous = preview?.focus;
    setPreview(null);
    requestAnimationFrame(() => {
      (previous?.isConnected ? previous : list.current)?.focus({ preventScroll: true });
    });
  }
  function openMenu(anchor: HTMLElement, clip?: Clip) {
    if (clip && !ready) return;
    if (menu?.anchor === anchor) {
      setMenu(null);
      return;
    }
    if (clip) setSelectedId(clip.id);
    setMenu({ anchor, clipId: clip?.id });
  }
  function menuAction(action: () => void) {
    setMenu(null);
    action();
  }
  function closeModal() {
    setModal(null);
    requestAnimationFrame(() => {
      const previous = modalFocus.current;
      (previous?.isConnected ? previous : search.current)?.focus({ preventScroll: true });
    });
  }
  function openModal(next: AppModal) {
    modalFocus.current = menu?.anchor ?? (document.activeElement as HTMLElement);
    setMenu(null);
    setModal(next);
  }
  function openCollectionEditor(collection: CollectionItem | null) {
    setEditingCollection(collection);
    setCollectionName(collection?.name ?? '');
    setCollectionError('');
    openModal('collection');
  }
  async function saveCollection(event: FormEvent) {
    event.preventDefault();
    if (live.mutating) return;
    setCollectionError('');
    try {
      const saved = editingCollection
        ? await live.renameCollection({
            id: editingCollection.id,
            expectedRevision: editingCollection.revision,
            name: collectionName,
          })
        : await live.createCollection(collectionName);
      if (!editingCollection) {
        setView(`collection:${saved.id}`);
        setQuery('');
        setType('all');
        setRegex(false);
      }
      closeModal();
      setToast({ message: editingCollection ? '分类已重命名' : '分类已创建' });
    } catch (error) {
      setCollectionError(safeError(error).message);
    }
  }
  function openTransfer(clip: Clip, mode: TransferMode) {
    setTransfer({ clip, mode });
    setTransferTarget(live.collections.find((item) => item.id !== clip.collectionId)?.id ?? '');
    setTransferError('');
    openModal('transfer');
  }
  async function saveTransfer(event: FormEvent) {
    event.preventDefault();
    if (!transfer?.clip.revision || live.mutating) return;
    setTransferError('');
    try {
      const result = await live.transferClip({
        id: transfer.clip.id,
        expectedRevision: transfer.clip.revision,
        collectionId: transferTarget || null,
        mode: transfer.mode,
      });
      setSelectedId(result.id);
      setView(transferTarget ? `collection:${transferTarget}` : 'all');
      setQuery('');
      setType('all');
      setRegex(false);
      setPreview(null);
      closeModal();
      setToast({
        message:
          transfer.mode === 'copy'
            ? '已复制到分类'
            : transferTarget
              ? '已移动到分类'
              : '已移出分类',
      });
    } catch (error) {
      setTransferError(safeError(error).message);
    }
  }
  function clearFilters() {
    setView('all');
    setType('all');
    setQuery('');
    setRegex(false);
  }

  useEffect(() => {
    search.current?.focus();
  }, []);
  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => setToast(null), toast.undo ? 6000 : 4000);
    return () => clearTimeout(timer);
  }, [toast]);
  useEffect(() => {
    if (preview && !previewClip && !result.pending) closePreview();
    if (menu?.clipId && !menuClip) setMenu(null);
  });
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.isComposing || event.keyCode === 229 || event.defaultPrevented || menu) return;
      const keys = appearance.native ? desktop.status?.keyboard.bindings : DEFAULT_KEYS;
      if (!keys) return;
      const is = (action: keyof typeof keys) =>
        matchesKey(event, keys[action], modifier === '⌘', action === 'up' || action === 'down');
      if (is('settings') && !modal) {
        event.preventDefault();
        openModal('settings');
        return;
      }
      if (modal) return;
      if (event.key === 'Escape' || is('dismiss')) {
        if (preview) {
          event.preventDefault();
          closePreview();
        } else if (query) {
          event.preventDefault();
          setQuery('');
        } else if (appearance.native) {
          event.preventDefault();
          void desktop.hide().catch((error) => setToast({ message: safeError(error).message }));
        }
        return;
      }
      if (is('search')) {
        event.preventDefault();
        if (preview) closePreview();
        requestAnimationFrame(() => search.current?.focus());
        return;
      }
      if (preview) return;
      const target = event.target as HTMLElement;
      const inList = target === list.current || target.matches('[data-row-main]');
      if (target !== search.current && !inList) return;
      // Option/Alt character entry in the search field keeps its native editing behavior.
      if (
        target === search.current &&
        event.altKey &&
        !event.ctrlKey &&
        !event.metaKey &&
        /^(Key|Digit|Comma|Period|Slash)/.test(event.code)
      )
        return;
      if (is('down') || is('up')) {
        if (!ready || !result.items.length) return;
        event.preventDefault();
        const index = result.items.findIndex((clip) => clip.id === selected?.id);
        const next =
          result.items[
            Math.max(0, Math.min(result.items.length - 1, index + (is('down') ? 1 : -1)))
          ];
        if (next) {
          setSelectedId(next.id);
          list.current
            ?.querySelector(`[data-clip-id="${next.id}"]`)
            ?.scrollIntoView({ block: 'nearest' });
          if (inList && target !== list.current)
            list.current
              ?.querySelector<HTMLElement>(`[data-clip-id="${next.id}"] [data-row-main]`)
              ?.focus({ preventScroll: true });
        }
      }
      if (is('remove') && inList) {
        event.preventDefault();
        if (ready && selected) removeClip(selected);
        return;
      }
      if (is('copy')) {
        event.preventDefault();
        if (ready && selected) void copyClip(selected);
      }
      if (is('preview') && inList) {
        event.preventDefault();
        if (ready && selected) openPreview(selected);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  function createClip(event: FormEvent) {
    event.preventDefault();
    if (!newContent) return;
    if (appearance.native) {
      void (async () => {
        try {
          const id = await live.createText(newContent, nativeCollectionId);
          setSelectedId(id);
          if (!nativeCollectionId) setView('all');
          setQuery('');
          setType('all');
          setRegex(false);
          setNewContent('');
          closeModal();
          setToast({ message: '文本已保存' });
        } catch (error) {
          setToast({ message: safeError(error).message });
        }
      })();
      return;
    }
    const clip: Clip = {
      id: `preview-${Date.now()}`,
      type: 'text',
      title: newTitle.trim(),
      content: newContent,
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
    setToast({ message: '已添加示例，刷新后还原' });
    requestAnimationFrame(() => {
      if (list.current) {
        list.current.scrollTop = 0;
        list.current.focus();
      }
    });
  }
  async function saveDesktop(
    update: Partial<
      Pick<
        import('./generated/contracts').UpdateDesktopPreferences,
        'closeToTray' | 'alwaysOnTop' | 'autoHide' | 'shortcut'
      >
    >,
  ) {
    if (!desktop.status) return;
    const p = desktop.status.preferences;
    try {
      await desktop.save({
        closeToTray: p.closeToTray,
        alwaysOnTop: p.alwaysOnTop,
        autoHide: p.autoHide,
        shortcut: p.shortcut,
        expectedRevision: p.revision,
        ...update,
      });
      setToast({ message: '桌面设置已保存' });
    } catch (error) {
      setToast({ message: safeError(error).message });
    }
  }
  async function chooseTheme(theme: Theme) {
    if (await appearance.saveTheme(theme))
      setToast({ message: appearance.native ? '外观已保存' : '预览外观已更新' });
  }

  return (
    <>
      <div className="app-shell" inert={modal ? true : undefined} data-testid="app-ready">
        <header className="topbar">
          <div className={`search-box ${result.error ? 'has-error' : ''}`}>
            <Icon name="search" size={20} />
            <label className="sr-only" htmlFor="history-search">
              搜索历史
            </label>
            <input
              ref={search}
              id="history-search"
              value={query}
              placeholder="搜索历史…"
              onChange={(event) => setQuery(event.target.value)}
              autoComplete="off"
              spellCheck={false}
              aria-invalid={!!result.error}
              aria-describedby={result.error ? 'search-error' : undefined}
            />
            {query && (
              <button
                className="icon-button"
                aria-label="清空搜索"
                onClick={() => {
                  setQuery('');
                  search.current?.focus();
                }}
              >
                <Icon name="close" size={16} />
              </button>
            )}
            <button
              className={`regex-toggle ${regex ? 'active' : ''}`}
              aria-label="正则表达式搜索"
              aria-pressed={regex}
              onClick={() => setRegex(!regex)}
            >
              .*
            </button>
          </div>
          <div className="filter-bar">
            <label className="scope-control">
              <Icon
                name={view === 'trash' ? 'trash' : view === 'pinned' ? 'pin' : 'history'}
                size={15}
              />
              <span className="sr-only">历史范围</span>
              <select
                aria-label="历史范围"
                value={view}
                onChange={(event) => setView(event.target.value as AppView)}
              >
                {Object.entries(
                  appearance.native
                    ? {
                        all: '全部记录',
                        pinned: '已置顶',
                        ...Object.fromEntries(
                          live.collections.map((item) => [`collection:${item.id}`, item.name]),
                        ),
                        trash: '回收站',
                      }
                    : viewTitles,
                ).map(([key, label]) => (
                  <option key={key} value={key} disabled={appearance.native && key === 'trash'}>
                    {label}
                    {appearance.native && key === 'trash' ? '（待接入）' : ''}
                  </option>
                ))}
              </select>
              <Icon name="chevron" size={12} />
            </label>
            <label className="type-control">
              <span className="sr-only">内容类型</span>
              <select
                aria-label="内容类型"
                value={type}
                onChange={(event) => setType(event.target.value as ClipType | 'all')}
              >
                <option value="all">所有类型</option>
                {Object.entries(typeNames).map(([key, label]) => (
                  <option key={key} value={key} disabled={appearance.native && key === 'link'}>
                    {label}
                  </option>
                ))}
              </select>
              <Icon name="chevron" size={12} />
            </label>
            {(view !== 'all' || type !== 'all' || regex) && (
              <button className="text-button clear-filters" onClick={clearFilters}>
                重置
              </button>
            )}
            <span className="result-count" aria-live="polite">
              {result.pending
                ? '搜索中'
                : `${appearance.native ? live.total : result.items.length} 条`}
            </span>
            {appearance.native && view === 'pinned' && (
              <button
                className="text-button"
                disabled={!ready || live.mutating || !live.revision}
                onClick={() => {
                  setDedupRevision(live.revision!);
                  setDedupError('');
                  openModal('deduplicate');
                }}
                aria-label="置顶页去重"
              >
                去重
              </button>
            )}
            <button
              className="icon-button app-more"
              aria-label="更多选项"
              aria-haspopup="menu"
              aria-expanded={!!menu && !menu.clipId}
              onClick={(event) => openMenu(event.currentTarget)}
            >
              <Icon name="more" size={20} />
            </button>
          </div>
        </header>
        {(appearance.error || live.statusError || live.status?.error) && (
          <div className="service-error" role="alert" data-testid="service-error">
            <Icon name="info" size={16} />
            <span>
              本地服务状态 ·{' '}
              {appearance.error ||
                live.statusError ||
                (live.status?.error ? safeError(live.status.error).message : '')}
            </span>
            <button
              className="text-button"
              onClick={() => {
                void appearance.refresh();
                void live.refresh();
              }}
            >
              刷新状态
            </button>
          </div>
        )}
        <main className="content-workspace">
          <section className="history-panel" hidden={!!previewClip} aria-label="剪贴板历史">
            <div
              ref={list}
              className={`history-scroll ${grid ? 'clip-grid' : ''}`}
              tabIndex={0}
              aria-label="历史列表，方向键选择，空格预览"
              aria-busy={result.pending}
            >
              {result.pending && !result.items.length ? (
                <div className="empty-state" role="status">
                  <Icon name="search" size={24} />
                  <strong>正在搜索…</strong>
                </div>
              ) : result.error ? (
                <div className="empty-state" role="alert" id="search-error">
                  <Icon name="info" size={24} />
                  <strong>搜索未完成</strong>
                  <p>{result.error}</p>
                  {regex && (
                    <button className="secondary-button" onClick={() => setRegex(false)}>
                      使用普通搜索
                    </button>
                  )}
                  {appearance.native && (
                    <button className="secondary-button" onClick={live.retryQuery}>
                      重试搜索
                    </button>
                  )}
                </div>
              ) : !result.items.length ? (
                <div className="empty-state">
                  <Icon name={view === 'trash' ? 'trash' : 'search'} size={26} />
                  <strong>
                    {view === 'trash' && !query && type === 'all'
                      ? '回收站为空'
                      : query || type !== 'all'
                        ? '没有匹配的记录'
                        : '暂无记录'}
                  </strong>
                  {(query || view !== 'all' || type !== 'all') && (
                    <button className="text-button" onClick={clearFilters}>
                      清除筛选
                    </button>
                  )}
                </div>
              ) : (
                result.items.map((clip) => (
                  <article
                    key={clip.id}
                    data-clip-id={clip.id}
                    data-clip-revision={clip.revision}
                    className={`clip-row ${selected?.id === clip.id ? 'selected' : ''} ${clip.type === 'image' ? 'image-row' : ''}`}
                    onContextMenu={(event) => {
                      event.preventDefault();
                      openMenu(event.currentTarget.querySelector<HTMLElement>('.row-more')!, clip);
                    }}
                  >
                    <button
                      className="row-main"
                      data-row-main
                      aria-pressed={selected?.id === clip.id}
                      aria-label={`${typeNames[clip.type]}：${clip.type === 'text' ? clip.content.slice(0, 60) : clip.title}`}
                      onFocus={() => setSelectedId(clip.id)}
                      onClick={() => setSelectedId(clip.id)}
                      onDoubleClick={() => {
                        if (appearance.native) void copyClip(clip, true);
                      }}
                    >
                      {clip.image ? (
                        <img className="row-thumbnail" src={clip.image} alt="" />
                      ) : (
                        <span className="row-icon">
                          <Icon name={clip.type} size={19} />
                        </span>
                      )}
                      <span className="row-content">
                        <span
                          className={`clip-excerpt ${clip.type === 'image' || clip.type === 'file' ? 'file-title' : ''}`}
                        >
                          {clip.type === 'image' ? (
                            clip.size
                          ) : appearance.native ? (
                            <Highlighted
                              text={clip.type === 'file' ? clip.title : clip.content}
                              ranges={clip.matchRanges}
                            />
                          ) : clip.type === 'file' ? (
                            clip.title
                          ) : (
                            clip.content.replace(/\n\s*\n/g, '\n')
                          )}
                        </span>
                        <span className="clip-row-meta">
                          {clip.source}
                          <span>·</span>
                          {clip.time}
                          {clip.collectionId && (
                            <>
                              <span>·</span>
                              <span className="clip-collection">
                                {live.collections.find((item) => item.id === clip.collectionId)
                                  ?.name ?? '分类不可用'}
                              </span>
                            </>
                          )}
                          {clip.pinned && (
                            <>
                              <Icon name="pin" size={12} />
                              <span className="sr-only">已置顶</span>
                            </>
                          )}
                        </span>
                      </span>
                    </button>
                    <div className="row-actions">
                      <button
                        className="row-copy text-button"
                        aria-label={`${clip.deleted ? '恢复' : '复制'}：${clip.title || clip.content.slice(0, 20)}`}
                        onClick={() => void copyClip(clip)}
                      >
                        {clip.deleted ? '恢复' : '复制'}
                      </button>
                      <button
                        className="icon-button row-more"
                        aria-label={`更多操作：${clip.title || clip.content.slice(0, 20)}`}
                        aria-haspopup="menu"
                        aria-expanded={menu?.clipId === clip.id}
                        onClick={(event) => openMenu(event.currentTarget, clip)}
                      >
                        <Icon name="more" size={17} />
                      </button>
                    </div>
                  </article>
                ))
              )}
            </div>
          </section>
          {previewClip && (
            <ClipPreview
              clip={previewClip}
              onBack={closePreview}
              onCopy={() => void copyClip(previewClip)}
              onPin={() => void togglePin(previewClip)}
              pinDisabled={!ready || live.mutating}
              collectionName={
                previewClip.collectionId
                  ? (live.collections.find((item) => item.id === previewClip.collectionId)?.name ??
                    '分类不可用')
                  : undefined
              }
              error={appearance.native ? live.detailError : undefined}
              onMore={(anchor) => openMenu(anchor, previewClip)}
            />
          )}
        </main>
        <footer className="statusbar">
          {appearance.native ? (
            <button
              className="text-button capture-toggle"
              disabled={
                !live.status ||
                live.mutating ||
                (!live.status.available && !live.status.policy.enabled)
              }
              onClick={() => void saveCapture(!live.status?.policy.enabled)}
            >
              {!live.status
                ? '正在连接…'
                : !live.status.available
                  ? live.status.policy.enabled
                    ? '采集不可用 · 点击暂停'
                    : '采集不可用'
                  : live.status.policy.enabled
                    ? '● 采集中 · 点击暂停'
                    : '开始采集'}
            </button>
          ) : (
            <span>示例数据 · 刷新后还原</span>
          )}
          {appearance.native && live.total > 25 && (
            <div className="pagination">
              <button
                className="text-button"
                disabled={live.pending || !live.offset}
                onClick={() => live.setOffset(Math.max(0, live.offset - 25))}
              >
                上一页
              </button>
              <span>{Math.floor(live.offset / 25) + 1}</span>
              <button
                className="text-button"
                disabled={live.pending || live.offset + 25 >= live.total}
                onClick={() => live.setOffset(live.offset + 25)}
              >
                下一页
              </button>
            </div>
          )}
          <button
            className="text-button"
            onClick={() => openModal('shortcuts')}
            aria-label="快捷键帮助"
          >
            <span className="footer-hint">↵ {view === 'trash' ? '恢复' : '复制'}</span>
            <Icon name="keyboard" size={16} />
          </button>
        </footer>
      </div>
      {menu && (
        <ActionMenu anchor={menu.anchor} onClose={() => setMenu(null)}>
          {menuClip ? (
            <>
              {appearance.native && (
                <button
                  role="menuitem"
                  disabled={!live.status?.pasteAvailable || !live.status.pasteTarget}
                  onClick={() => menuAction(() => void copyClip(menuClip, true))}
                >
                  <Icon name="arrow" size={15} />
                  粘贴到原应用
                </button>
              )}
              <button role="menuitem" onClick={() => menuAction(() => openPreview(menuClip))}>
                <Icon name="text" size={15} />
                查看完整内容
              </button>
              <button
                role="menuitem"
                data-testid="toggle-pin"
                disabled={!ready || live.mutating || menuClip.deleted}
                onClick={() => menuAction(() => void togglePin(menuClip))}
              >
                <Icon name="pin" size={15} />
                {menuClip.pinned ? '取消置顶' : '置顶'}
              </button>
              {appearance.native && (
                <>
                  <button
                    role="menuitem"
                    disabled={!ready || live.mutating}
                    data-testid="move-to-collection"
                    onClick={() => menuAction(() => openTransfer(menuClip, 'move'))}
                  >
                    <Icon name="folder" size={15} />
                    移动到分类…
                  </button>
                  <button
                    role="menuitem"
                    disabled={!ready || live.mutating || !live.collections.length}
                    data-testid="copy-to-collection"
                    onClick={() => menuAction(() => openTransfer(menuClip, 'copy'))}
                  >
                    <Icon name="copy" size={15} />
                    复制到分类…
                  </button>
                </>
              )}
              {!appearance.native && !menuClip.deleted && (
                <>
                  <div className="menu-separator" />
                  <p className="menu-label">移入分类</p>
                  {(Object.keys(collectionNames) as Collection[]).map((key) => (
                    <button
                      role="menuitem"
                      key={key}
                      onClick={() =>
                        menuAction(() => {
                          changeClip(menuClip.id, { collection: key });
                          setToast({ message: `示例已移入「${collectionNames[key]}」` });
                        })
                      }
                    >
                      <Icon name="folder" size={15} />
                      {collectionNames[key]}
                      {menuClip.collection === key && <Icon name="check" size={14} />}
                    </button>
                  ))}
                </>
              )}
              <div className="menu-separator" />
              <button
                role="menuitem"
                className={menuClip.deleted ? '' : 'destructive'}
                disabled={!ready || live.mutating}
                onClick={() => menuAction(() => removeClip(menuClip))}
              >
                <Icon name={menuClip.deleted ? 'restore' : 'trash'} size={15} />
                {menuClip.deleted ? '恢复' : appearance.native ? '删除' : '移入回收站'}
              </button>
            </>
          ) : (
            <>
              <button
                role="menuitem"
                data-testid="new-text"
                onClick={() => menuAction(() => openModal('new'))}
              >
                <Icon name="plus" size={15} />
                {appearance.native ? '新建文本' : '新建示例'}
              </button>
              {appearance.native && (
                <>
                  <button
                    role="menuitem"
                    data-testid="new-collection"
                    disabled={live.mutating}
                    onClick={() => menuAction(() => openCollectionEditor(null))}
                  >
                    <Icon name="folder" size={15} />
                    新建分类
                  </button>
                  {currentCollection && (
                    <button
                      role="menuitem"
                      data-testid="rename-collection"
                      disabled={live.mutating}
                      onClick={() => menuAction(() => openCollectionEditor(currentCollection))}
                    >
                      <Icon name="folder" size={15} />
                      重命名当前分类
                    </button>
                  )}
                </>
              )}
              <button role="menuitem" onClick={() => menuAction(() => setGrid(!grid))}>
                <Icon name={grid ? 'list' : 'grid'} size={15} />
                {grid ? '切换列表视图' : '切换网格视图'}
              </button>
              <button
                role="menuitem"
                disabled={appearance.native}
                onClick={() => menuAction(() => setOldest(!oldest))}
              >
                <Icon name="sort" size={15} />
                {oldest ? '改为最近优先' : '改为最早优先'}
              </button>
              <div className="menu-separator" />
              <button
                role="menuitem"
                data-testid="open-settings"
                aria-label="设置"
                onClick={() => menuAction(() => openModal('settings'))}
              >
                <Icon name="settings" size={15} />
                设置
                <span className="menu-shortcut">{formatKey(activeKeys.settings, modifier)}</span>
              </button>
              <button role="menuitem" onClick={() => menuAction(() => openModal('shortcuts'))}>
                <Icon name="keyboard" size={15} />
                快捷键帮助
              </button>
              {appearance.native && (
                <button
                  role="menuitem"
                  onClick={() =>
                    menuAction(() => {
                      void desktop.quit();
                    })
                  }
                >
                  <Icon name="close" size={15} />
                  退出应用
                </button>
              )}
            </>
          )}
        </ActionMenu>
      )}
      {toast && (
        <div className="toast" role="status">
          <span>{toast.message}</span>
          {toast.undo && (
            <button
              className="text-button"
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
      {modal === 'collection' && (
        <Modal
          title={editingCollection ? '重命名分类' : '新建分类'}
          onClose={() => {
            if (!live.mutating) closeModal();
          }}
          description="名称最多 64 个字符，同名分类不可重复创建。"
        >
          <form className="new-form" onSubmit={saveCollection}>
            <label htmlFor="collection-name">分类名称</label>
            <input
              id="collection-name"
              data-autofocus
              value={collectionName}
              onChange={(event) => setCollectionName(event.target.value)}
              maxLength={128}
              required
            />
            {collectionError && (
              <p role="alert" className="settings-feedback">
                {collectionError}
              </p>
            )}
            <div className="form-actions">
              <button
                type="button"
                className="secondary-button"
                disabled={live.mutating}
                onClick={closeModal}
              >
                取消
              </button>
              <button
                className="primary-button"
                disabled={
                  live.mutating ||
                  !collectionName.trim() ||
                  collectionName.trim() === editingCollection?.name
                }
              >
                保存分类
              </button>
            </div>
          </form>
        </Modal>
      )}
      {modal === 'transfer' && transfer && (
        <Modal
          title={transfer.mode === 'copy' ? '复制到分类' : '移动到分类'}
          onClose={() => {
            if (!live.mutating) closeModal();
          }}
          description={
            transfer.mode === 'copy'
              ? '保留原记录，在目标分类创建独立副本，副本默认不置顶。'
              : '保留记录身份和置顶状态，更改所属分类。'
          }
        >
          <form className="new-form" onSubmit={saveTransfer}>
            <label htmlFor="transfer-collection">目标分类</label>
            <select
              id="transfer-collection"
              data-autofocus
              value={transferTarget}
              onChange={(event) => setTransferTarget(event.target.value)}
            >
              {transfer.mode === 'move' && <option value="">未分类</option>}
              {transfer.mode === 'copy' && !transferTarget && (
                <option value="" disabled>
                  选择分类
                </option>
              )}
              {live.collections.map((item) => (
                <option key={item.id} value={item.id}>
                  {item.name}
                </option>
              ))}
            </select>
            {!live.collections.length && (
              <p className="settings-feedback">可在更多选项中先新建分类。</p>
            )}
            {transferError && (
              <p role="alert" className="settings-feedback">
                {transferError}
              </p>
            )}
            <div className="form-actions">
              <button
                type="button"
                className="secondary-button"
                disabled={live.mutating}
                onClick={closeModal}
              >
                取消
              </button>
              <button
                className="primary-button"
                disabled={
                  live.mutating ||
                  (transfer.mode === 'copy'
                    ? !transferTarget
                    : (transfer.clip.collectionId ?? '') === transferTarget)
                }
              >
                {transfer.mode === 'copy' ? '复制记录' : '移动记录'}
              </button>
            </div>
          </form>
        </Modal>
      )}
      {modal === 'deduplicate' && (
        <Modal
          title="置顶页去重"
          onClose={() => {
            if (!live.mutating) closeModal();
          }}
          description="以全部置顶记录为依据，删除全部历史中内容完全相同的非置顶记录。所有置顶记录都会保留；当前搜索、类型筛选和页码不限制去重范围。"
        >
          {dedupError && (
            <p role="alert" className="settings-feedback">
              {dedupError} 请关闭此面板，刷新后重新检查范围。
            </p>
          )}
          <div className="form-actions">
            <button
              className="secondary-button"
              data-autofocus
              disabled={live.mutating}
              onClick={closeModal}
            >
              取消
            </button>
            <button
              className="primary-button"
              disabled={live.mutating || !!dedupError}
              onClick={() => void deduplicatePinned()}
            >
              {live.mutating ? '正在去重…' : '删除非置顶重复项'}
            </button>
          </div>
        </Modal>
      )}
      {modal === 'settings' && (
        <Modal title="设置" onClose={closeModal}>
          {appearance.native && (
            <section className="capture-settings">
              <h3>采集</h3>
              <label className="setting-row">
                记录新复制
                <input
                  type="checkbox"
                  checked={!!live.status?.policy.enabled}
                  disabled={
                    !live.status ||
                    live.mutating ||
                    (!live.status.available && !live.status.policy.enabled)
                  }
                  onChange={(event) => void saveCapture(event.target.checked)}
                />
              </label>
              <p className="settings-feedback">
                只记录开启后的新复制，不补采已有剪贴板。来源无法可靠获取时标为未知。
              </p>
              <label className="policy-field">
                排除正则（文本与文件路径）
                <input
                  aria-label="排除正则"
                  value={policyPattern}
                  maxLength={4096}
                  onChange={(event) => setPolicyPattern(event.target.value)}
                />
              </label>
              <label className="policy-field">
                最多保留条数
                <input
                  aria-label="最多保留条数"
                  type="number"
                  min={1}
                  max={1000}
                  value={policyMax}
                  onChange={(event) => setPolicyMax(Number(event.target.value))}
                />
              </label>
              <button
                className="secondary-button"
                disabled={
                  !live.status ||
                  live.mutating ||
                  !Number.isInteger(policyMax) ||
                  policyMax < 1 ||
                  policyMax > 1000
                }
                onClick={() => void saveCapture(!!live.status?.policy.enabled, true)}
              >
                保存采集规则
              </button>
              <p className="settings-feedback">
                文本上限 4MiB；图片上限 8M 像素；文件引用最多 256 项。总历史载荷最多
                128MiB，超限不截断。
                {desktop.status?.shortcutAvailable
                  ? ` ${desktop.status.preferences.shortcut} 唤起窗口。`
                  : ''}
              </p>
            </section>
          )}
          {appearance.native && (
            <section className="settings-section">
              <h3>桌面</h3>
              <label className="setting-row">
                关闭窗口后常驻
                <input
                  type="checkbox"
                  checked={!!desktop.status?.preferences.closeToTray}
                  disabled={!desktop.status || desktop.busy}
                  onChange={(event) => void saveDesktop({ closeToTray: event.target.checked })}
                />
              </label>
              <label className="setting-row">
                窗口置顶
                <input
                  type="checkbox"
                  checked={!!desktop.status?.preferences.alwaysOnTop}
                  disabled={!desktop.status || desktop.busy}
                  onChange={(event) => void saveDesktop({ alwaysOnTop: event.target.checked })}
                />
              </label>
              <label className="setting-row">
                失焦自动隐藏
                <input
                  type="checkbox"
                  checked={!!desktop.status?.preferences.autoHide}
                  disabled={!desktop.status || desktop.busy}
                  onChange={(event) => void saveDesktop({ autoHide: event.target.checked })}
                />
              </label>
              <label className="setting-row">
                登录时启动
                <input
                  type="checkbox"
                  checked={!!desktop.status?.autostartEnabled}
                  disabled={
                    !desktop.status || desktop.status.autostartEnabled === null || desktop.busy
                  }
                  onChange={(event) => {
                    void desktop
                      .autostart(event.target.checked)
                      .then(() => setToast({ message: '自启动设置已生效' }))
                      .catch((error) => setToast({ message: safeError(error).message }));
                  }}
                />
              </label>
              <label className="policy-field">
                唤起快捷键
                <input
                  aria-label="唤起快捷键"
                  value={shortcut}
                  maxLength={100}
                  onChange={(event) => setShortcut(event.target.value)}
                />
              </label>
              <button
                className="secondary-button"
                disabled={!desktop.status || desktop.busy}
                onClick={() => void saveDesktop({ shortcut })}
              >
                保存快捷键
              </button>
              <p className="settings-feedback" role="status">
                {desktop.error ||
                  (desktop.status?.shortcutAvailable
                    ? '唤起快捷键可用；Esc 可隐藏窗口。'
                    : '唤起快捷键不可用。')}{' '}
                置顶时不自动隐藏；菜单、设置和预览打开时保留窗口。Linux 无可靠恢复入口时关闭会退出。
              </p>
            </section>
          )}
          {appearance.native && (
            <KeyboardSettings
              snapshot={desktop.status?.keyboard}
              busy={desktop.busy}
              primary={modifier}
              save={desktop.saveKeys}
            />
          )}
          <section className="settings-section">
            <h3>外观</h3>
            <label className="setting-row" htmlFor="theme">
              主题
              <select
                id="theme"
                value={appearance.theme}
                disabled={
                  appearance.saving ||
                  appearance.loading ||
                  (appearance.native && !appearance.snapshot)
                }
                onChange={(event) => void chooseTheme(event.target.value as Theme)}
              >
                <option value="system">跟随系统</option>
                <option value="light">浅色</option>
                <option value="dark">深色</option>
              </select>
              <Icon name="chevron" size={12} />
            </label>
            <p className="settings-feedback" role="status">
              {appearance.saving
                ? '正在保存…'
                : appearance.error ||
                  (appearance.native ? '外观保存在此设备。' : '浏览器只记住外观偏好。')}
            </p>
          </section>
          <details className="about-section">
            <summary>关于与状态</summary>
            <p>UClip · 0.1.0</p>
            <p>
              {appearance.native
                ? '历史保存在此设备。文件仅保存引用；云同步、CLI 和完整管理尚未接入。自动回粘与跨平台验证状态单独报告。'
                : '当前为浏览器界面预览，记录全部为内存示例。'}
            </p>
            {appearance.snapshot?.platform.capabilities.map((item) => (
              <div className="capability-row" key={item.id}>
                <span>{capabilityNames[item.id] ?? item.id}</span>
                <span>{stateNames[item.state]}</span>
              </div>
            ))}
            {appearance.native && (
              <button
                className="secondary-button"
                disabled={appearance.loading || appearance.saving}
                onClick={() => {
                  void appearance.refresh();
                  void live.refresh();
                }}
              >
                刷新服务状态
              </button>
            )}
          </details>
        </Modal>
      )}
      {modal === 'new' && (
        <Modal
          title={appearance.native ? '新建文本' : '新建示例'}
          description={
            appearance.native
              ? '保存在此设备；采集暂停时也可手动添加，排除与容量规则仍生效。'
              : '仅保存在此次预览，刷新后还原。'
          }
          onClose={closeModal}
        >
          <form className="new-form" onSubmit={createClip}>
            {!appearance.native && (
              <>
                <label htmlFor="clip-title">标题（可选）</label>
                <input
                  id="clip-title"
                  data-autofocus
                  value={newTitle}
                  onChange={(event) => setNewTitle(event.target.value)}
                  maxLength={96}
                />
              </>
            )}
            <label htmlFor="clip-content">内容</label>
            <textarea
              id="clip-content"
              value={newContent}
              onChange={(event) => setNewContent(event.target.value)}
              rows={6}
              maxLength={appearance.native ? 4 * 1024 * 1024 : 8000}
              data-autofocus={appearance.native ? true : undefined}
              required
            />
            <div className="form-actions">
              <button className="secondary-button" type="button" onClick={closeModal}>
                取消
              </button>
              <button className="primary-button" disabled={!newContent || live.mutating}>
                {appearance.native ? '保存文本' : '添加示例'}
              </button>
            </div>
          </form>
        </Modal>
      )}
      {modal === 'shortcuts' && (
        <Modal title="快捷键" onClose={closeModal}>
          <div className="shortcut-list">
            <div>
              <span>聚焦搜索</span>
              <kbd>{formatKey(activeKeys.search, modifier)}</kbd>
            </div>
            <div>
              <span>选择记录</span>
              <kbd>
                {formatKey(activeKeys.up, modifier)} / {formatKey(activeKeys.down, modifier)}
              </kbd>
            </div>
            <div>
              <span>复制 / 在回收站恢复</span>
              <kbd>{formatKey(activeKeys.copy, modifier)}</kbd>
            </div>
            <div>
              <span>查看完整内容（列表焦点）</span>
              <kbd>{formatKey(activeKeys.preview, modifier)}</kbd>
            </div>
            <div>
              <span>删除记录（列表焦点）</span>
              <kbd>{formatKey(activeKeys.remove, modifier)}</kbd>
            </div>
            <div>
              <span>关闭当前面板 / 清空搜索</span>
              <kbd>{formatKey(activeKeys.dismiss, modifier)}</kbd>
            </div>
            <div>
              <span>设置</span>
              <kbd>{formatKey(activeKeys.settings, modifier)}</kbd>
            </div>
          </div>
          <p className="settings-feedback">
            以上快捷键仅在当前窗口内生效；
            {appearance.native
              ? '复制快捷键只复制；自动回粘需单独选择，失败不会向任意窗口粘贴。'
              : '浏览器只支持示例文本复制。'}
          </p>
        </Modal>
      )}
    </>
  );
}

function Highlighted({ text, ranges = [] }: { text: string; ranges?: [number, number][] }) {
  const parts: import('react').ReactNode[] = [];
  let cursor = 0;
  for (const [start, end] of ranges) {
    if (start < cursor || end <= start || end > text.length) continue;
    parts.push(text.slice(cursor, start), <mark key={start}>{text.slice(start, end)}</mark>);
    cursor = end;
  }
  parts.push(text.slice(cursor));
  return <>{parts}</>;
}
