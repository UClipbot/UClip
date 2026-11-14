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
        worker.onerror = () =>
          finish({ items: [], error: 'search_is_unavailable_switch_to_plain_search' });
        deadline = setTimeout(
          () => finish({ items: [], error: 'the_expression_took_too_long_simplify_your_search' }),
          350,
        );
        worker.postMessage(input);
      } catch {
        finish({ items: [], error: 'search_is_unavailable_switch_to_plain_search' });
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
