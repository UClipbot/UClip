import { invoke } from '@tauri-apps/api/core';
import { t } from '../i18n';
import { safeError } from '../platform/client';

export function WindowResizeGrip({
  os,
  onError,
}: {
  os: string;
  onError: (message: string) => void;
}) {
  return (
    <span
      className="window-resize-grip"
      data-testid="window-resize-grip"
      aria-hidden="true"
      title={t('resize_window')}
      onMouseDown={(event) => {
        if (os !== 'windows' && os !== 'linux') return;
        if (event.button !== 0) return;
        event.preventDefault();
        void invoke('resize_auxiliary_corner').catch((error) => onError(safeError(error).message));
      }}
    />
  );
}
