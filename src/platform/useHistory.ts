import { t, formatHistoryDate, formatHistoryTime } from '../i18n';
import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { safeError } from './client';
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
  CollectionItem,
  CollectionsSnapshot,
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
  const [detail, setDetail] = useState<ClipDetail | null>(null);
  const [detailError, setDetailError] = useState('');
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
    offset,
    historyRevision,
    queryEpoch,
  ]);
  const [settledKey, setSettledKey] = useState('');
  const pending = native && (loading || settledKey !== requestKey);
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
  }, [query, regex, type, pinnedOnly, collectionId]);
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
        request: {
          query,
          regex,
          pinnedOnly,
          collectionId,
          kind:
            type === 'all' ? null : type === 'image' ? 'image' : type === 'file' ? 'files' : 'text',
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
    offset,
    historyRevision,
    refresh,
    requestKey,
    settingsOnly,
  ]);
  useEffect(() => {
    if (!native || !previewId) {
      setDetail(null);
      setDetailError('');
      return;
    }
    let active = true;
    setDetail(null);
    setDetailError('');
    invoke<ClipDetail>('clip_detail', { id: previewId, thumbnail: false }).then(
      (value) => {
        if (active) setDetail(value);
      },
      (error) => {
        if (active) setDetailError(safeError(error).message);
      },
    );
    return () => {
      active = false;
    };
  }, [native, previewId]);
  useEffect(() => {
    if (!native || !page) return;
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
  }, [native, page]);
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
  async function copy(id: string, paste: boolean) {
    const result = await invoke<CopyResult>('copy_clip', { id, paste });
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
  const items: Clip[] = (page?.items ?? []).map((item) => ({
    id: item.id,
    revision: item.revision,
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
    collection: null,
    collectionId: item.collectionId ?? undefined,
    size:
      item.kind === 'image'
        ? `${item.width} × ${item.height}`
        : item.kind === 'files'
          ? t('0_items', { 0: item.fileCount })
          : undefined,
    image: thumbnails[item.id],
  }));
  return {
    status,
    collections: collections?.items ?? [],
    items,
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
    transferClip: (request: TransferClip) =>
      mutateHistory<ClipTransferSnapshot>('transfer_clip', request),
    changePin: (request: ChangeClipPin) =>
      mutateHistory<ClipPinSnapshot>('change_clip_pin', request),
    deduplicatePinned: (expectedRevision: string) =>
      mutateHistory<DeduplicationSnapshot>('deduplicate_pinned', { expectedRevision }),
    createText,
    mutating,
    copy,
    refresh,
    retryQuery: () => setQueryEpoch((epoch) => epoch + 1),
  };
}
