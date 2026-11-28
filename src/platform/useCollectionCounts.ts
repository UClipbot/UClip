import { invoke } from '@tauri-apps/api/core';
import { useEffect, useRef, useState } from 'react';
import type { CollectionCounts } from '../generated/contracts';

export function useCollectionCounts(visible: boolean, historyRevision?: string) {
  const [snapshot, setSnapshot] = useState<CollectionCounts>();
  const pending = useRef<Promise<CollectionCounts> | undefined>(undefined);
  const countedRevision = snapshot?.revision;

  useEffect(() => {
    if (!visible || historyRevision === undefined) return;
    if (countedRevision !== undefined && BigInt(countedRevision) >= BigInt(historyRevision)) return;
    let active = true;
    let retry: ReturnType<typeof setTimeout>;
    async function refresh() {
      try {
        const next = await (pending.current ??= invoke<CollectionCounts>(
          'collection_counts',
        ).finally(() => {
          pending.current = undefined;
        }));
        if (active) setSnapshot(next);
      } catch {
        if (active) retry = setTimeout(refresh, 750);
      }
    }
    void refresh();
    return () => {
      active = false;
      clearTimeout(retry);
    };
  }, [visible, historyRevision, countedRevision]);

  return snapshot?.counts;
}
