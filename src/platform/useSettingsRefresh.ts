import { useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';

// Notifications carry no settings or clipboard content; every window reads committed state.
export function useSettingsRefresh(native: boolean, refresh: () => Promise<unknown>) {
  useEffect(() => {
    if (!native) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen('settings-changed', () => void refresh())
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch(() => {});
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [native, refresh]);
}
