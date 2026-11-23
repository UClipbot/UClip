import { t, formatHistoryTime } from './i18n';
import { selectionRange } from './platform/selection';
import { useEffect, useRef, useState, type CSSProperties, type FormEvent } from 'react';
import { WindowResizeGrip } from './components/WindowResizeGrip';
import { GlobalShortcutSettings } from './components/GlobalShortcutSettings';
import { onQuickPasteUnavailable } from './platform/quickPaste';
import { Icon } from './components/Icon';
import { Modal } from './components/Modal';
import { ActionMenu } from './components/ActionMenu';
import { FilterMenu } from './components/FilterMenu';
import { ClipPreview } from './components/ClipPreview';
import { ClipEditor } from './components/ClipEditor';
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
import { SyncSettings } from './components/SyncSettings';
import { WindowTitlebar } from './components/WindowTitlebar';
import { HistoryViewport } from './components/HistoryViewport';
import { HoverPreview } from './components/HoverPreview';
import {
  DEFAULT_KEYS,
  matchesKey,
  formatKey,
  type HistoryKeyAction,
  type HistoryKeymap,
} from './platform/keyboard';
import { useDesktop } from './platform/useDesktop';
import { useQuickPastePreferences } from './platform/useQuickPastePreferences';
import { safeError } from './platform/client';
import { useAppearance } from './platform/useAppearance';
import type { Theme, Language, CollectionItem, TransferMode } from './generated/contracts';

type Toast = { message: string; undo?: () => void };
type Menu = { anchor: HTMLElement; clipId?: string };
type AppView = View | `collection:${string}`;
type AppModal =
  | 'settings'
  | 'new'
  | 'shortcuts'
  | 'deduplicate'
  | 'collection'
  | 'transfer'
  | 'delete-collection'
  | 'edit';
type SelectionEntry = { clip: Clip; index: number };
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
  sync: 'sync_settings',
};

export function App({ historyKeymap }: { historyKeymap?: HistoryKeymap }) {
  const appearance = useAppearance();
  const [clips, setClips] = useState<Clip[]>(() => initialClips.map((clip) => ({ ...clip })));
  const [view, setView] = useState<AppView>('all');
  const [type, setType] = useState<ClipType | 'all'>('all');
  const [searchQuery, setQuery] = useState('');
  const [searchOverride, setSearchOverride] = useState<boolean | null>(null);
  const searchVisible = searchOverride ?? appearance.showSearchByDefault;
  const query = searchVisible ? searchQuery : '';
  const filtersActive = view !== 'all' || type !== 'all';
  const searchToggleLabel = t(filtersActive ? 'toggle_search_filtered' : 'toggle_search');
  const [regex, setRegex] = useState(false);
  const [oldest, setOldest] = useState(false);
  const [grid, setGrid] = useState(false);
  const [collectionsOpen, setCollectionsOpen] = useState(false);
  const [collectionWidth, setCollectionWidth] = useState(168);
  const [selectedId, setSelectedId] = useState('note-1');
  const [selection, setSelection] = useState<{
    scope: string;
    query: string;
    entries: SelectionEntry[];
  }>({ scope: '', query: '', entries: [] });
  const anchorIndex = useRef<number | null>(null);
  const currentSelectionScope = useRef('');
  const [batchBusy, setBatchBusy] = useState(false);
  const batchLock = useRef(false);
  const [preview, setPreview] = useState<{ id: string; focus: HTMLElement | null } | null>(null);
  const [menu, setMenu] = useState<Menu | null>(null);
  const [modal, setModal] = useState<AppModal | null>(null);
  const [settingsPage, setSettingsPage] = useState('capture');
  const [collectionName, setCollectionName] = useState('');
  const [editingCollection, setEditingCollection] = useState<CollectionItem | null>(null);
  const [collectionError, setCollectionError] = useState('');
  const [transfer, setTransfer] = useState<{
    clips: Clip[];
    mode: TransferMode;
    completed: number;
  } | null>(null);
  const [editingClip, setEditingClip] = useState<Clip | null>(null);
  const [transferTarget, setTransferTarget] = useState('');
  const [transferError, setTransferError] = useState('');
  const [transferUnknown, setTransferUnknown] = useState(false);
  const [dedupRevision, setDedupRevision] = useState('');
  const [dedupError, setDedupError] = useState('');
  const [toast, setToast] = useState<Toast | null>(null);
  const settingsWindow =
    appearance.native && new URLSearchParams(window.location.search).get('window') === 'settings';
  const desktop = useDesktop(appearance.native, !!(modal || menu || preview), !settingsWindow);
  const quickPaste = useQuickPastePreferences(settingsWindow);
  const customChrome =
    appearance.native && !settingsWindow && desktop.status?.preferences.nativeTitlebar === false;
  useEffect(() => {
    document.body.classList.toggle('settings-window', settingsWindow);
    return () => document.body.classList.remove('settings-window');
  }, [settingsWindow]);
  useEffect(() => {
    if (!appearance.native) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void onQuickPasteUnavailable((error) =>
      setToast({
        message:
          error.code === 'restricted' || error.code === 'unavailable'
            ? t('quick_paste_unavailable')
            : safeError(error).message,
      }),
    )
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch(() => {});
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [appearance.native]);
  const copying = useRef(false);
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
    view === 'trash',
    preview?.id,
    settingsWindow,
  );
  const currentCollection = live.collections.find((item) => item.id === nativeCollectionId);
  const scopeOptions = appearance.native
    ? [
        { value: 'all', label: t('all_history') },
        { value: 'pinned', label: t('pinned') },
        ...live.collections.map((item) => ({ value: `collection:${item.id}`, label: item.name })),
        { value: 'trash', label: t('trash') },
      ]
    : Object.entries(viewTitles).map(([value, label]) => ({ value, label: t(label) }));
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
  const selectionQuery = JSON.stringify([query, regex, type, view, oldest]);
  const selectionScope = JSON.stringify([selectionQuery, live.revision]);
  currentSelectionScope.current = selectionScope;
  const replaceSelection = (entries: SelectionEntry[]) =>
    setSelection({ scope: selectionScope, query: selectionQuery, entries });
  useEffect(() => {
    if (selection.scope === selectionScope) return;
    const sameQuery = selection.query === selectionQuery;
    if (sameQuery && result.pending) return;
    const previous =
      sameQuery && !result.error && selection.entries.length === 1
        ? selection.entries[0]?.clip
        : undefined;
    // Refresh the position of a single unchanged row after a background history update.
    const index = previous
      ? result.items.findIndex(
          (clip) => clip.id === previous.id && clip.revision === previous.revision,
        )
      : -1;
    anchorIndex.current = index < 0 ? null : live.offset + index;
    replaceSelection(index < 0 ? [] : [{ clip: result.items[index]!, index: live.offset + index }]);
  }, [
    selection,
    selectionQuery,
    selectionScope,
    result.items,
    result.pending,
    result.error,
    live.offset,
  ]);
  const selectionEntries = selection.scope === selectionScope ? selection.entries : [];
  const selectedClips = appearance.native
    ? selectionEntries.map(({ clip }) => clip)
    : result.items.filter((clip) => selectionEntries.some((entry) => entry.clip.id === clip.id));
  function selectClip(id: string, extend = false, toggle = false) {
    if (!ready || batchLock.current) return;
    const localIndex = result.items.findIndex((clip) => clip.id === id);
    if (localIndex === -1) return;
    const index = live.offset + localIndex;
    setSelectedId(id);
    if (extend && anchorIndex.current !== null) {
      const { start, end } = selectionRange(anchorIndex.current, index);
      const applyRange = (clips: Clip[]) =>
        replaceSelection(clips.map((clip, offset) => ({ clip, index: start + offset })));
      if (!appearance.native || (start >= live.offset && end < live.offset + result.items.length)) {
        applyRange(result.items.slice(start - live.offset, end - live.offset + 1));
      } else {
        batchLock.current = true;
        setBatchBusy(true);
        void live
          .readRange(start, end - start + 1)
          .then((clips) => {
            if (
              currentSelectionScope.current === selectionScope &&
              clips.length === end - start + 1
            )
              applyRange(clips);
          })
          .catch((error) => {
            if (currentSelectionScope.current === selectionScope)
              setToast({ message: safeError(error).message });
          })
          .finally(() => {
            batchLock.current = false;
            setBatchBusy(false);
          });
      }
      return;
    }
    anchorIndex.current = index;
    const entry = { clip: result.items[localIndex]!, index };
    const withoutCurrent = selectionEntries.filter(({ clip }) => clip.id !== id);
    const entries = toggle
      ? withoutCurrent.length === selectionEntries.length
        ? [...withoutCurrent, entry]
        : withoutCurrent
      : [entry];
    replaceSelection(entries.sort((a, b) => a.index - b.index));
  }
  function clearSelection() {
    if (batchLock.current) return;
    replaceSelection([]);
    anchorIndex.current = null;
  }
  async function removeSelected() {
    if (!ready || batchLock.current || copying.current || live.mutating || !selectedClips.length)
      return;
    if (selectedClips.length === 1) {
      removeClip(selectedClips[0]!);
      return;
    }
    const targets = [...selectedClips];
    batchLock.current = true;
    setBatchBusy(true);
    try {
      if (!appearance.native) {
        const ids = new Set(targets.map((clip) => clip.id));
        setClips((current) =>
          current.map((clip) => (ids.has(clip.id) ? { ...clip, deleted: !clip.deleted } : clip)),
        );
        setToast({
          message: t('batch_changed', { 0: targets.length }),
          undo: () => {
            setClips((current) =>
              current.map((clip) => {
                const original = targets.find((target) => target.id === clip.id);
                return original ? { ...clip, deleted: original.deleted } : clip;
              }),
            );
          },
        });
      } else {
        const outcome = await live.changeDeletions(
          targets.map((clip) => ({
            id: clip.id,
            expectedRevision: clip.revision!,
            deleted: !clip.deleted,
          })),
        );
        const undo =
          outcome.completed.length && view !== 'trash'
            ? () => {
                if (batchLock.current) return;
                batchLock.current = true;
                setBatchBusy(true);
                void live
                  .changeDeletions(
                    outcome.completed.map((clip) => ({
                      id: clip.id,
                      expectedRevision: clip.revision,
                      deleted: false,
                    })),
                  )
                  .then((restored) =>
                    setToast({
                      message: restored.failure
                        ? t('batch_partial', {
                            0: restored.completed.length,
                            1: outcome.completed.length,
                            2: t(safeError(restored.failure).message),
                          })
                        : t('deletion_undone'),
                    }),
                  )
                  .catch((error) => setToast({ message: safeError(error).message }))
                  .finally(() => {
                    batchLock.current = false;
                    setBatchBusy(false);
                  });
              }
            : undefined;
        setToast({
          message: outcome.failure
            ? t('batch_partial', {
                0: outcome.completed.length,
                1: targets.length,
                2: t(safeError(outcome.failure).message),
              })
            : t(view === 'trash' ? 'batch_restored' : 'batch_changed', {
                0: outcome.completed.length,
              }),
          undo,
        });
      }
      replaceSelection([]);
      list.current?.focus({ preventScroll: true });
    } catch (error) {
      setToast({ message: safeError(error).message });
    } finally {
      batchLock.current = false;
      setBatchBusy(false);
    }
  }
  async function copySelected(paste = false, fallbackToCopy = false) {
    if (!ready || batchLock.current || copying.current || !selectedClips.length) return;
    if (selectedClips.length === 1) {
      await copyClip(selectedClips[0]!, paste, fallbackToCopy);
      return;
    }
    const targets = [...selectedClips];
    if (
      targets.some((clip) => clip.deleted || clip.type === 'image') ||
      targets.some((clip) => (clip.type === 'file') !== (targets[0]!.type === 'file'))
    ) {
      setToast({ message: t('batch_paste_types') });
      return;
    }
    copying.current = true;
    batchLock.current = true;
    setBatchBusy(true);
    try {
      if (appearance.native) {
        const outcome = await live.copy(
          targets[0]!.id,
          paste,
          fallbackToCopy,
          targets.map((clip) => clip.id),
        );
        setToast({
          message: outcome.warning
            ? `${outcome.pasteRequested ? t('copied_and_requested_paste_to_original_app') : t('copied')} · ${t(safeError(outcome.warning).message)}`
            : outcome.pasteRequested
              ? t('copied_and_requested_paste_to_original_app')
              : t('copied'),
        });
        if (outcome.pasteRequested) void desktop.hide().catch(() => {});
      } else if (targets[0]!.type === 'file') {
        setToast({ message: t('copying_0_is_not_available_yet', { 0: t('file') }) });
      } else {
        await navigator.clipboard.writeText(targets.map((clip) => clip.content).join('\n'));
        setToast({ message: t('sample_text_copied') });
      }
    } catch (error) {
      setToast({
        message: appearance.native
          ? safeError(error).message
          : t('the_browser_blocked_copying_select_and_copy_text_in_the_full_preview'),
      });
    } finally {
      copying.current = false;
      batchLock.current = false;
      setBatchBusy(false);
    }
  }
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
    if (!ready || batchLock.current || live.mutating || clip.deleted) return;
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
    if (batchLock.current) return;
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
        deleted: !clip.deleted,
      });
      setSelectedId(nextId);
      if (preview) closePreview();
      else requestAnimationFrame(() => list.current?.focus({ preventScroll: true }));
      if (clip.deleted) {
        const missingCollection =
          clip.collectionId &&
          !live.collections.some((collection) => collection.id === clip.collectionId);
        setToast({ message: t(missingCollection ? 'restored_to_history' : 'record_restored') });
        return;
      }
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
    if (
      batchLock.current ||
      !ready ||
      ![...result.items, ...selectedClips].some((item) => item.id === clip.id)
    )
      return;
    if (clip.deleted) {
      removeClip(clip);
      return;
    }
    if (appearance.native) {
      if (copying.current) return;
      copying.current = true;
      try {
        const outcome = await live.copy(
          clip.id,
          paste,
          fallbackToCopy,
          result.items.some((item) => item.id === clip.id) ? undefined : [clip.id],
        );
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
    if (!ready || batchLock.current || (appearance.native && clip.deleted)) return;
    setSelectedId(clip.id);
    setPreview({ id: clip.id, focus: menu?.anchor ?? (document.activeElement as HTMLElement) });
  }
  function closePreview() {
    const previous = preview?.focus;
    setPreview(null);
    requestAnimationFrame(() => {
      (previous?.getClientRects().length ? previous : list.current)?.focus({
        preventScroll: true,
      });
    });
  }
  function openMenu(anchor: HTMLElement, clip?: Clip) {
    if (clip && !ready) return;
    if (menu?.anchor === anchor) {
      setMenu(null);
      return;
    }
    if (clip && !selectedClips.some((item) => item.id === clip.id)) selectClip(clip.id);
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
      (previous?.getClientRects().length ? previous : list.current)?.focus({
        preventScroll: true,
      });
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
  async function deleteCollection() {
    if (!editingCollection || live.mutating) return;
    setCollectionError('');
    try {
      const count = await live.deleteCollection({
        id: editingCollection.id,
        expectedRevision: editingCollection.revision,
      });
      clearFilters();
      setPreview(null);
      closeModal();
      setToast({ message: t('collection_deleted_records', { 0: count }) });
    } catch (error) {
      setCollectionError(safeError(error).message);
    }
  }
  function openTransfer(clip: Clip, mode: TransferMode) {
    if (batchLock.current || clip.deleted) return;
    const clips = selectedClips.some((item) => item.id === clip.id) ? selectedClips : [clip];
    setTransfer({ clips, mode, completed: 0 });
    setTransferTarget(
      live.collections.find(
        (item) => mode === 'copy' || clips.some((clip) => clip.collectionId !== item.id),
      )?.id ?? '',
    );
    setTransferError('');
    setTransferUnknown(false);
    openModal('transfer');
  }
  async function saveTransfer(event: FormEvent) {
    event.preventDefault();
    if (!transfer || live.mutating || transferUnknown) return;
    const destination = transferTarget || null;
    const targets = transfer.clips.filter(
      (clip) => transfer.mode === 'copy' || (clip.collectionId ?? null) !== destination,
    );
    if (!targets.length || targets.some((clip) => !clip.revision)) return;
    setTransferError('');
    try {
      const clips = targets.map((clip) => ({ id: clip.id, expectedRevision: clip.revision! }));
      const request = { collectionId: destination, mode: transfer.mode };
      const outcome =
        clips.length === 1 && !transfer.completed
          ? { completed: [await live.transferClip({ ...request, ...clips[0]! })], failure: null }
          : await live.transferClips({ ...request, clips });
      if (outcome.failure) {
        const completed = transfer.completed + outcome.completed.length;
        setTransfer({ ...transfer, completed, clips: targets.slice(outcome.completed.length) });
        setTransferError(
          t('batch_partial', {
            0: completed,
            1: transfer.completed + targets.length,
            2: t(safeError(outcome.failure).message),
          }),
        );
        return;
      }
      setSelectedId(outcome.completed[0]!.id);
      setView(transferTarget ? `collection:${transferTarget}` : 'all');
      setQuery('');
      setType('all');
      setRegex(false);
      setPreview(null);
      closeModal();
      setToast({
        message:
          transfer.clips.length > 1 || transfer.completed
            ? t(
                transfer.mode === 'copy'
                  ? 'batch_copied_to_collection'
                  : 'batch_moved_to_collection',
                { 0: transfer.completed + outcome.completed.length },
              )
            : transfer.mode === 'copy'
              ? t('copied_to_collection')
              : transferTarget
                ? t('moved_to_collection')
                : t('removed_from_collection'),
      });
    } catch (error) {
      if (typeof error !== 'object' || error === null || !('code' in error)) {
        setTransferUnknown(true);
        setTransferError(t('transfer_outcome_unknown'));
      } else {
        setTransferError(safeError(error).message);
      }
    }
  }
  function clearFilters() {
    setView('all');
    setType('all');
    setQuery('');
    setRegex(false);
  }
  function toggleSearch() {
    setPreview(null);
    if (searchVisible) setQuery('');
    setSearchOverride(!searchVisible);
  }

  useEffect(() => {
    setSearchOverride(null);
  }, [appearance.showSearchByDefault]);
  useEffect(() => {
    if (!searchVisible) setQuery('');
    if (!settingsWindow && !modal && !preview && !menu)
      (searchVisible ? search.current : list.current)?.focus({ preventScroll: true });
  }, [searchVisible, settingsWindow]);
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
      const inListMovement = (action: string) =>
        (action === 'up' || action === 'down') && event.target !== search.current && event.shiftKey;
      const is = (action: keyof typeof keys) =>
        matchesKey(
          inListMovement(action)
            ? {
                ...event,
                code: event.code,
                ctrlKey: event.ctrlKey,
                metaKey: event.metaKey,
                altKey: event.altKey,
                shiftKey: keys[action].shift,
                isComposing: event.isComposing,
                repeat: event.repeat,
              }
            : event,
          keys[action],
          modifier === '⌘',
          action === 'search' || action === 'up' || action === 'down',
        );
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
        } else if (
          selectedClips.length > 1 ||
          (selectedClips.length === 1 &&
            !result.items.some((clip) => clip.id === selectedClips[0]!.id))
        ) {
          event.preventDefault();
          clearSelection();
        } else if (searchVisible) {
          event.preventDefault();
          toggleSearch();
        } else if (appearance.native) {
          event.preventDefault();
          void desktop.hide().catch((error) => setToast({ message: safeError(error).message }));
        }
        return;
      }
      const target = event.target;
      const inList =
        target instanceof HTMLElement &&
        (target === list.current || target.matches('[data-row-main]'));
      const mappedAction =
        historyKeymap &&
        !preview &&
        inList &&
        !event.ctrlKey &&
        !event.metaKey &&
        !event.altKey &&
        window.getSelection()?.isCollapsed !== false
          ? historyKeymap(event)
          : undefined;
      const isHistoryAction = (action: HistoryKeyAction) =>
        mappedAction === undefined ? is(action) : mappedAction === action;
      if (isHistoryAction('search')) {
        event.preventDefault();
        if (!event.repeat) toggleSearch();
        return;
      }
      if (preview || !(target instanceof HTMLElement)) return;
      const withinList = target === list.current || !!list.current?.contains(target);
      if (
        withinList &&
        event.code === 'KeyC' &&
        !event.altKey &&
        !event.shiftKey &&
        (modifier === '⌘' ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey)
      ) {
        // Let the system copy an explicit text selection, and keep input editing native.
        if (window.getSelection()?.isCollapsed === false) return;
        if (selectedClips.length && view !== 'trash') {
          event.preventDefault();
          if (!event.repeat) void copySelected();
        }
        return;
      }
      if (withinList && isHistoryAction('remove') && (!appearance.native || view !== 'trash')) {
        event.preventDefault();
        if (!event.repeat) void removeSelected();
        return;
      }
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
      if (inList && (event.ctrlKey || event.metaKey) && !event.altKey && event.code === 'KeyA') {
        event.preventDefault();
        if (ready && !batchLock.current)
          replaceSelection(
            result.items.map((clip, index) => ({ clip, index: live.offset + index })),
          );
        return;
      }
      if (isHistoryAction('down') || isHistoryAction('up')) {
        if (!ready || !result.items.length) return;
        event.preventDefault();
        const index = result.items.findIndex((clip) => clip.id === selected?.id);
        const next =
          result.items[
            Math.max(
              0,
              Math.min(result.items.length - 1, index + (isHistoryAction('down') ? 1 : -1)),
            )
          ];
        if (next) {
          selectClip(next.id, event.shiftKey);
          list.current
            ?.querySelector(`[data-clip-id="${next.id}"]`)
            ?.scrollIntoView({ block: 'nearest' });
          if (inList && target !== list.current)
            list.current
              ?.querySelector<HTMLElement>(`[data-clip-id="${next.id}"] [data-row-main]`)
              ?.focus({ preventScroll: true });
        }
      }
      if (isHistoryAction('copy')) {
        event.preventDefault();
        if (!event.repeat && ready && selected) {
          if (selectedClips.length && view === 'trash') void removeSelected();
          else if (selectedClips.length === 1) void copySelected(true, true);
          else if (selectedClips.length) void copySelected();
          else if (target === search.current && !selected.deleted) void copyClip(selected);
        }
      }
      if (isHistoryAction('preview') && inList) {
        event.preventDefault();
        if (!event.repeat && ready && selectedClips.length === 1) openPreview(selectedClips[0]!);
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
      savedAt: String(Date.now()),
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
        'closeToTray' | 'autoHide' | 'shortcut' | 'nativeTitlebar'
      >
    >,
  ) {
    if (!desktop.status) return;
    const p = desktop.status.preferences;
    try {
      await desktop.save({
        closeToTray: p.closeToTray,
        autoHide: p.autoHide,
        nativeTitlebar: p.nativeTitlebar,
        shortcut: p.shortcut,
        expectedRevision: p.revision,
        ...update,
      });
      setToast({ message: t('desktop_settings_saved') });
    } catch (error) {
      const message = safeError(error).message;
      setToast({ message });
      return message;
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
        <header className="topbar" id="history-search-panel" hidden={!searchVisible}>
          {searchBox}
          {searchVisible && (
            <div className="filter-bar">
              <div className="scope-control">
                <Icon
                  name={view === 'trash' ? 'trash' : view === 'pinned' ? 'pin' : 'history'}
                  size={15}
                />
                <FilterMenu
                  label={t('history_scope')}
                  value={view}
                  onChange={(value) => setView(value as AppView)}
                  options={scopeOptions}
                />
                <Icon name="chevron" size={12} />
              </div>
              <div className="type-control">
                <FilterMenu
                  label={t('content_type')}
                  value={type}
                  onChange={(value) => setType(value as ClipType | 'all')}
                  options={[
                    { value: 'all', label: t('all_types') },
                    ...Object.entries(typeNames).map(([key, label]) => ({
                      value: key,
                      label: t(label),
                      disabled: appearance.native && key === 'link',
                    })),
                  ]}
                />
                <Icon name="chevron" size={12} />
              </div>
            </div>
          )}
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
        <main
          className={`content-workspace ${collectionsOpen && !previewClip ? 'workspace-with-collections' : ''}`}
        >
          {appearance.native && collectionsOpen && !previewClip && (
            <nav
              id="collection-sidebar"
              aria-label={t('collections_sidebar')}
              className="collection-sidebar"
              style={{ '--collection-width': `${collectionWidth}px` } as CSSProperties}
            >
              <div className="collection-sidebar-items">
                {scopeOptions.map(({ value, label }) => (
                  <button
                    key={value}
                    type="button"
                    aria-current={view === value ? 'page' : undefined}
                    title={label}
                    onClick={() => setView(value as AppView)}
                  >
                    <Icon
                      name={
                        value === 'all'
                          ? 'history'
                          : value === 'trash'
                            ? 'trash'
                            : value === 'pinned'
                              ? 'pin'
                              : 'folder'
                      }
                      size={15}
                    />
                    <span>{label}</span>
                  </button>
                ))}
              </div>
              <input
                aria-label={t('collection_sidebar_width')}
                title={t('collection_sidebar_width')}
                type="range"
                min={128}
                max={280}
                value={collectionWidth}
                onChange={(event) => setCollectionWidth(Number(event.target.value))}
              />
            </nav>
          )}
          <section
            className="history-panel"
            hidden={!!previewClip}
            aria-label={t('clipboard_history')}
            onClick={(event) => {
              const target = event.target as HTMLElement;
              if (target.closest('.clip-row, button, input, a')) return;
              clearSelection();
              list.current?.focus({ preventScroll: true });
            }}
          >
            <span
              className={selectedClips.length > 1 ? 'history-selection-count' : 'sr-only'}
              aria-live="polite"
            >
              {t('selected_count', { 0: selectedClips.length })}
            </span>
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
                    className={`clip-row ${selectedClips.some((item) => item.id === clip.id) ? 'selected' : ''} ${clip.type === 'image' ? 'image-row' : ''}`}
                    onContextMenu={(event) => {
                      event.preventDefault();
                      openMenu(event.currentTarget.querySelector<HTMLElement>('.row-more')!, clip);
                    }}
                  >
                    <button
                      className="row-main"
                      data-row-main
                      aria-pressed={selectedClips.some((item) => item.id === clip.id)}
                      aria-label={`${t(typeNames[clip.type])}：${clip.type === 'text' ? clip.content.slice(0, 60) : clip.title}`}
                      onFocus={() => {
                        setSelectedId(clip.id);
                      }}
                      onClick={(event) =>
                        selectClip(clip.id, event.shiftKey, event.ctrlKey || event.metaKey)
                      }
                      onDoubleClick={(event) => {
                        if (
                          (!appearance.native || !clip.deleted) &&
                          !event.ctrlKey &&
                          !event.metaKey &&
                          !event.shiftKey
                        )
                          void copySelected(true, true);
                      }}
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
          {ready &&
            !modal &&
            !menu &&
            !preview &&
            !settingsWindow &&
            (!appearance.native || view !== 'trash') && (
              <HoverPreview
                key={JSON.stringify([query, regex, type, view, live.offset, oldest, grid])}
                listRef={list}
                clips={result.items}
                native={appearance.native}
                onError={(message) => setToast({ message: t(message) })}
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
                disabled={live.pending || batchBusy || !live.offset}
                onClick={() => live.setOffset(Math.max(0, live.offset - 25))}
              >
                {t('previous_page')}
              </button>
              <span>{Math.floor(live.offset / 25) + 1}</span>
              <button
                className="text-button"
                disabled={live.pending || batchBusy || live.offset + 25 >= live.total}
                onClick={() => live.setOffset(live.offset + 25)}
              >
                {t('next_page')}
              </button>
            </div>
          )}
          <div className="footer-tools">
            {appearance.native && (
              <button
                className="icon-button"
                type="button"
                aria-label={t('collections_sidebar')}
                title={t('collections_sidebar')}
                aria-expanded={collectionsOpen}
                aria-controls={collectionsOpen && !previewClip ? 'collection-sidebar' : undefined}
                onClick={() => {
                  setPreview(null);
                  setCollectionsOpen(!collectionsOpen);
                }}
              >
                <Icon name="folder" size={16} />
              </button>
            )}
            <button
              type="button"
              className={`icon-button search-toggle ${filtersActive ? 'has-filters' : ''}`}
              aria-label={searchToggleLabel}
              aria-controls="history-search-panel"
              aria-expanded={searchVisible}
              title={`${searchToggleLabel} (${formatKey(activeKeys.search, modifier)})`}
              onClick={toggleSearch}
            >
              <Icon name="search" size={16} />
            </button>
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
              aria-label={t('keep_window_visible')}
              title={
                appearance.native
                  ? desktop.status?.preferences.autoHide
                    ? t('hide_on_blur_click_to_keep_visible')
                    : t('window_fixed_click_to_hide_on_blur')
                  : t('window_pinning_requires_the_desktop_app')
              }
              aria-pressed={!!desktop.status && !desktop.status.preferences.autoHide}
              disabled={!appearance.native || !desktop.status || desktop.busy}
              onClick={() => void saveDesktop({ autoHide: !desktop.status?.preferences.autoHide })}
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
                  disabled={
                    !ready || batchBusy || !live.status?.pasteAvailable || !live.status.pasteTarget
                  }
                  onClick={() =>
                    menuAction(
                      () =>
                        void (selectedClips.length > 1
                          ? copySelected(true)
                          : copyClip(menuClip, true)),
                    )
                  }
                >
                  <Icon name="arrow" size={15} />
                  {t(selectedClips.length > 1 ? 'paste_selected' : 'paste_to_original_app_2')}
                </button>
              )}
              {!menuClip.deleted && (
                <button
                  role="menuitem"
                  data-testid="copy-history-item"
                  disabled={!ready || batchBusy}
                  onClick={() =>
                    menuAction(
                      () => void (selectedClips.length > 1 ? copySelected() : copyClip(menuClip)),
                    )
                  }
                >
                  <Icon name="copy" size={15} />
                  {t('copy')}
                </button>
              )}
              {selectedClips.length <= 1 && (
                <>
                  {(!appearance.native || !menuClip.deleted) && (
                    <button role="menuitem" onClick={() => menuAction(() => openPreview(menuClip))}>
                      <Icon name="text" size={15} />
                      {t('view_full_content')}
                    </button>
                  )}
                  {appearance.native && !menuClip.deleted && menuClip.type !== 'image' && (
                    <button
                      role="menuitem"
                      data-testid="edit-clip"
                      disabled={!ready || live.mutating}
                      onClick={() => {
                        setEditingClip(menuClip);
                        openModal('edit');
                      }}
                    >
                      <Icon name="text" size={15} />
                      {t('edit_record')}
                    </button>
                  )}
                  <button
                    role="menuitem"
                    data-testid="toggle-pin"
                    disabled={!ready || live.mutating || menuClip.deleted}
                    onClick={() => menuAction(() => void togglePin(menuClip))}
                  >
                    <Icon name="pin" size={15} />
                    {menuClip.pinned ? t('unpin') : t('pin')}
                  </button>
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
                </>
              )}
              {appearance.native && !menuClip.deleted && (
                <>
                  <button
                    role="menuitem"
                    disabled={!ready || batchBusy || live.mutating}
                    data-testid="move-to-collection"
                    onClick={() => openTransfer(menuClip, 'move')}
                  >
                    <Icon name="folder" size={15} />
                    {t(
                      selectedClips.length > 1
                        ? 'move_selected_to_collection'
                        : 'move_to_collection',
                    )}
                  </button>
                  <button
                    role="menuitem"
                    disabled={!ready || batchBusy || live.mutating || !live.collections.length}
                    data-testid="copy-to-collection"
                    onClick={() => openTransfer(menuClip, 'copy')}
                  >
                    <Icon name="copy" size={15} />
                    {t(
                      selectedClips.length > 1
                        ? 'copy_selected_to_collection'
                        : 'copy_to_collection',
                    )}
                  </button>
                </>
              )}
              <div className="menu-separator" />
              <button
                role="menuitem"
                className={menuClip.deleted ? '' : 'destructive'}
                data-testid="delete-history-item"
                disabled={!ready || batchBusy || live.mutating}
                onClick={() =>
                  menuAction(() =>
                    selectedClips.length > 1 ? void removeSelected() : removeClip(menuClip),
                  )
                }
              >
                <Icon name={menuClip.deleted ? 'restore' : 'trash'} size={15} />
                {menuClip.deleted
                  ? t('restore')
                  : selectedClips.length > 1
                    ? t('delete_selected')
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
                    <>
                      <button
                        role="menuitem"
                        data-testid="rename-collection"
                        disabled={live.mutating}
                        onClick={() => menuAction(() => openCollectionEditor(currentCollection))}
                      >
                        <Icon name="folder" size={15} />
                        {t('rename_current_collection')}
                      </button>
                      <button
                        role="menuitem"
                        data-testid="delete-collection"
                        className="destructive"
                        disabled={live.mutating}
                        onClick={() => {
                          setEditingCollection(currentCollection);
                          setCollectionError('');
                          openModal('delete-collection');
                        }}
                      >
                        <Icon name="trash" size={15} />
                        {t('delete_current_collection')}
                      </button>
                    </>
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
      {modal === 'delete-collection' && editingCollection && (
        <Modal
          title={t('delete_current_collection')}
          description={t('collection_delete_confirmation', { 0: editingCollection.name })}
          onClose={() => {
            if (!live.mutating) closeModal();
          }}
        >
          {collectionError && (
            <p role="alert" className="settings-feedback">
              {t(collectionError)}
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
              disabled={live.mutating}
              onClick={() => void deleteCollection()}
            >
              {t('delete')}
            </button>
          </div>
        </Modal>
      )}
      {modal === 'edit' && editingClip?.revision && (
        <ClipEditor
          clip={{ id: editingClip.id, expectedRevision: editingClip.revision }}
          load={live.editableContent}
          save={live.editClip}
          onClose={closeModal}
          onSaved={() => {
            setPreview(null);
            closeModal();
            setToast({ message: t('record_edited') });
          }}
        />
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
              disabled={live.mutating || transfer.completed > 0 || transferUnknown}
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
                  transferUnknown ||
                  (transfer.mode === 'copy'
                    ? !transferTarget
                    : transfer.clips.every((clip) => (clip.collectionId ?? '') === transferTarget))
                }
              >
                {transfer.clips.length > 1
                  ? t(
                      transfer.mode === 'copy' ? 'copy_selected_records' : 'move_selected_records',
                      {
                        0: transfer.clips.filter(
                          (clip) =>
                            transfer.mode === 'copy' ||
                            (clip.collectionId ?? '') !== transferTarget,
                        ).length,
                      },
                    )
                  : t(transfer.mode === 'copy' ? 'copy_record' : 'move_record')}
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
        <Modal title={t('settings')} onClose={closeModal} showCloseButton={!settingsWindow}>
          {settingsWindow && appearance.snapshot && appearance.snapshot.platform.os !== 'macos' && (
            <WindowResizeGrip
              os={appearance.snapshot.platform.os}
              onError={(message) => setToast({ message })}
            />
          )}
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
                    ...(appearance.native
                      ? ([['sync', t('sync_settings'), 'history']] as const)
                      : []),
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
                    {t('capture_payload_limits')}
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
                    {t('keep_window_visible')}
                    <input
                      type="checkbox"
                      checked={!!desktop.status && !desktop.status.preferences.autoHide}
                      disabled={!desktop.status || desktop.busy}
                      onChange={(event) => void saveDesktop({ autoHide: !event.target.checked })}
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
                  <p className="settings-feedback">{t('window_pin_behavior')}</p>
                  <label className="setting-row">
                    {t('quick_paste_include_pinned')}
                    <input
                      id="quick-paste-include-pinned"
                      type="checkbox"
                      checked={quickPaste.snapshot?.preferences.includePinned ?? false}
                      disabled={!quickPaste.snapshot || quickPaste.busy || desktop.busy}
                      onChange={(event) => {
                        const includePinned = event.target.checked;
                        void windowAction(() => quickPaste.save({ includePinned }));
                      }}
                    />
                  </label>
                </section>
              )}
              {appearance.native && (
                <div
                  className="shortcut-settings"
                  hidden={settingsWindow && settingsPage !== 'keyboard'}
                >
                  <GlobalShortcutSettings
                    desktop={desktop}
                    quickPaste={quickPaste}
                    os={appearance.snapshot?.platform.os ?? ''}
                    saveMainShortcut={(shortcut) => saveDesktop({ shortcut })}
                  />
                  <KeyboardSettings
                    snapshot={desktop.status?.keyboard}
                    busy={desktop.busy || quickPaste.busy}
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
                <label className="setting-row">
                  {t('show_search_by_default')}
                  <input
                    type="checkbox"
                    id="show-search-by-default"
                    checked={appearance.showSearchByDefault}
                    disabled={
                      appearance.saving ||
                      (appearance.native && (appearance.loading || !appearance.snapshot))
                    }
                    onChange={(event) =>
                      void appearance.saveShowSearchByDefault(event.target.checked)
                    }
                  />
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
              {settingsWindow && settingsPage === 'sync' && <SyncSettings />}
              <details
                className="about-section"
                open={settingsWindow || undefined}
                hidden={settingsWindow && settingsPage !== 'about'}
              >
                <summary>{t('about_and_status')}</summary>
                <p>UClip · 0.1.0</p>
                <p>
                  {appearance.native
                    ? t('about_sync_availability')
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
              <span>{t('toggle_search')}</span>
              <kbd>{formatKey(activeKeys.search, modifier)}</kbd>
            </div>
            <div>
              <span>{t('select_record')}</span>
              <kbd>
                {formatKey(activeKeys.up, modifier)} / {formatKey(activeKeys.down, modifier)}
              </kbd>
            </div>
            <div>
              <span>
                {t(
                  appearance.native
                    ? 'paste_or_copy_restore_from_trash'
                    : 'copy_restore_from_trash',
                )}
              </span>
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
              <span>{t('close_panel_or_hide_search')}</span>
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
              ? t('single_record_paste_hint')
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
