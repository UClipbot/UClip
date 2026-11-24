import { invoke } from '@tauri-apps/api/core';
import { useEffect, useState } from 'react';
import { latestQuery } from '../platform/latest';
import { safeError } from '../platform/client';

const directions = [
  'North',
  'NorthEast',
  'East',
  'SouthEast',
  'South',
  'SouthWest',
  'West',
  'NorthWest',
] as const;

export function SettingsResizeHandles({ onError }: { onError: (message: string) => void }) {
  const [available, setAvailable] = useState(false);
  useEffect(() => {
    const query = latestQuery();
    const refresh = () =>
      void query.run(
        () => invoke<boolean>('settings_resize_available'),
        setAvailable,
        () => setAvailable(false),
      );
    refresh();
    window.addEventListener('resize', refresh);
    return () => {
      window.removeEventListener('resize', refresh);
      query.invalidate();
    };
  }, []);

  if (!available) return null;
  return (
    <div className="settings-resize-handles" aria-hidden="true">
      {directions.map((direction) => (
        <div
          key={direction}
          data-resize={direction}
          onMouseDown={(event) => {
            // A native drag can consume the release before WebKit resets its pointer state.
            if (event.button !== 0) return;
            event.preventDefault();
            void invoke('start_settings_resize', { direction }).catch((error) =>
              onError(safeError(error).message),
            );
          }}
        />
      ))}
    </div>
  );
}
