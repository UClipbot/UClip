import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { CapturePolicy, ClipboardStatus, Theme } from '../generated/contracts';
import { t } from '../i18n';
import { safeError } from '../platform/client';
import { latestQuery } from '../platform/latest';
import { useAppearance } from '../platform/useAppearance';
import { useSettingsRefresh } from '../platform/useSettingsRefresh';
import { Icon, type IconName } from './Icon';
import './tray-menu.css';

const themes: { value: Theme; icon: IconName; label: string }[] = [
  { value: 'system', icon: 'monitor', label: 'tray_system_theme' },
  { value: 'light', icon: 'sun', label: 'light' },
  { value: 'dark', icon: 'moon', label: 'dark' },
];

export function TrayMenuWindow() {
  const appearance = useAppearance();
  const gate = useMemo(latestQuery, []);
  const [capture, setCapture] = useState<ClipboardStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState('');
  const [captureError, setCaptureError] = useState('');
  const acting = useRef(false);

  const refreshCapture = useCallback(
    () =>
      gate.run(
        () => invoke<ClipboardStatus>('clipboard_status'),
        (next) => {
          setCapture(next);
          setCaptureError('');
        },
        (error) => {
          setCapture(null);
          setCaptureError(safeError(error).message);
        },
      ),
    [gate],
  );
  useSettingsRefresh(appearance.native, refreshCapture);

  useEffect(() => {
    if (!appearance.native) return;
    const onFocus = () => {
      setActionError('');
      void refreshCapture();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || event.isComposing) return;
      event.preventDefault();
      void invoke('dismiss_tray_menu').catch((error) => setActionError(safeError(error).message));
    };
    void refreshCapture();
    window.addEventListener('focus', onFocus);
    window.addEventListener('keydown', onKeyDown);
    return () => {
      gate.invalidate();
      window.removeEventListener('focus', onFocus);
      window.removeEventListener('keydown', onKeyDown);
    };
  }, [appearance.native, gate, refreshCapture]);

  async function act(action: () => Promise<unknown>) {
    if (acting.current) return;
    acting.current = true;
    setBusy(true);
    setActionError('');
    try {
      await action();
    } catch (error) {
      setActionError(safeError(error).message);
    } finally {
      acting.current = false;
      setBusy(false);
    }
  }

  const enabled = capture?.policy.enabled ?? false;
  const locked = busy || appearance.saving;
  const error = actionError || appearance.error || captureError;
  const captureLabel = capture
    ? t(
        capture.available
          ? enabled
            ? 'monitoring_on'
            : 'monitoring_off'
          : 'capture_service_unavailable',
      )
    : t(captureError ? 'capture_service_unavailable' : 'connecting_to_capture_service');

  return (
    <main
      className={`tray-menu${appearance.snapshot?.platform.os === 'windows' ? ' tray-menu--floating' : ''}`}
      aria-label={t('system_tray')}
      onContextMenu={(event) => event.preventDefault()}
    >
      <button
        type="button"
        className="tray-menu-row tray-menu-capture"
        role="switch"
        aria-checked={enabled}
        aria-labelledby="tray-capture-label"
        aria-describedby="tray-capture-status"
        disabled={locked || !capture || (!capture.available && !enabled)}
        onClick={() =>
          void act(async () => {
            if (!capture) return;
            const { excludePattern, maxItems, revision } = capture.policy;
            try {
              await invoke<CapturePolicy>('update_capture_policy', {
                request: {
                  enabled: !enabled,
                  excludePattern,
                  maxItems,
                  expectedRevision: revision,
                },
              });
            } finally {
              await refreshCapture();
            }
          })
        }
      >
        <Icon name="copy" size={17} />
        <span className="tray-menu-capture-text">
          <strong id="tray-capture-label">{t('clipboard_monitoring')}</strong>
          <small id="tray-capture-status">{captureLabel}</small>
        </span>
        <span className="tray-menu-switch" aria-hidden="true" />
      </button>

      <div className="tray-menu-theme" role="group" aria-labelledby="tray-theme-label">
        <span id="tray-theme-label">{t('theme')}</span>
        <div className="tray-menu-theme-options">
          {themes.map(({ value, icon, label }) => (
            <button
              key={value}
              type="button"
              aria-pressed={appearance.theme === value}
              disabled={locked || (appearance.native && !appearance.snapshot)}
              onClick={() => void act(() => appearance.saveTheme(value))}
            >
              <Icon name={icon} size={15} />
              {t(label)}
            </button>
          ))}
        </div>
      </div>

      {error && <p role="alert">{t(error)}</p>}
      <div className="tray-menu-actions">
        <button
          type="button"
          className="tray-menu-row"
          disabled={locked || !appearance.native}
          onClick={() => void act(() => invoke('open_settings_window'))}
        >
          <Icon name="settings" size={17} />
          {t('settings')}
        </button>
        <button
          type="button"
          className="tray-menu-row tray-menu-quit"
          disabled={locked || !appearance.native}
          onClick={() => void act(() => invoke('quit_app'))}
        >
          <Icon name="power" size={17} />
          {t('quit_app')}
        </button>
      </div>
    </main>
  );
}
