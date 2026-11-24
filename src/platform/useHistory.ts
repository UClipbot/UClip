import { t, formatHistoryDate, formatHistoryTime, getLocale } from '../i18n';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
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
  HistoryQuery,
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

const HISTORY_BATCH = 100;
const MAX_CACHED_ROWS = 4000;

export type HistoryRange = { start: number; end: number };

type HistorySnapshot = {
  scope: string;
  key: string;
  revision: string;
  total: number;
  rows: ReadonlyMap<number, HistoryPage['items'][number]>;
};

function firstMissing(snapshot: HistorySnapshot, range: HistoryRange) {
  for (let index = range.start; index < Math.min(range.end, snapshot.total); index++) {
    if (!snapshot.rows.has(index)) return Math.floor(index / HISTORY_BATCH) * HISTORY_BATCH;
  }
  return undefined;
}

function cacheRows(
  existing: HistorySnapshot['rows'] | undefined,
  page: HistoryPage,
  offset: number,
) {
  const rows = new Map(existing);
  page.items.forEach((item, index) => rows.set(offset + index, item));
  if (rows.size > MAX_CACHED_ROWS) {
    const center = offset + page.items.length / 2;
    const evictable = [...rows.keys()]
      .filter((index) => index >= HISTORY_BATCH)
      .sort((a, b) => Math.abs(b - center) - Math.abs(a - center));
    for (const index of evictable.slice(0, rows.size - MAX_CACHED_ROWS)) rows.delete(index);
  }
  return rows;
}

async function readHistory(request: HistoryQuery, trash: boolean, count?: number) {
  let deadline: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      invoke<HistoryPage>('history_query', { request, trash, count }),
      new Promise<never>((_, reject) => {
        deadline = setTimeout(() => reject(safeError({ code: 'query_timeout' })), 5000);
      }),
    ]);
  } finally {
    clearTimeout(deadline);
  }
}

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
  const queryScope = JSON.stringify([query, regex, type, pinnedOnly, collectionId, trash]);
  const [status, setStatus] = useState<ClipboardStatus | null>(null);
  const [result, setResult] = useState<HistorySnapshot>();
  const [collections, setCollections] = useState<CollectionsSnapshot | null>(null);
  const [range, setRange] = useState({ scope: queryScope, start: 0, end: 0 });
  const [queryEpoch, setQueryEpoch] = useState(0);
  const [failure, setFailure] = useState<{ key: string; message: string }>();
  if (range.scope !== queryScope) {
    setRange({ scope: queryScope, start: 0, end: 0 });
    setResult(undefined);
    setFailure(undefined);
  }
  const [statusError, setStatusError] = useState('');
  const { detail, detailError } = useClipDetail(native, previewId);
  const [thumbnails, setThumbnails] = useState<Record<string, string>>({});
  const [visibleImages, setVisibleImages] = useState<readonly string[]>([]);
  const [mutating, setMutating] = useState(false);
  const statusGeneration = useRef(0);
  const mutation = useRef(false);
  const mounted = useRef(true);
  const cache = useRef(new Map<string, string>());
  const historyRevision = status?.historyRevision;
  const requestKey = JSON.stringify([queryScope, historyRevision, queryEpoch]);
  const page = result?.scope === queryScope ? result : undefined;
  const current = page?.key === requestKey ? page : undefined;
  const windowOffset = current ? firstMissing(current, range) : undefined;
  const refreshStart = current || !page ? 0 : range.start;
  const refreshEnd = current || !page ? 0 : range.end;
  const loadKey = JSON.stringify([requestKey, windowOffset]);
  const queryError = failure?.key === loadKey ? failure.message : '';
  const pending = native && !settingsOnly && (mutating || (!current && !queryError));
  const requestRange = useCallback(
    ({ start, end }: HistoryRange) => setRange({ scope: queryScope, start, end }),
    [queryScope],
  );
  const queryFields = useMemo<Omit<HistoryQuery, 'offset' | 'expectedRevision'>>(
    () => ({
      query,
      regex,
      pinnedOnly,
      collectionId,
      kind: type === 'all' ? null : type === 'image' ? 'image' : type === 'file' ? 'files' : 'text',
    }),
    [query, regex, pinnedOnly, collectionId, type],
  );
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
    if (
      !native ||
      settingsOnly ||
      mutating ||
      queryError ||
      (current && windowOffset === undefined)
    )
      return;
    let active = true;
    const load = async () => {
      try {
        if (current && windowOffset !== undefined) {
          const next = await readHistory(
            { ...queryFields, offset: windowOffset, expectedRevision: current.revision },
            trash,
            Math.min(HISTORY_BATCH, current.total - windowOffset),
          );
          if (!next.items.length) throw safeError({ code: 'conflict' });
          if (active) setResult({ ...current, rows: cacheRows(current.rows, next, windowOffset) });
          return;
        }
        const head = await readHistory(
          { ...queryFields, offset: 0, expectedRevision: null },
          trash,
        );
        if (!active) return;
        let rows = cacheRows(undefined, head, 0);
        // Refresh the region being read together with the new total, so a
        // recently captured record does not displace the reader's anchor.
        if (refreshEnd > head.items.length && head.total > head.items.length) {
          const span = Math.min(1000, refreshEnd - refreshStart);
          const offset = Math.max(0, Math.min(refreshStart, head.total - span));
          const next = await readHistory(
            { ...queryFields, offset, expectedRevision: head.revision },
            trash,
            Math.min(span, head.total - offset),
          );
          rows = cacheRows(rows, next, offset);
        }
        if (active)
          setResult({
            scope: queryScope,
            key: requestKey,
            revision: head.revision,
            total: head.total,
            rows,
          });
      } catch (error) {
        if (!active) return;
        const reason = safeError(error);
        setFailure({ key: loadKey, message: reason.message });
        if (reason.code === 'conflict') void refresh();
      }
    };
    const timer = setTimeout(() => void load(), current ? 40 : 80);
    return () => {
      active = false;
      clearTimeout(timer);
    };
  }, [
    native,
    trash,
    current,
    windowOffset,
    refreshStart,
    refreshEnd,
    mutating,
    queryError,
    queryFields,
    queryScope,
    refresh,
    requestKey,
    loadKey,
    settingsOnly,
  ]);
  useEffect(() => {
    if (!native || !page || trash || !visibleImages.length) return;
    let active = true;
    void (async () => {
      for (const id of visibleImages) {
        if (!active) return;
        if (cache.current.has(id)) continue;
        try {
          const image = await invoke<ClipDetail>('clip_detail', { id, thumbnail: true });
          if (!active) return;
          if (image.imageDataUrl) {
            cache.current.set(id, image.imageDataUrl);
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
  }, [native, page, trash, visibleImages]);
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
  const locale = getLocale();
  const mapItem = useCallback(
    (item: HistoryPage['items'][number]): Clip => ({
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
    }),
    [trash, thumbnails, locale],
  );
  const itemsByPosition = useMemo(
    () =>
      new Map(
        [...(page?.rows ?? [])]
          .sort(([a], [b]) => a - b)
          .map(([index, item]) => [index, mapItem(item)]),
      ),
    [page, mapItem],
  );
  const items = useMemo(() => [...itemsByPosition.values()], [itemsByPosition]);
  async function readRange(start: number, count: number) {
    if (!current || pending) throw safeError({ code: 'conflict' });
    if (count > 1000) throw safeError({ code: 'limit_exceeded' });
    const window = await readHistory(
      { ...queryFields, offset: start, expectedRevision: current.revision },
      trash,
      count,
    );
    return window.items.map(mapItem);
  }
  return {
    status,
    collections: collections?.items ?? [],
    items,
    itemsByPosition,
    pending,
    error: current ? '' : queryError,
    moreError: current ? queryError : '',
    loadingWindow: windowOffset !== undefined && !pending && !queryError,
    requestRange,
    readRange,
    retryMore: () => setFailure(undefined),
    setVisibleImages,
    statusError,
    detail,
    detailError,
    total: page?.total ?? 0,
    revision: page?.revision,
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
    refresh,
    retryQuery: () => {
      setFailure(undefined);
      setQueryEpoch((epoch) => epoch + 1);
    },
  };
}
