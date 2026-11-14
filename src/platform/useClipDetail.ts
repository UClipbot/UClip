import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { ClipDetail } from '../generated/contracts';
import { safeError } from './client';

// Read-only requests may finish after dismissal; only the current request may render.
export function useClipDetail(native: boolean, id?: string, revision?: string) {
  const key = native && id ? JSON.stringify([id, revision]) : '';
  const [result, setResult] = useState<{ key: string; detail?: ClipDetail; error?: string }>({
    key: '',
  });
  useEffect(() => {
    setResult({ key });
    if (!key || !id) return;
    let active = true;
    const timer = setTimeout(() => {
      if (!active) return;
      active = false;
      setResult({ key, error: 'content_preview_timed_out' });
    }, 5000);
    invoke<ClipDetail>('clip_detail', { id, thumbnail: false }).then(
      (detail) => {
        if (active) setResult({ key, detail });
        clearTimeout(timer);
      },
      (error) => {
        if (active) setResult({ key, error: safeError(error).message });
        clearTimeout(timer);
      },
    );
    return () => {
      active = false;
      clearTimeout(timer);
    };
  }, [key, id]);
  return {
    detail: key && result.key === key ? (result.detail ?? null) : null,
    detailError: key && result.key === key ? (result.error ?? '') : '',
  };
}
