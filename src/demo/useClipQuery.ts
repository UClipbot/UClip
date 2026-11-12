import { useEffect, useMemo, useState } from 'react';
import { queryClips, type Clip, type ClipType, type View } from './clips';

type QueryInput = {
  clips: Clip[];
  view: View;
  type: ClipType | 'all';
  query: string;
  regex: boolean;
  oldest: boolean;
};
type Result = { items: Clip[]; error: string; pending: boolean };

export function useClipQuery(
  clips: Clip[],
  view: View,
  type: ClipType | 'all',
  query: string,
  regex: boolean,
  oldest: boolean,
): Result {
  const input = useMemo(
    () => ({ clips, view, type, query, regex, oldest }),
    [clips, view, type, query, regex, oldest],
  );
  const plain = useMemo(
    () => queryClips(clips, view, type, query, false, oldest),
    [clips, view, type, query, oldest],
  );
  const [completed, setCompleted] = useState<{ input: QueryInput; result: Result } | null>(null);
  useEffect(() => {
    if (!regex || !query) return;
    let worker: Worker | undefined;
    let deadline: ReturnType<typeof setTimeout> | undefined;
    let settled = false;
    const finish = (result: Omit<Result, 'pending'>) => {
      if (settled) return;
      settled = true;
      clearTimeout(deadline);
      worker?.terminate();
      setCompleted({ input, result: { ...result, pending: false } });
    };
    const debounce = setTimeout(() => {
      try {
        worker = new Worker(new URL('./clip-query.worker.ts', import.meta.url), { type: 'module' });
        worker.onmessage = (event: MessageEvent<Omit<Result, 'pending'>>) => finish(event.data);
        worker.onerror = () => finish({ items: [], error: '搜索暂不可用，请切换普通搜索。' });
        deadline = setTimeout(
          () => finish({ items: [], error: '表达式耗时过长，请简化搜索条件。' }),
          350,
        );
        worker.postMessage(input);
      } catch {
        finish({ items: [], error: '搜索暂不可用，请切换普通搜索。' });
      }
    }, 80);
    return () => {
      settled = true;
      clearTimeout(debounce);
      clearTimeout(deadline);
      worker?.terminate();
    };
  }, [input, regex, query]);
  if (!regex || !query) return { ...plain, pending: false };
  return completed?.input === input ? completed.result : { items: [], error: '', pending: true };
}
