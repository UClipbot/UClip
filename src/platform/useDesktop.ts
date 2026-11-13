import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type {
  DesktopStatus,
  UpdateDesktopPreferences,
  UpdateKeyBindings,
} from '../generated/contracts';
import { safeError } from './client';

export function useDesktop(native: boolean, interaction: boolean) {
  const [status, setStatus] = useState<DesktopStatus | null>(null);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const changing = useRef(false);
  const generation = useRef(0);
  const refresh = useCallback(async () => {
    if (!native || changing.current) return;
    const current = ++generation.current;
    try {
      const next = await invoke<DesktopStatus>('desktop_status');
      if (current === generation.current) {
        setStatus(next);
        setError(next.error ? safeError(next.error).message : '');
      }
    } catch (failure) {
      if (current === generation.current) setError(safeError(failure).message);
    }
  }, [native]);
  useEffect(() => {
    void refresh();
    window.addEventListener('focus', refresh);
    return () => {
      generation.current++;
      window.removeEventListener('focus', refresh);
    };
  }, [refresh]);
  useEffect(() => {
    if (!native) return;
    // Native blur timers use this only as a veto; the Rust lifetime is independent.
    void invoke('set_desktop_interaction', { active: interaction }).catch(() => {});
    const guard = () => void invoke('set_desktop_interaction', { active: true }).catch(() => {});
    const release = () =>
      void invoke('set_desktop_interaction', { active: interaction }).catch(() => {});
    document.addEventListener('pointerdown', guard, true);
    window.addEventListener('pointerup', release, true);
    document.addEventListener('compositionstart', guard, true);
    document.addEventListener('compositionend', release, true);
    return () => {
      document.removeEventListener('pointerdown', guard, true);
      window.removeEventListener('pointerup', release, true);
      document.removeEventListener('compositionstart', guard, true);
      document.removeEventListener('compositionend', release, true);
    };
  }, [native, interaction]);
  async function change(command: string, args: Record<string, unknown>) {
    if (changing.current) return;
    changing.current = true;
    generation.current++;
    setBusy(true);
    setError('');
    try {
      await invoke(command, args);
    } catch (failure) {
      setError(safeError(failure).message);
      throw failure;
    } finally {
      changing.current = false;
      setBusy(false);
      // Never retry a mutation automatically: it may already have committed.
      await refresh();
    }
  }
  return {
    status,
    error,
    busy,
    save: (request: UpdateDesktopPreferences) => change('update_desktop_preferences', { request }),
    saveKeys: (request: UpdateKeyBindings) => change('update_key_bindings', { request }),
    autostart: (enabled: boolean) => change('set_autostart', { enabled }),
    hide: () => invoke('hide_main_window'),
    minimize: () => invoke('minimize_main_window'),
    toggleMaximized: () => invoke('toggle_main_window_maximized'),
    startDrag: () =>
      invoke('start_main_window_drag').finally(() => {
        // The OS can consume pointerup while dragging; do not leave the blur guard stuck.
        void invoke('set_desktop_interaction', { active: interaction }).catch(() => {});
      }),
    close: () => invoke('close_main_window'),
    quit: () => invoke('quit_app'),
  };
}
