import { t, formatHistoryTime } from './i18n';
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
import { WindowTitlebar } from './components/WindowTitlebar';
import { HistoryViewport } from './components/HistoryViewport';
import { DEFAULT_KEYS, matchesKey, formatKey } from './platform/keyboard';
import { useDesktop } from './platform/useDesktop';
import { safeError } from './platform/client';
import { useAppearance } from './platform/useAppearance';
import type { Theme, Language, CollectionItem, TransferMode } from './generated/contracts';

type Toast = { message: string; undo?: () => void };
type Menu = { anchor: HTMLElement; clipId?: string };
type AppView = View | `collection:${string}`;
type AppModal = 'settings' | 'new' | 'shortcuts' | 'deduplicate' | 'collection' | 'transfer';
const viewTitles: Record<View, string> = {
  all: 'all_history',
  pinned: 'pinned',
  trash: 'trash',
  ...collectionNames,
};
const stateNames = {
  available: 'available',
  unverified: 'unverified',
  restricted: 'restricted',
  permission_required: 'permission_required',
  not_implemented: 'not_implemented',
};
const capabilityNames: Record<string, string> = {
  clipboard_capture: 'clipboard_capture',
  clipboard_write: 'system_clipboard_write',
  global_shortcut: 'global_shortcut',
  paste_to_app: 'paste_to_original_app',
  tray: 'system_tray',
  sync: 'cloud_sync',
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
  const [settingsPage, setSettingsPage] = useState('capture');
  const [collectionName, setCollectionName] = useState('');
  const [editingCollection, setEditingCollection] = useState<CollectionItem | null>(null);
  const [collectionError, setCollectionError] = useState('');
  const [transfer, setTransfer] = useState<{ clip: Clip; mode: TransferMode } | null>(null);
  const [transferTarget, setTransferTarget] = useState('');
  const [transferError, setTransferError] = useState('');
  const [dedupRevision, setDedupRevision] = useState('');
  const [dedupError, setDedupError] = useState('');
  const [toast, setToast] = useState<Toast | null>(null);
  const settingsWindow =
    appearance.native && new URLSearchParams(window.location.search).get('window') === 'settings';
  const desktop = useDesktop(appearance.native, !!(modal || menu || preview), !settingsWindow);
  const customChrome =
    appearance.native && !settingsWindow && desktop.status?.preferences.nativeTitlebar === false;
  useEffect(() => {
    document.body.classList.toggle('settings-window', settingsWindow);
    return () => document.body.classList.remove('settings-window');
  }, [settingsWindow]);
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
    settingsWindow,
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
  useEffect(() => {
    if (
      !result.pending &&
      !result.error &&
      result.items.length &&
      !result.items.some((item) => item.id === selectedId)
    ) {
      const first = result.items[0];
      if (first) setSelectedId(first.id);
    }
  }, [result.items, result.pending, result.error, selectedId]);
  const previewBase = preview && result.items.find((clip) => clip.id === preview.id);
  const previewClip =
    previewBase && appearance.native
      ? {
          ...previewBase,
          content:
            live.detail?.id === previewBase.id ? live.detail.text : t('loading_full_content'),
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
        setToast({ message: clip.pinned ? t('unpinned') : t('pinned') });
      } catch (error) {
        setToast({ message: safeError(error).message });
      }
      return;
    }
    changeClip(clip.id, { pinned: !clip.pinned });
    setToast({ message: clip.pinned ? t('sample_unpinned') : t('sample_pinned') });
  }
  async function deduplicatePinned() {
    if (live.mutating) return;
    setDedupError('');
    try {
      const result = await live.deduplicatePinned(dedupRevision);
      closeModal();
      setToast({
        message: result.removed
          ? t('done_deleted_0_unpinned_duplicates', { 0: result.removed })
          : t('no_unpinned_duplicates_to_delete'),
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
      message: clip.deleted ? t('sample_restored') : t('sample_moved_to_trash'),
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
        message: t('deleted'),
        undo: () => {
          void (async () => {
            try {
              await live.changeDeletion({
                id: deleted.id,
                expectedRevision: deleted.revision,
                deleted: false,
              });
              setSelectedId(deleted.id);
              setToast({ message: t('deletion_undone') });
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
  async function copyClip(clip: Clip, paste = false, fallbackToCopy = false) {
    if (!ready || !result.items.some((item) => item.id === clip.id)) return;
    if (clip.deleted) {
      removeClip(clip);
      return;
    }
    if (appearance.native) {
      if (copying.current) return;
      copying.current = true;
      try {
        const outcome = await live.copy(clip.id, paste, fallbackToCopy);
        setToast({
          message: outcome.warning
            ? `${outcome.pasteRequested ? t('copied_and_requested_paste_to_original_app') : t('copied')}；${t(safeError(outcome.warning).message)}`
            : outcome.pasteRequested
              ? t('copied_and_requested_paste_to_original_app')
              : t('copied'),
        });
        if (fallbackToCopy && outcome.pasteRequested) void desktop.hide().catch(() => {});
      } catch (error) {
        setToast({ message: safeError(error).message });
      } finally {
        copying.current = false;
      }
      return;
    }
    if (clip.type === 'image' || clip.type === 'file') {
      setToast({ message: t('copying_0_is_not_available_yet', { 0: t(typeNames[clip.type]) }) });
      return;
    }
    try {
      await navigator.clipboard.writeText(clip.content);
      setToast({ message: t('sample_text_copied') });
    } catch {
      setToast({
        message: t('the_browser_blocked_copying_select_and_copy_text_in_the_full_preview'),
      });
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
      setToast({
        message: enabled
          ? t('capture_rules_applied_only_future_copies_will_be_recorded')
          : t('capture_paused'),
      });
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
    if (settingsWindow) {
      void windowAction(desktop.close);
      return;
    }
    setModal(null);
    requestAnimationFrame(() => {
      const previous = modalFocus.current;
      (previous?.isConnected ? previous : search.current)?.focus({ preventScroll: true });
    });
  }
  function openModal(next: AppModal) {
    if (next === 'settings' && appearance.native && !settingsWindow) {
      setMenu(null);
      void desktop.openSettings().catch((error) => setToast({ message: safeError(error).message }));
      return;
    }
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
      setToast({ message: editingCollection ? t('collection_renamed') : t('collection_created') });
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
            ? t('copied_to_collection')
            : transferTarget
              ? t('moved_to_collection')
              : t('removed_from_collection'),
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
      if (
        settingsWindow ||
        event.isComposing ||
        event.keyCode === 229 ||
        event.defaultPrevented ||
        menu
      )
        return;
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
          setToast({ message: t('text_saved') });
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
      source: t('manually_created'),
      time: formatHistoryTime(Date.now()),
      day: 'today',
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
    setToast({ message: t('sample_added_reloading_will_reset_it') });
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
        'closeToTray' | 'alwaysOnTop' | 'autoHide' | 'shortcut' | 'nativeTitlebar'
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
        nativeTitlebar: p.nativeTitlebar,
        shortcut: p.shortcut,
        expectedRevision: p.revision,
        ...update,
      });
      setToast({ message: t('desktop_settings_saved') });
    } catch (error) {
      setToast({ message: safeError(error).message });
    }
  }
  async function chooseTheme(theme: Theme) {
    if (await appearance.saveTheme(theme))
      setToast({
        message: appearance.native ? t('appearance_saved') : t('preview_appearance_updated'),
      });
  }
  async function chooseLanguage(language: Language) {
    if (await appearance.saveLanguage(language)) setToast(null);
  }
  async function windowAction(action: () => Promise<unknown>) {
    try {
      await action();
    } catch (error) {
      setToast({ message: safeError(error).message });
    }
  }

  const captureLabel = !appearance.native
    ? t('browser_demo_capture_requires_the_desktop_app')
    : !live.status
      ? t('connecting_to_capture_service')
      : live.mutating
        ? t('saving')
        : !live.status.available
          ? live.status.policy.enabled
            ? t('capture_unavailable_click_to_pause')
            : t('capture_service_unavailable')
          : live.status.policy.enabled
            ? t('capturing_click_to_pause')
            : t('paused_click_to_capture_future_copies');

  const searchBox = (
    <div className={`search-box ${result.error ? 'has-error' : ''}`}>
      <Icon name="search" size={18} />
      <label className="sr-only" htmlFor="history-search">
        {t('search_history')}
      </label>
      <input
        ref={search}
        id="history-search"
        value={query}
        placeholder={t('search_history_2')}
        onChange={(event) => setQuery(event.target.value)}
        autoComplete="off"
        spellCheck={false}
        aria-invalid={!!result.error}
        aria-describedby={result.error ? 'search-error' : undefined}
      />
      {query && (
        <button
          className="icon-button"
          aria-label={t('clear_search')}
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
        aria-label={t('regular_expression_search')}
        aria-pressed={regex}
        onClick={() => setRegex(!regex)}
      >
        .*
      </button>
    </div>
  );

  return (
    <>
      {customChrome && (
        <WindowTitlebar
          onDrag={() => void windowAction(desktop.startDrag)}
          onMinimize={() => void windowAction(desktop.minimize)}
          onToggleMaximized={() => void windowAction(desktop.toggleMaximized)}
          onClose={() => void windowAction(desktop.close)}
        />
      )}
      <div
        className="app-shell"
        hidden={settingsWindow}
        inert={modal || settingsWindow ? true : undefined}
        data-testid="app-ready"
      >
        <header className="topbar">
          {searchBox}
          <div className="filter-bar">
            <label className="scope-control">
              <Icon
                name={view === 'trash' ? 'trash' : view === 'pinned' ? 'pin' : 'history'}
                size={15}
              />
              <span className="sr-only">{t('history_scope')}</span>
              <select
                aria-label={t('history_scope')}
                value={view}
                onChange={(event) => setView(event.target.value as AppView)}
              >
                {Object.entries(
                  appearance.native
                    ? {
                        all: t('all_history'),
                        pinned: t('pinned'),
                        ...Object.fromEntries(
                          live.collections.map((item) => [`collection:${item.id}`, item.name]),
                        ),
                        trash: t('trash'),
                      }
                    : viewTitles,
                ).map(([key, label]) => (
                  <option key={key} value={key} disabled={appearance.native && key === 'trash'}>
                    {appearance.native && key.startsWith('collection:') ? label : t(label)}
                    {appearance.native && key === 'trash' ? t('not_implemented_2') : ''}
                  </option>
                ))}
              </select>
              <Icon name="chevron" size={12} />
            </label>
            <label className="type-control">
              <span className="sr-only">{t('content_type')}</span>
              <select
                aria-label={t('content_type')}
                value={type}
                onChange={(event) => setType(event.target.value as ClipType | 'all')}
              >
                <option value="all">{t('all_types')}</option>
                {Object.entries(typeNames).map(([key, label]) => (
                  <option key={key} value={key} disabled={appearance.native && key === 'link'}>
                    {t(label)}
                  </option>
                ))}
              </select>
              <Icon name="chevron" size={12} />
            </label>
            {(view !== 'all' || type !== 'all' || regex) && (
              <button className="text-button clear-filters" onClick={clearFilters}>
                {t('reset')}
              </button>
            )}
          </div>
        </header>
        {(appearance.error || live.statusError || live.status?.error) && (
          <div className="service-error" role="alert" data-testid="service-error">
            <Icon name="info" size={16} />
            <span>
              {t('local_service_status')}{' '}
              {t(
                appearance.error ||
                  live.statusError ||
                  (live.status?.error ? safeError(live.status.error).message : ''),
              )}
            </span>
            <button
              className="text-button"
              onClick={() => {
                void appearance.refresh();
                void live.refresh();
              }}
            >
              {t('refresh_status')}
            </button>
          </div>
        )}
        <main className="content-workspace">
          <section
            className="history-panel"
            hidden={!!previewClip}
            aria-label={t('clipboard_history')}
          >
            <HistoryViewport
              listRef={list}
              resetKey={JSON.stringify([query, regex, type, view, live.offset, oldest, grid])}
              className={`history-scroll ${grid ? 'clip-grid' : ''}`}
              pending={result.pending}
            >
              {result.pending && !result.items.length ? (
                <div className="empty-state" role="status">
                  <Icon name="search" size={24} />
                  <strong>{t('searching')}</strong>
                </div>
              ) : result.error ? (
                <div className="empty-state" role="alert" id="search-error">
                  <Icon name="info" size={24} />
                  <strong>{t('search_incomplete')}</strong>
                  <p>{t(result.error)}</p>
                  {regex && (
                    <button className="secondary-button" onClick={() => setRegex(false)}>
                      {t('use_plain_search')}
                    </button>
                  )}
                  {appearance.native && (
                    <button className="secondary-button" onClick={live.retryQuery}>
                      {t('retry_search')}
                    </button>
                  )}
                </div>
              ) : !result.items.length ? (
                <div className="empty-state">
                  <Icon name={view === 'trash' ? 'trash' : 'search'} size={26} />
                  <strong>
                    {view === 'trash' && !query && type === 'all'
                      ? t('trash_is_empty')
                      : query || type !== 'all'
                        ? t('no_matching_records')
                        : t('no_history_yet')}
                  </strong>
                  {(query || view !== 'all' || type !== 'all') && (
                    <button className="text-button" onClick={clearFilters}>
                      {t('clear_filters')}
                    </button>
                  )}
                </div>
              ) : (
                result.items.map((clip) => (
                  <article
                    key={clip.id}
                    data-clip-id={clip.id}
                    data-clip-type={clip.type}
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
                      aria-label={`${t(typeNames[clip.type])}：${clip.type === 'text' ? clip.content.slice(0, 60) : clip.title}`}
                      onFocus={() => setSelectedId(clip.id)}
                      onClick={() => setSelectedId(clip.id)}
                      onDoubleClick={() => void copyClip(clip, true, true)}
                    >
                      {clip.type === 'image' ? (
                        clip.image ? (
                          <img
                            className="row-thumbnail"
                            src={clip.image}
                            width={104}
                            height={76}
                            loading="lazy"
                            decoding="async"
                            alt=""
                          />
                        ) : (
                          <span
                            className="row-thumbnail row-thumbnail-placeholder"
                            aria-hidden="true"
                          >
                            <Icon name="image" size={24} />
                          </span>
                        )
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
                                  ?.name ?? t('collection_unavailable')}
                              </span>
                            </>
                          )}
                          {clip.pinned && (
                            <>
                              <Icon name="pin" size={12} />
                              <span className="sr-only">{t('pinned')}</span>
                            </>
                          )}
                        </span>
                      </span>
                    </button>
                    <div className="row-actions">
                      <button
                        className="icon-button row-more"
                        aria-label={t('more_actions_0', {
                          0: clip.title || clip.content.slice(0, 20),
                        })}
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
            </HistoryViewport>
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
                    t('collection_unavailable'))
                  : undefined
              }
              error={appearance.native ? t(live.detailError) : undefined}
              onMore={(anchor) => openMenu(anchor, previewClip)}
            />
          )}
        </main>
        <footer className="statusbar" aria-label={t('bottom_toolbar')}>
          <button
            className={`capture-toggle ${live.status?.policy.enabled && live.status.available ? 'active' : ''} ${live.status && !live.status.available ? 'unavailable' : ''}`}
            disabled={
              !appearance.native ||
              !live.status ||
              live.mutating ||
              (!live.status.available && !live.status.policy.enabled)
            }
            aria-pressed={appearance.native ? !!live.status?.policy.enabled : undefined}
            aria-label={`${result.pending ? t('searching_2') : t('0_records', { 0: appearance.native ? live.total : result.items.length })} · ${captureLabel}`}
            title={captureLabel}
            onClick={() => void saveCapture(!live.status?.policy.enabled)}
          >
            <Icon name="copy" size={16} />
            <span aria-live="polite">
              {result.pending ? '…' : appearance.native ? live.total : result.items.length}
            </span>
          </button>
          {appearance.native && live.total > 25 && (
            <div className="pagination">
              <button
                className="text-button"
                disabled={live.pending || !live.offset}
                onClick={() => live.setOffset(Math.max(0, live.offset - 25))}
              >
                {t('previous_page')}
              </button>
              <span>{Math.floor(live.offset / 25) + 1}</span>
              <button
                className="text-button"
                disabled={live.pending || live.offset + 25 >= live.total}
                onClick={() => live.setOffset(live.offset + 25)}
              >
                {t('next_page')}
              </button>
            </div>
          )}
          <div className="footer-tools">
            <button
              className="icon-button"
              aria-label={t('settings')}
              title={t('settings_0', { 0: formatKey(activeKeys.settings, modifier) })}
              data-testid="footer-settings"
              onClick={() => openModal('settings')}
            >
              <Icon name="settings" size={17} />
            </button>
            <button
              className="icon-button"
              aria-label={appearance.dark ? t('switch_to_light_theme') : t('switch_to_dark_theme')}
              title={appearance.dark ? t('switch_to_light_theme') : t('switch_to_dark_theme')}
              disabled={
                appearance.loading ||
                appearance.saving ||
                (appearance.native && !appearance.snapshot)
              }
              onClick={() => void chooseTheme(appearance.dark ? 'light' : 'dark')}
            >
              <Icon name={appearance.dark ? 'moon' : 'sun'} size={17} />
            </button>
            <button
              className="icon-button"
              aria-label={t('always_on_top')}
              title={
                appearance.native
                  ? desktop.status?.preferences.alwaysOnTop
                    ? t('window_fixed_click_to_hide_on_blur')
                    : desktop.status?.preferences.autoHide
                      ? t('hide_on_blur_click_to_keep_visible')
                      : t('click_to_keep_the_window_visible')
                  : t('always_on_top_requires_the_desktop_app')
              }
              aria-pressed={!!desktop.status?.preferences.alwaysOnTop}
              disabled={!appearance.native || !desktop.status || desktop.busy}
              onClick={() =>
                void saveDesktop({
                  alwaysOnTop: !desktop.status?.preferences.alwaysOnTop,
                  ...(desktop.status?.preferences.alwaysOnTop ? { autoHide: true } : {}),
                })
              }
            >
              <Icon name="pin" size={17} />
            </button>
            <button
              className="icon-button app-more"
              aria-label={t('more_options')}
              aria-haspopup="menu"
              aria-expanded={!!menu && !menu.clipId}
              onClick={(event) => openMenu(event.currentTarget)}
            >
              <Icon name="more" size={20} />
            </button>
          </div>
        </footer>
      </div>
      {menu && (
        <ActionMenu anchor={menu.anchor} onClose={() => setMenu(null)}>
          {menuClip ? (
            <>
              {appearance.native && !menuClip.deleted && (
                <button
                  role="menuitem"
                  disabled={!live.status?.pasteAvailable || !live.status.pasteTarget}
                  onClick={() => menuAction(() => void copyClip(menuClip, true))}
                >
                  <Icon name="arrow" size={15} />
                  {t('paste_to_original_app_2')}
                </button>
              )}
              {!menuClip.deleted && (
                <button
                  role="menuitem"
                  data-testid="copy-history-item"
                  onClick={() => menuAction(() => void copyClip(menuClip))}
                >
                  <Icon name="copy" size={15} />
                  {t('copy')}
                </button>
              )}
              <button role="menuitem" onClick={() => menuAction(() => openPreview(menuClip))}>
                <Icon name="text" size={15} />
                {t('view_full_content')}
              </button>
              <button
                role="menuitem"
                data-testid="toggle-pin"
                disabled={!ready || live.mutating || menuClip.deleted}
                onClick={() => menuAction(() => void togglePin(menuClip))}
              >
                <Icon name="pin" size={15} />
                {menuClip.pinned ? t('unpin') : t('pin')}
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
                    {t('move_to_collection')}
                  </button>
                  <button
                    role="menuitem"
                    disabled={!ready || live.mutating || !live.collections.length}
                    data-testid="copy-to-collection"
                    onClick={() => menuAction(() => openTransfer(menuClip, 'copy'))}
                  >
                    <Icon name="copy" size={15} />
                    {t('copy_to_collection')}
                  </button>
                </>
              )}
              {!appearance.native && !menuClip.deleted && (
                <>
                  <div className="menu-separator" />
                  <p className="menu-label">{t('move_to_collection_2')}</p>
                  {(Object.keys(collectionNames) as Collection[]).map((key) => (
                    <button
                      role="menuitem"
                      key={key}
                      onClick={() =>
                        menuAction(() => {
                          changeClip(menuClip.id, { collection: key });
                          setToast({
                            message: t('sample_moved_to_0', { 0: t(collectionNames[key]) }),
                          });
                        })
                      }
                    >
                      <Icon name="folder" size={15} />
                      {t(collectionNames[key])}
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
                {menuClip.deleted
                  ? t('restore')
                  : appearance.native
                    ? t('delete')
                    : t('move_to_trash')}
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
                {appearance.native ? t('new_text') : t('new_sample')}
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
                    {t('new_collection')}
                  </button>
                  {currentCollection && (
                    <button
                      role="menuitem"
                      data-testid="rename-collection"
                      disabled={live.mutating}
                      onClick={() => menuAction(() => openCollectionEditor(currentCollection))}
                    >
                      <Icon name="folder" size={15} />
                      {t('rename_current_collection')}
                    </button>
                  )}
                </>
              )}
              {appearance.native && view === 'pinned' && (
                <button
                  role="menuitem"
                  disabled={!ready || live.mutating || !live.revision}
                  onClick={() => {
                    setMenu(null);
                    setDedupRevision(live.revision!);
                    setDedupError('');
                    openModal('deduplicate');
                  }}
                  aria-label={t('deduplicate_pinned_history')}
                >
                  {t('deduplicate_pinned_history')}
                </button>
              )}
              <button role="menuitem" onClick={() => menuAction(() => setGrid(!grid))}>
                <Icon name={grid ? 'list' : 'grid'} size={15} />
                {grid ? t('switch_to_list_view') : t('switch_to_grid_view')}
              </button>
              <button
                role="menuitem"
                disabled={appearance.native}
                onClick={() => menuAction(() => setOldest(!oldest))}
              >
                <Icon name="sort" size={15} />
                {oldest ? t('show_newest_first') : t('show_oldest_first')}
              </button>
              <div className="menu-separator" />
              <button
                role="menuitem"
                data-testid="open-settings"
                aria-label={t('settings')}
                onClick={() => menuAction(() => openModal('settings'))}
              >
                <Icon name="settings" size={15} />
                {t('settings')}
                <span className="menu-shortcut">{formatKey(activeKeys.settings, modifier)}</span>
              </button>
              <button role="menuitem" onClick={() => menuAction(() => openModal('shortcuts'))}>
                <Icon name="keyboard" size={15} />
                {t('keyboard_shortcuts')}
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
                  {t('quit_app')}
                </button>
              )}
            </>
          )}
        </ActionMenu>
      )}
      {toast && (
        <div className="toast" role="status">
          <span>{t(toast.message)}</span>
          {toast.undo && (
            <button
              className="text-button"
              onClick={() => {
                toast.undo?.();
                setToast(null);
              }}
            >
              {t('undo')}
            </button>
          )}
          <button
            className="icon-button"
            aria-label={t('dismiss_notification')}
            onClick={() => setToast(null)}
          >
            <Icon name="close" size={14} />
          </button>
        </div>
      )}
      {modal === 'collection' && (
        <Modal
          title={editingCollection ? t('rename_collection') : t('new_collection')}
          onClose={() => {
            if (!live.mutating) closeModal();
          }}
          description={t('names_can_contain_up_to_64_characters_collection_names_must_be_unique')}
        >
          <form className="new-form" onSubmit={saveCollection}>
            <label htmlFor="collection-name">{t('collection_name')}</label>
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
                {t(collectionError)}
              </p>
            )}
            <div className="form-actions">
              <button
                type="button"
                className="secondary-button"
                disabled={live.mutating}
                onClick={closeModal}
              >
                {t('cancel')}
              </button>
              <button
                className="primary-button"
                disabled={
                  live.mutating ||
                  !collectionName.trim() ||
                  collectionName.trim() === editingCollection?.name
                }
              >
                {t('save_collection')}
              </button>
            </div>
          </form>
        </Modal>
      )}
      {modal === 'transfer' && transfer && (
        <Modal
          title={transfer.mode === 'copy' ? t('copy_to_collection_2') : t('move_to_collection_3')}
          onClose={() => {
            if (!live.mutating) closeModal();
          }}
          description={
            transfer.mode === 'copy'
              ? t(
                  'keep_the_original_and_create_an_independent_unpinned_copy_in_the_target_collection',
                )
              : t('keep_the_record_identity_and_pin_status_and_change_its_collection')
          }
        >
          <form className="new-form" onSubmit={saveTransfer}>
            <label htmlFor="transfer-collection">{t('target_collection')}</label>
            <select
              id="transfer-collection"
              data-autofocus
              value={transferTarget}
              onChange={(event) => setTransferTarget(event.target.value)}
            >
              {transfer.mode === 'move' && <option value="">{t('uncategorized')}</option>}
              {transfer.mode === 'copy' && !transferTarget && (
                <option value="" disabled>
                  {t('select_collection')}
                </option>
              )}
              {live.collections.map((item) => (
                <option key={item.id} value={item.id}>
                  {item.name}
                </option>
              ))}
            </select>
            {!live.collections.length && (
              <p className="settings-feedback">
                {t('create_a_collection_first_using_more_options')}
              </p>
            )}
            {transferError && (
              <p role="alert" className="settings-feedback">
                {t(transferError)}
              </p>
            )}
            <div className="form-actions">
              <button
                type="button"
                className="secondary-button"
                disabled={live.mutating}
                onClick={closeModal}
              >
                {t('cancel')}
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
                {transfer.mode === 'copy' ? t('copy_record') : t('move_record')}
              </button>
            </div>
          </form>
        </Modal>
      )}
      {modal === 'deduplicate' && (
        <Modal
          title={t('deduplicate_pinned_history')}
          onClose={() => {
            if (!live.mutating) closeModal();
          }}
          description={t(
            'delete_unpinned_records_whose_content_exactly_matches_any_pinned_record_across_all_history_all_pinned_records_are_kept_search_type_filters_and_pagination_do_not_limit_the_scope',
          )}
        >
          {dedupError && (
            <p role="alert" className="settings-feedback">
              {t(dedupError)}
              {t('close_this_panel_refresh_and_check_the_scope_again')}
            </p>
          )}
          <div className="form-actions">
            <button
              className="secondary-button"
              data-autofocus
              disabled={live.mutating}
              onClick={closeModal}
            >
              {t('cancel')}
            </button>
            <button
              className="primary-button"
              disabled={live.mutating || !!dedupError}
              onClick={() => void deduplicatePinned()}
            >
              {live.mutating ? t('deduplicating') : t('delete_unpinned_duplicates')}
            </button>
          </div>
        </Modal>
      )}
      {(modal === 'settings' || settingsWindow) && (
        <Modal title={t('settings')} onClose={closeModal}>
          {settingsWindow && (appearance.error || live.statusError || live.status?.error) && (
            <p className="settings-feedback" role="alert">
              {t(
                appearance.error ||
                  live.statusError ||
                  (live.status?.error ? safeError(live.status.error).message : ''),
              )}
              <button
                className="text-button"
                onClick={() => {
                  void appearance.refresh();
                  void live.refresh();
                }}
              >
                {t('refresh_status')}
              </button>
            </p>
          )}
          <div className={settingsWindow ? 'settings-layout' : undefined}>
            {settingsWindow && (
              <nav className="settings-nav" aria-label={t('settings_categories')}>
                {(
                  [
                    ['capture', t('capture'), 'history'],
                    ['desktop', t('desktop'), 'monitor'],
                    ['keyboard', t('shortcuts'), 'keyboard'],
                    ['appearance', t('appearance'), 'sun'],
                    ['about', t('about_and_status'), 'info'],
                  ] as const
                ).map(([id, label, icon]) => (
                  <button
                    key={id}
                    data-settings-page={id}
                    aria-current={settingsPage === id ? 'page' : undefined}
                    onClick={() => setSettingsPage(id)}
                  >
                    <Icon name={icon} size={17} />
                    <span>{label}</span>
                  </button>
                ))}
              </nav>
            )}
            <div className={settingsWindow ? 'settings-content' : undefined}>
              {appearance.native && (
                <section
                  className="capture-settings"
                  hidden={settingsWindow && settingsPage !== 'capture'}
                >
                  <h3>{t('capture')}</h3>
                  <label className="setting-row">
                    {t('record_future_copies')}
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
                    {t(
                      'only_copies_made_after_enabling_capture_are_recorded_existing_clipboard_content_is_not_imported_unreliable_sources_are_marked_unknown',
                    )}
                  </p>
                  <label className="policy-field">
                    {t('exclude_regex_text_and_file_paths')}
                    <input
                      aria-label={t('exclude_regex')}
                      value={policyPattern}
                      maxLength={4096}
                      onChange={(event) => setPolicyPattern(event.target.value)}
                    />
                  </label>
                  <label className="policy-field">
                    {t('maximum_records')}
                    <input
                      aria-label={t('maximum_records')}
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
                    {t('save_capture_rules')}
                  </button>
                  <p className="settings-feedback">
                    {t(
                      'limits_4_mib_of_text_8_million_image_pixels_256_file_references_and_128_mib_of_total_history_payload_oversized_content_is_rejected_without_truncation',
                    )}
                    {desktop.status?.shortcutAvailable
                      ? t('0_to_show_the_window', { 0: desktop.status.preferences.shortcut })
                      : ''}
                  </p>
                </section>
              )}
              {appearance.native && (
                <section
                  className="settings-section"
                  hidden={settingsWindow && settingsPage !== 'desktop'}
                >
                  <h3>{t('desktop')}</h3>
                  <label className="setting-row">
                    {t('keep_running_after_closing')}
                    <input
                      type="checkbox"
                      checked={!!desktop.status?.preferences.closeToTray}
                      disabled={!desktop.status || desktop.busy}
                      onChange={(event) => void saveDesktop({ closeToTray: event.target.checked })}
                    />
                  </label>
                  <label className="setting-row">
                    {t('always_on_top')}
                    <input
                      type="checkbox"
                      checked={!!desktop.status?.preferences.alwaysOnTop}
                      disabled={!desktop.status || desktop.busy}
                      onChange={(event) => void saveDesktop({ alwaysOnTop: event.target.checked })}
                    />
                  </label>
                  <label className="setting-row">
                    {t('hide_on_blur')}
                    <input
                      type="checkbox"
                      checked={!!desktop.status?.preferences.autoHide}
                      disabled={!desktop.status || desktop.busy}
                      onChange={(event) => void saveDesktop({ autoHide: event.target.checked })}
                    />
                  </label>
                  <label className="setting-row">
                    {t('launch_at_login')}
                    <input
                      type="checkbox"
                      checked={!!desktop.status?.autostartEnabled}
                      disabled={
                        !desktop.status || desktop.status.autostartEnabled === null || desktop.busy
                      }
                      onChange={(event) => {
                        void desktop
                          .autostart(event.target.checked)
                          .then(() => setToast({ message: t('autostart_setting_applied') }))
                          .catch((error) => setToast({ message: safeError(error).message }));
                      }}
                    />
                  </label>
                  <label className="policy-field">
                    {t('show_window_shortcut')}
                    <input
                      aria-label={t('show_window_shortcut')}
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
                    {t('save_shortcut')}
                  </button>
                  <p className="settings-feedback" role="status">
                    {t(
                      desktop.error ||
                        (desktop.status?.shortcutAvailable
                          ? t('show_window_shortcut_available_esc_hides_the_window')
                          : t('the_show_window_shortcut_is_unavailable')),
                    )}{' '}
                    {t(
                      'always_on_top_prevents_hiding_on_blur_menus_settings_and_previews_keep_the_window_open_on_linux_closing_quits_if_there_is_no_reliable_way_to_restore_the_window',
                    )}
                  </p>
                </section>
              )}
              {appearance.native && (
                <div hidden={settingsWindow && settingsPage !== 'keyboard'}>
                  <KeyboardSettings
                    expanded={settingsWindow}
                    snapshot={desktop.status?.keyboard}
                    busy={desktop.busy}
                    primary={modifier}
                    save={desktop.saveKeys}
                  />
                </div>
              )}
              <section
                className="settings-section"
                hidden={settingsWindow && settingsPage !== 'appearance'}
              >
                <h3>{t('appearance')}</h3>
                {appearance.native && (
                  <>
                    <label className="setting-row">
                      {t('use_system_title_bar')}
                      <input
                        type="checkbox"
                        aria-label={t('use_system_title_bar')}
                        checked={desktop.status?.preferences.nativeTitlebar ?? false}
                        disabled={!desktop.status || desktop.busy}
                        onChange={(event) =>
                          void saveDesktop({ nativeTitlebar: event.target.checked })
                        }
                      />
                    </label>
                    <p className="settings-feedback">
                      {t(
                        'turn_off_to_use_the_custom_title_bar_with_dragging_minimize_maximize_and_close_controls_applies_immediately_and_persists_after_restart',
                      )}
                    </p>
                  </>
                )}
                <label className="setting-row" htmlFor="language">
                  {t('language')}
                  <select
                    id="language"
                    value={appearance.language}
                    disabled={
                      appearance.saving ||
                      appearance.loading ||
                      (appearance.native && !appearance.snapshot)
                    }
                    onChange={(event) => void chooseLanguage(event.target.value as Language)}
                  >
                    <option value="system">{t('follow_system')}</option>
                    <option value="zh-CN" lang="zh-CN">
                      {t('simplified_chinese')}
                    </option>
                    <option value="en" lang="en">
                      English
                    </option>
                  </select>
                </label>
                <label className="setting-row" htmlFor="theme">
                  {t('theme')}
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
                    <option value="system">{t('follow_system')}</option>
                    <option value="light">{t('light')}</option>
                    <option value="dark">{t('dark')}</option>
                  </select>
                </label>
                <p className="settings-feedback" role="status">
                  {appearance.saving
                    ? t('saving')
                    : t(appearance.error) ||
                      (appearance.native
                        ? t('language_and_appearance_preferences_are_saved_on_this_device')
                        : t('the_browser_remembers_language_and_appearance_preferences'))}
                </p>
              </section>
              <details
                className="about-section"
                open={settingsWindow || undefined}
                hidden={settingsWindow && settingsPage !== 'about'}
              >
                <summary>{t('about_and_status')}</summary>
                <p>UClip · 0.1.0</p>
                <p>
                  {appearance.native
                    ? t(
                        'history_stays_on_this_device_files_are_references_only_cloud_sync_cli_and_full_management_are_not_available_yet_automatic_paste_and_platform_verification_are_reported_separately',
                      )
                    : t('this_is_a_browser_preview_all_records_are_in_memory_samples')}
                </p>
                {appearance.snapshot?.platform.capabilities.map((item) => (
                  <div className="capability-row" key={item.id}>
                    <span>{t(capabilityNames[item.id] ?? item.id)}</span>
                    <span>{t(stateNames[item.state])}</span>
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
                    {t('refresh_service_status')}
                  </button>
                )}
              </details>
            </div>
          </div>
        </Modal>
      )}
      {modal === 'new' && (
        <Modal
          title={appearance.native ? t('new_text') : t('new_sample')}
          description={
            appearance.native
              ? t(
                  'saved_on_this_device_even_while_capture_is_paused_exclusion_and_capacity_rules_still_apply',
                )
              : t('saved_only_for_this_preview_reloading_resets_it')
          }
          onClose={closeModal}
        >
          <form className="new-form" onSubmit={createClip}>
            {!appearance.native && (
              <>
                <label htmlFor="clip-title">{t('title_optional')}</label>
                <input
                  id="clip-title"
                  data-autofocus
                  value={newTitle}
                  onChange={(event) => setNewTitle(event.target.value)}
                  maxLength={96}
                />
              </>
            )}
            <label htmlFor="clip-content">{t('content')}</label>
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
                {t('cancel')}
              </button>
              <button className="primary-button" disabled={!newContent || live.mutating}>
                {appearance.native ? t('save_text') : t('add_sample')}
              </button>
            </div>
          </form>
        </Modal>
      )}
      {modal === 'shortcuts' && (
        <Modal title={t('shortcuts')} onClose={closeModal}>
          <div className="shortcut-list">
            <div>
              <span>{t('focus_search')}</span>
              <kbd>{formatKey(activeKeys.search, modifier)}</kbd>
            </div>
            <div>
              <span>{t('select_record')}</span>
              <kbd>
                {formatKey(activeKeys.up, modifier)} / {formatKey(activeKeys.down, modifier)}
              </kbd>
            </div>
            <div>
              <span>{t('copy_restore_from_trash')}</span>
              <kbd>{formatKey(activeKeys.copy, modifier)}</kbd>
            </div>
            <div>
              <span>{t('view_full_content_list_focus')}</span>
              <kbd>{formatKey(activeKeys.preview, modifier)}</kbd>
            </div>
            <div>
              <span>{t('delete_record_list_focus')}</span>
              <kbd>{formatKey(activeKeys.remove, modifier)}</kbd>
            </div>
            <div>
              <span>{t('close_panel_clear_search')}</span>
              <kbd>{formatKey(activeKeys.dismiss, modifier)}</kbd>
            </div>
            <div>
              <span>{t('settings')}</span>
              <kbd>{formatKey(activeKeys.settings, modifier)}</kbd>
            </div>
          </div>
          <p className="settings-feedback">
            {t('these_shortcuts_apply_only_in_the_current_window')}
            {appearance.native
              ? t('double_click_pastes_at_the_original_cursor')
              : t('the_browser_can_only_copy_sample_text')}
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
