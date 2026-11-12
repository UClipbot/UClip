import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { isTauri } from '@tauri-apps/api/core';
import { desktop, safeError } from './client';
import { latestQuery } from './latest';
import type { BootstrapSnapshot, Theme } from '../generated/contracts';

const native = isTauri();
function previewTheme(): Theme {
  if (native) return 'system';
  try {
    const value = localStorage.getItem('uclip-preview-theme');
    if (value === 'system' || value === 'light' || value === 'dark') return value;
  } catch {
    /* The preview still works with unavailable browser storage. */
  }
  return 'dark';
}
export function useAppearance() {
  const [snapshot, setSnapshot] = useState<BootstrapSnapshot | null>(null);
  const [theme, setTheme] = useState<Theme>(previewTheme);
  const [error, setError] = useState('');
  const [saving, setSaving] = useState(false);
  const [loading, setLoading] = useState(native);
  const [systemDark, setSystemDark] = useState(
    () => matchMedia('(prefers-color-scheme: dark)').matches,
  );
  const mutationPending = useRef(false);
  const gate = useMemo(latestQuery, []);
  const refresh = useCallback(() => {
    if (!native || mutationPending.current) return Promise.resolve();
    setLoading(true);
    return gate.run(
      desktop.bootstrap,
      (value) => {
        setSnapshot(value);
        setTheme(value.settings.theme);
        setError('');
        setLoading(false);
      },
      (failure) => {
        setError(safeError(failure).message);
        setLoading(false);
      },
    );
  }, [gate]);
  useEffect(() => {
    void refresh();
    const onFocus = () => {
      void refresh();
    };
    window.addEventListener('focus', onFocus);
    return () => {
      gate.invalidate();
      window.removeEventListener('focus', onFocus);
    };
  }, [gate, refresh]);
  useEffect(() => {
    const media = matchMedia('(prefers-color-scheme: dark)');
    const update = () => setSystemDark(media.matches);
    media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  }, []);
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);
  async function saveTheme(value: Theme) {
    if (mutationPending.current || (native && !snapshot)) return false;
    if (!native) {
      setTheme(value);
      try {
        localStorage.setItem('uclip-preview-theme', value);
      } catch {
        /* In-memory preview is sufficient. */
      }
      return true;
    }
    gate.invalidate();
    mutationPending.current = true;
    setSaving(true);
    try {
      const settings = await desktop.updateSettings({
        theme: value,
        expectedRevision: snapshot!.settings.revision,
      });
      setSnapshot((current) => current && { ...current, settings });
      setTheme(settings.theme);
      setError('');
      return true;
    } catch (failure) {
      setError(safeError(failure).message);
      return false;
    } finally {
      gate.invalidate();
      mutationPending.current = false;
      setLoading(false);
      setSaving(false);
    }
  }
  return {
    native,
    theme,
    dark: theme === 'dark' || (theme === 'system' && systemDark),
    snapshot,
    error,
    saving,
    loading,
    saveTheme,
    refresh,
  };
}
