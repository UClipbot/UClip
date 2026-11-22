import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { isTauri } from '@tauri-apps/api/core';
import { desktop, safeError } from './client';
import { latestQuery } from './latest';
import { useSettingsRefresh } from './useSettingsRefresh';
import { getLocale, setLocale, subscribeLocale, resolveLanguage } from '../i18n';
import type { BootstrapSnapshot, Language, Theme } from '../generated/contracts';

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
function previewLanguage(): Language {
  if (!native) {
    try {
      const value = localStorage.getItem('uclip-preview-language');
      if (value === 'system' || value === 'zh-CN' || value === 'en') return value;
    } catch {
      /* Storage is optional in the preview. */
    }
  }
  return 'system';
}
export function useAppearance() {
  const locale = useSyncExternalStore(subscribeLocale, getLocale);
  const [language, setLanguage] = useState<Language>(previewLanguage);
  const [snapshot, setSnapshot] = useState<BootstrapSnapshot | null>(null);
  const [theme, setTheme] = useState<Theme>(previewTheme);
  const [previewShowSearch, setPreviewShowSearch] = useState(() => {
    if (native) return false;
    try {
      return localStorage.getItem('uclip-preview-show-search') !== 'false';
    } catch {
      return true;
    }
  });
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
        setLanguage(value.settings.language);
        setError('');
        setLoading(false);
      },
      (failure) => {
        setError(safeError(failure).message);
        setLoading(false);
      },
    );
  }, [gate]);
  useSettingsRefresh(native, refresh);
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
  useEffect(() => {
    const update = () => setLocale(resolveLanguage(language, navigator.languages));
    update();
    window.addEventListener('languagechange', update);
    return () => window.removeEventListener('languagechange', update);
  }, [language]);
  useEffect(() => {
    document.documentElement.lang = locale;
    // Only the main window supplies the resolved system language to native menus.
    if (native && snapshot && !new URLSearchParams(location.search).get('window')) {
      void desktop.setUiLanguage(resolveLanguage(language, navigator.languages)).catch(() => {});
    }
  }, [language, locale, !!snapshot]);
  async function savePreferences(value: {
    theme?: Theme;
    language?: Language;
    showSearchByDefault?: boolean;
  }) {
    if (mutationPending.current || (native && !snapshot)) return false;
    if (!native) {
      if (value.theme) setTheme(value.theme);
      if (value.language) setLanguage(value.language);
      if (value.showSearchByDefault !== undefined) setPreviewShowSearch(value.showSearchByDefault);
      try {
        if (value.theme) localStorage.setItem('uclip-preview-theme', value.theme);
        if (value.language) localStorage.setItem('uclip-preview-language', value.language);
        if (value.showSearchByDefault !== undefined)
          localStorage.setItem('uclip-preview-show-search', String(value.showSearchByDefault));
      } catch {
        /* In-memory preview is sufficient. */
      }
      return true;
    }
    gate.invalidate();
    mutationPending.current = true;
    setSaving(true);
    let committed = false;
    try {
      const settings = await desktop.updateSettings({
        theme: value.theme ?? snapshot!.settings.theme,
        language: value.language ?? null,
        showSearchByDefault: value.showSearchByDefault ?? null,
        expectedRevision: snapshot!.settings.revision,
      });
      setSnapshot((current) => current && { ...current, settings });
      setTheme(settings.theme);
      setLanguage(settings.language);
      committed = true;
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
      // A settings notification can arrive while a save is pending. Re-read after success.
      if (committed) await refresh();
    }
  }
  return {
    native,
    theme,
    showSearchByDefault: snapshot?.settings.showSearchByDefault ?? previewShowSearch,
    language,
    locale,
    dark: theme === 'dark' || (theme === 'system' && systemDark),
    snapshot,
    error,
    saving,
    loading,
    saveTheme: (theme: Theme) => savePreferences({ theme }),
    saveLanguage: (language: Language) => savePreferences({ language }),
    saveShowSearchByDefault: (showSearchByDefault: boolean) =>
      savePreferences({ showSearchByDefault }),
    refresh,
  };
}
