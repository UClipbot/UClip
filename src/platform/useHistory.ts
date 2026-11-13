import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { safeError } from './client';
import type {
  ChangeClipDeletion,
  ClipDeletionSnapshot,
  ClipboardStatus,
  ClipDetail,
  CopyResult,
  HistoryPage,
  UpdateCapturePolicy,
} from '../generated/contracts';
import type { Clip, ClipType } from '../demo/clips';

export function useHistory(
  native: boolean,
  query: string,
  regex: boolean,
  type: ClipType | 'all',
  previewId?: string,
) {
  const [status, setStatus] = useState<ClipboardStatus | null>(null);
  const [page, setPage] = useState<HistoryPage | null>(null);
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
  const historyRevision = status?.historyRevision;
  const requestKey = JSON.stringify([query, regex, type, offset, historyRevision, queryEpoch]);
  const [settledKey, setSettledKey] = useState('');
  const pending = native && (loading || settledKey !== requestKey);
  const refresh = useCallback(async () => {
    if (!native || mutation.current) return;
    const generation = ++statusGeneration.current;
    try {
      const next = await invoke<ClipboardStatus>('clipboard_status');
      if (mounted.current && generation === statusGeneration.current) {
        setStatus(next);
        setStatusError('');
      }
    } catch (error) {
      if (mounted.current && generation === statusGeneration.current)
        setStatusError(safeError(error).message);
    }
  }, [native]);
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
  }, [query, regex, type]);
  useEffect(() => {
    if (!native) return;
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
          kind:
            type === 'all' ? null : type === 'image' ? 'image' : type === 'file' ? 'files' : 'text',
          offset,
          expectedRevision: offset ? (historyRevision ?? null) : null,
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
  }, [native, query, regex, type, offset, historyRevision, refresh, requestKey]);
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
  async function createText(text: string) {
    if (mutation.current) throw safeError({ code: 'busy' });
    mutation.current = true;
    setMutating(true);
    statusGeneration.current++;
    try {
      return await invoke<string>('create_text', { text });
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
  const items: Clip[] = (page?.items ?? []).map((item) => ({
    id: item.id,
    revision: item.revision,
    matchRanges: item.matchRanges,
    native: true,
    type: item.kind === 'files' ? 'file' : item.kind,
    title: item.kind === 'files' ? item.preview : '',
    content: item.preview,
    source: item.manual ? '手动创建' : '来源未知',
    time: new Date(Number(item.capturedAt)).toLocaleTimeString('zh-CN', {
      hour: '2-digit',
      minute: '2-digit',
    }),
    day:
      new Date(Number(item.capturedAt)).toDateString() === new Date().toDateString()
        ? '今天'
        : '昨天',
    pinned: false,
    collection: null,
    size:
      item.kind === 'image'
        ? `${item.width} × ${item.height}`
        : item.kind === 'files'
          ? `${item.fileCount} 项`
          : undefined,
    image: thumbnails[item.id],
  }));
  return {
    status,
    items,
    pending,
    error,
    statusError,
    detail,
    detailError,
    total: page?.total ?? 0,
    offset,
    setOffset,
    updatePolicy,
    changeDeletion,
    createText,
    mutating,
    copy,
    refresh,
    retryQuery: () => setQueryEpoch((epoch) => epoch + 1),
  };
}
