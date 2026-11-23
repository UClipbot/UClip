import { t, formatHistoryDate, formatHistoryTime } from '../i18n';
import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { safeError } from './client';
import { useClipDetail } from './useClipDetail';
import type {
  ChangeClipDeletion,
  ChangeClipPin,
  ClipPinSnapshot,
  DeduplicationSnapshot,
  ClipDeletionSnapshot,
  ClipboardStatus,
  ClipDetail,
  CopyResult,
  HistoryPage,
  PurgeTrash,
  CollectionItem,
  CollectionsSnapshot,
  DeleteCollection,
  ClipTransferBatch,
  ClipVersion,
  EditableContent,
  EditClip,
  TransferClips,
  RenameCollection,
  TransferClip,
  ClipTransferSnapshot,
  UpdateCapturePolicy,
} from '../generated/contracts';
import type { Clip, ClipType } from '../demo/clips';

export function useHistory(
  native: boolean,
  query: string,
  regex: boolean,
  type: ClipType | 'all',
  pinnedOnly: boolean,
  collectionId: string | null,
  trash: boolean,
  previewId?: string,
  settingsOnly = false,
) {
  const [status, setStatus] = useState<ClipboardStatus | null>(null);
  const [page, setPage] = useState<HistoryPage | null>(null);
  const [collections, setCollections] = useState<CollectionsSnapshot | null>(null);
  const [offset, setOffset] = useState(0);
  const [queryEpoch, setQueryEpoch] = useState(0);
  const [loading, setPending] = useState(native);
  const [error, setError] = useState('');
  const [statusError, setStatusError] = useState('');
  const { detail, detailError } = useClipDetail(native, previewId);
  const [thumbnails, setThumbnails] = useState<Record<string, string>>({});
  const [mutating, setMutating] = useState(false);
  const statusGeneration = useRef(0);
  const mutation = useRef(false);
  const mounted = useRef(true);
  const cache = useRef(new Map<string, string>());
  const pageRevision = useRef<string | null>(null);
  const historyRevision = status?.historyRevision;
  const requestKey = JSON.stringify([
    query,
    regex,
    type,
    pinnedOnly,
    collectionId,
    trash,
    offset,
    historyRevision,
    queryEpoch,
  ]);
  const [settledKey, setSettledKey] = useState('');
  const pending = native && (loading || settledKey !== requestKey);
  const queryScope = {
    query,
    regex,
    pinnedOnly,
    collectionId,
    kind: type === 'all' ? null : type === 'image' ? 'image' : type === 'file' ? 'files' : 'text',
  };
  const refresh = useCallback(async () => {
    if (!native || mutation.current) return;
    const generation = ++statusGeneration.current;
    try {
      const [next, categories] = await Promise.all([
        invoke<ClipboardStatus>('clipboard_status'),
        settingsOnly ? Promise.resolve(null) : invoke<CollectionsSnapshot>('collections_list'),
      ]);
      if (mounted.current && generation === statusGeneration.current) {
        setStatus(next);
        setCollections(categories);
        setStatusError('');
      }
    } catch (error) {
      if (mounted.current && generation === statusGeneration.current)
        setStatusError(safeError(error).message);
    }
  }, [native, settingsOnly]);
  useEffect(() => {
    mounted.current = true;
    if (!native) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    const tick = async () => {
      await refresh();
      if (!stopped) timer = setTimeout(tick, 750);
    };
    void tick();
    return () => {
      mounted.current = false;
      stopped = true;
      statusGeneration.current++;
      clearTimeout(timer);
    };
  }, [native, refresh]);
  useEffect(() => {
    setOffset(0);
  }, [query, regex, type, pinnedOnly, collectionId, trash]);
  useEffect(() => {
    if (!native || settingsOnly) return;
    let active = true;
    setPending(true);
    setError('');
    let deadline: ReturnType<typeof setTimeout>;
    const timer = setTimeout(() => {
      deadline = setTimeout(() => {
        if (!active) return;
        active = false;
        setSettledKey(requestKey);
        setError(safeError({ code: 'query_timeout' }).message);
        setPending(false);
      }, 5000);
      invoke<HistoryPage>('history_query', {
        trash,
        request: {
          ...queryScope,
          offset,
          expectedRevision: offset ? pageRevision.current : null,
        },
      }).then(
        (value) => {
          clearTimeout(deadline);
          if (active) {
            setSettledKey(requestKey);
            if (!value.items.length && offset > 0) {
              setOffset(Math.max(0, offset - 25));
              return;
            }
            pageRevision.current = value.revision;
            setPage(value);
            setPending(false);
          }
        },
        (failure) => {
          clearTimeout(deadline);
          if (!active) return;
          if (safeError(failure).code === 'conflict' && offset) {
            setOffset(0);
            void refresh();
          } else {
            setSettledKey(requestKey);
            setError(safeError(failure).message);
            setPending(false);
          }
        },
      );
    }, 80);
    return () => {
      active = false;
      clearTimeout(timer);
      clearTimeout(deadline);
    };
  }, [
    native,
    query,
    regex,
    type,
    pinnedOnly,
    collectionId,
    trash,
    offset,
    historyRevision,
    refresh,
    requestKey,
    settingsOnly,
  ]);
  useEffect(() => {
    if (!native || !page || trash) return;
    let active = true;
    void (async () => {
      for (const item of page.items) {
        if (!active) break;
        if (item.kind !== 'image' || cache.current.has(item.id)) continue;
        try {
          const image = await invoke<ClipDetail>('clip_detail', { id: item.id, thumbnail: true });
          if (!active) break;
          if (image.imageDataUrl) {
            cache.current.set(item.id, image.imageDataUrl);
            if (cache.current.size > 50) cache.current.delete(cache.current.keys().next().value!);
            setThumbnails(Object.fromEntries(cache.current));
          }
        } catch {
          /* Keep the type placeholder; explicit full preview reports read failures. */
        }
      }
    })();
    return () => {
      active = false;
    };
  }, [native, page, trash]);
  async function updatePolicy(request: UpdateCapturePolicy) {
    if (mutation.current) throw safeError({ code: 'busy' });
    mutation.current = true;
    statusGeneration.current++;
    setMutating(true);
    try {
      await invoke('update_capture_policy', { request });
    } finally {
      mutation.current = false;
      if (mounted.current) setMutating(false);
      await refresh();
    }
  }
  async function createText(text: string, collectionId: string | null = null) {
    if (mutation.current) throw safeError({ code: 'busy' });
    mutation.current = true;
    setMutating(true);
    statusGeneration.current++;
    try {
      return await invoke<string>('create_text', { text, collectionId });
    } finally {
      mutation.current = false;
      if (mounted.current) {
        setMutating(false);
        setQueryEpoch((epoch) => epoch + 1);
      }
      await refresh();
    }
  }
  async function changeDeletion(request: ChangeClipDeletion) {
    if (mutation.current) throw safeError({ code: 'busy' });
    mutation.current = true;
    statusGeneration.current++;
    setMutating(true);
    setPending(true);
    try {
      return await invoke<ClipDeletionSnapshot>('change_clip_deletion', { request });
    } finally {
      mutation.current = false;
      if (mounted.current) {
        setMutating(false);
        setQueryEpoch((epoch) => epoch + 1);
      }
      await refresh();
    }
  }
  async function changeDeletions(requests: ChangeClipDeletion[]) {
    if (mutation.current) throw safeError({ code: 'busy' });
    mutation.current = true;
    statusGeneration.current++;
    setMutating(true);
    setPending(true);
    const completed: ClipDeletionSnapshot[] = [];
    let failure: unknown;
    try {
      for (const request of requests) {
        try {
          completed.push(await invoke<ClipDeletionSnapshot>('change_clip_deletion', { request }));
        } catch (error) {
          failure = error;
          break;
        }
      }
      return { completed, failure };
    } finally {
      mutation.current = false;
      if (mounted.current) {
        setMutating(false);
        setQueryEpoch((epoch) => epoch + 1);
      }
      await refresh();
    }
  }
  async function copy(id: string, paste: boolean, fallbackToCopy: boolean, ids?: string[]) {
    const result = await invoke<CopyResult>('copy_clip', {
      id,
      ids,
      paste,
      fallbackToCopy,
      historyRevision: ids ? page?.revision : undefined,
    });
    await refresh();
    return result;
  }
  async function mutateHistory<T>(command: string, request: unknown) {
    if (mutation.current) throw safeError({ code: 'busy' });
    mutation.current = true;
    statusGeneration.current++;
    setMutating(true);
    setPending(true);
    try {
      return await invoke<T>(command, { request });
    } finally {
      mutation.current = false;
      if (mounted.current) {
        setMutating(false);
        setQueryEpoch((epoch) => epoch + 1);
      }
      await refresh();
    }
  }
  const mapItems = (items: HistoryPage['items']): Clip[] =>
    items.map((item) => ({
      id: item.id,
      revision: item.revision,
      savedAt: item.capturedAt,
      matchRanges: item.matchRanges,
      native: true,
      type: item.kind === 'files' ? 'file' : item.kind,
      title: item.kind === 'files' ? item.preview : '',
      content:
        item.kind === 'image'
          ? t('image_0_1', { 0: item.width ?? 0, 1: item.height ?? 0 })
          : item.preview,
      source: item.manual ? t('manually_created') : (item.sourceApp ?? t('unknown_source')),
      time: formatHistoryTime(Number(item.capturedAt)),
      day: formatHistoryDate(Number(item.capturedAt)),
      pinned: item.pinned,
      deleted: trash,
      collection: null,
      collectionId: item.collectionId ?? undefined,
      size:
        item.kind === 'image'
          ? `${item.width} × ${item.height}`
          : item.kind === 'files'
            ? t('0_items', { 0: item.fileCount })
            : undefined,
      image: trash ? undefined : thumbnails[item.id],
    }));
  async function readRange(offset: number, count: number) {
    if (!page || pending || error) throw safeError({ code: 'conflict' });
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      const range = await Promise.race([
        invoke<HistoryPage>('history_query', {
          trash,
          count,
          request: { ...queryScope, offset, expectedRevision: page.revision },
        }),
        new Promise<never>((_, reject) => {
          timer = setTimeout(() => reject(safeError({ code: 'query_timeout' })), 5000);
        }),
      ]);
      return mapItems(range.items);
    } finally {
      clearTimeout(timer);
    }
  }
  return {
    status,
    collections: collections?.items ?? [],
    items: mapItems(page?.items ?? []),
    pending,
    error,
    statusError,
    detail,
    detailError,
    total: page?.total ?? 0,
    revision: page?.revision,
    offset,
    setOffset,
    updatePolicy,
    changeDeletion,
    createCollection: (name: string) =>
      mutateHistory<CollectionItem>('create_collection', { name }),
    renameCollection: (request: RenameCollection) =>
      mutateHistory<CollectionItem>('rename_collection', request),
    deleteCollection: (request: DeleteCollection) =>
      mutateHistory<number>('delete_collection', request),
    editableContent: (request: ClipVersion) =>
      invoke<EditableContent>('editable_content', { request }),
    editClip: (request: EditClip) => mutateHistory<void>('edit_clip', request),
    transferClip: (request: TransferClip) =>
      mutateHistory<ClipTransferSnapshot>('transfer_clip', request),
    transferClips: (request: TransferClips) =>
      mutateHistory<ClipTransferBatch>('transfer_clips', request),
    changePin: (request: ChangeClipPin) =>
      mutateHistory<ClipPinSnapshot>('change_clip_pin', request),
    deduplicatePinned: (expectedRevision: string) =>
      mutateHistory<DeduplicationSnapshot>('deduplicate_pinned', { expectedRevision }),
    purgeTrash: (request: PurgeTrash) => mutateHistory<number>('purge_trash', request),
    createText,
    mutating,
    copy,
    changeDeletions,
    readRange,
    refresh,
    retryQuery: () => setQueryEpoch((epoch) => epoch + 1),
  };
}
