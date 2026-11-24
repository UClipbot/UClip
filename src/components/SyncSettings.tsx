import { invoke } from '@tauri-apps/api/core';
import { useEffect, useRef, useState } from 'react';
import type {
  SyncRemoteInput,
  SyncRemoteProgress,
  SyncStatus,
  UpdateSync,
} from '../generated/contracts';
import { formatTimestamp, t } from '../i18n';
import { safeError } from '../platform/client';

type RemoteKind = 'off' | SyncRemoteInput['kind'];

function remoteProgressLabel(progress: SyncRemoteProgress) {
  switch (progress.stage) {
    case 'queued':
      return t('sync_storage_requested');
    case 'connecting':
      return t('sync_storage_connecting');
    case 'checking':
      return t('sync_storage_checking');
    case 'downloading':
      return t('sync_storage_downloading_0_1', { 0: progress.processed, 1: progress.total });
    case 'uploading':
      return t('sync_storage_uploading_0_1', { 0: progress.processed, 1: progress.total });
    case 'finalizing':
      return t('sync_storage_finalizing');
  }
}

export function SyncSettings() {
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [code, setCode] = useState('');
  const [visibleCode, setVisibleCode] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [historyOverLimit, setHistoryOverLimit] = useState<'create' | 'join' | null>(null);
  const [loadingError, setLoadingError] = useState('');
  const [kind, setKind] = useState<RemoteKind>('off');
  const [initialized, setInitialized] = useState(false);
  const generation = useRef(0);
  const saving = useRef(false);

  useEffect(() => {
    let running = true;
    let timer: ReturnType<typeof setTimeout>;
    const refresh = async () => {
      const requested = generation.current;
      const current = () => running && !saving.current && requested === generation.current;
      try {
        const next = await invoke<SyncStatus>('sync_status');
        if (current()) {
          setStatus(next);
          setLoadingError('');
        }
      } catch (failure) {
        if (current()) setLoadingError(safeError(failure).message);
      }
      if (running) timer = setTimeout(refresh, 1200);
    };
    void refresh();
    return () => {
      running = false;
      clearTimeout(timer);
    };
  }, []);

  useEffect(() => {
    if (status && !initialized) {
      setKind(status.remote?.kind ?? 'off');
      setInitialized(true);
    }
  }, [initialized, status]);

  async function perform(operation: () => Promise<void>, action?: UpdateSync['action']) {
    if (saving.current) return;
    saving.current = true;
    generation.current++;
    setBusy(true);
    setError('');
    try {
      await operation();
    } catch (failure) {
      const reason = safeError(failure);
      if ((action === 'create' || action === 'join') && reason.code === 'limit_exceeded') {
        setHistoryOverLimit(action);
        setError('sync_existing_history_over_limit');
      } else {
        setError(reason.message);
      }
    } finally {
      saving.current = false;
      setBusy(false);
    }
  }

  const update = (request: UpdateSync) =>
    perform(async () => {
      setStatus(await invoke<SyncStatus>('update_sync', { request }));
      if (request.action === 'join' || request.action === 'join_from_now') setCode('');
    }, request.action);

  const remote = status?.remote;
  const progress = status?.remoteProgress;
  const countedProgress = progress && 'total' in progress ? progress : null;
  const roleMessage = {
    off: 'sync_lan_off',
    discovering: 'sync_lan_discovering',
    host: 'sync_lan_host',
    client: 'sync_lan_client',
  } as const;
  const lastSync = (value: string | null) =>
    value && t('sync_last_0', { 0: formatTimestamp(Number(value) * 1000) });

  return (
    <section className="settings-section sync-settings" aria-labelledby="sync-heading">
      <h3 id="sync-heading">{t('sync_settings')}</h3>
      {(error || loadingError) && (
        <p className="settings-feedback" role="alert">
          {t(error || loadingError)}
        </p>
      )}
      {!status && !loadingError && <p role="status">{t('sync_loading')}</p>}
      {status && !status.paired && (
        <>
          <p>{t('sync_share_history')}</p>
          <button
            className="secondary-button"
            disabled={busy}
            onClick={() => void update({ action: 'create' })}
          >
            {t('sync_create_group')}
          </button>
          {historyOverLimit === 'create' && (
            <div>
              <p>{t('sync_create_from_now_description')}</p>
              <button
                className="secondary-button"
                disabled={busy}
                onClick={() => void update({ action: 'create_from_now' })}
              >
                {t('sync_create_from_now')}
              </button>
            </div>
          )}
          <form
            className="new-form sync-form"
            onSubmit={(event) => {
              event.preventDefault();
              void update({ action: 'join', code });
            }}
          >
            <label htmlFor="sync-join-code">{t('sync_pairing_code')}</label>
            <input
              id="sync-join-code"
              type="password"
              value={code}
              maxLength={64}
              autoComplete="off"
              onChange={(event) => setCode(event.target.value)}
              spellCheck={false}
              required
            />
            <button className="secondary-button" disabled={busy || !code.trim()}>
              {t('sync_join_group')}
            </button>
            {historyOverLimit === 'join' && (
              <>
                <p>{t('sync_create_from_now_description')}</p>
                <button
                  type="button"
                  className="secondary-button"
                  disabled={busy || !code.trim()}
                  onClick={() => void update({ action: 'join_from_now', code })}
                >
                  {t('sync_join_from_now')}
                </button>
              </>
            )}
          </form>
        </>
      )}
      {status?.paired && (
        <>
          <label className="setting-row">
            {t('sync_lan_enabled')}
            <input
              type="checkbox"
              checked={status.lanEnabled}
              disabled={busy}
              onChange={(event) => void update({ action: 'lan', enabled: event.target.checked })}
            />
          </label>
          <p className="settings-feedback" role="status">
            {t(roleMessage[status.role], { 0: status.peers })}
            {status.lanError ? ` · ${t(safeError(status.lanError).message)}` : ''}
            {status.lanLastSync ? ` · ${lastSync(status.lanLastSync)}` : ''}
          </p>
          <button
            className="secondary-button"
            disabled={busy}
            onClick={() => {
              if (visibleCode) setVisibleCode('');
              else
                void perform(async () => setVisibleCode(await invoke<string>('sync_pairing_code')));
            }}
          >
            {t(visibleCode ? 'sync_hide_code' : 'sync_show_code')}
          </button>
          {visibleCode && (
            <div className="new-form sync-form">
              <label htmlFor="sync-show-code">{t('sync_pairing_code')}</label>
              <input
                id="sync-show-code"
                type="text"
                value={visibleCode}
                readOnly
                onFocus={(event) => event.target.select()}
                spellCheck={false}
                autoComplete="off"
              />
              <p className="settings-feedback">{t('sync_code_privacy')}</p>
            </div>
          )}

          <h3>{t('sync_remote_settings')}</h3>
          <label className="setting-row">
            {t('sync_storage_type')}
            <select
              value={kind}
              disabled={busy}
              onChange={(event) => setKind(event.target.value as RemoteKind)}
            >
              <option value="off">{t('sync_storage_off')}</option>
              <option value="webdav">WebDAV</option>
              <option value="sftp">SFTP</option>
            </select>
          </label>
          <form
            key={kind}
            className="new-form sync-form"
            onSubmit={(event) => {
              event.preventDefault();
              const form = new FormData(event.currentTarget);
              const read = (name: string) => String(form.get(name) ?? '');
              let next: SyncRemoteInput | null = null;
              if (kind === 'webdav') {
                next = {
                  kind,
                  url: read('url').trim(),
                  username: read('username'),
                  password: read('password'),
                };
              } else if (kind === 'sftp') {
                next = {
                  kind,
                  host: read('host').trim(),
                  port: Number(read('port')),
                  directory: read('directory').trim(),
                  username: read('username'),
                  password: read('password'),
                  hostKey: read('hostKey').trim(),
                };
              }
              const password =
                event.currentTarget.querySelector<HTMLInputElement>('input[type=password]');
              if (password) password.value = '';
              void update({ action: 'remote', remote: next });
            }}
          >
            {kind === 'webdav' && (
              <>
                <label htmlFor="sync-dav-url">{t('sync_dav_url')}</label>
                <input
                  id="sync-dav-url"
                  aria-describedby="sync-dav-folder-hint"
                  name="url"
                  type="url"
                  defaultValue={remote?.kind === kind ? remote.url : ''}
                  maxLength={2048}
                  required
                />
                <p id="sync-dav-folder-hint" className="settings-feedback">
                  {t('sync_dav_folder_hint')}
                </p>
              </>
            )}
            {kind === 'sftp' && (
              <>
                <label htmlFor="sync-sftp-host">{t('sync_sftp_host')}</label>
                <input
                  id="sync-sftp-host"
                  name="host"
                  defaultValue={remote?.kind === kind ? remote.host : ''}
                  maxLength={253}
                  required
                />
                <label htmlFor="sync-sftp-port">{t('sync_sftp_port')}</label>
                <input
                  id="sync-sftp-port"
                  name="port"
                  type="number"
                  min={1}
                  max={65535}
                  defaultValue={remote?.kind === kind ? remote.port : 22}
                  required
                />
                <label htmlFor="sync-sftp-folder">{t('sync_sftp_directory')}</label>
                <input
                  id="sync-sftp-folder"
                  name="directory"
                  defaultValue={remote?.kind === kind ? remote.directory : ''}
                  maxLength={2048}
                  required
                />
                <label htmlFor="sync-sftp-key">{t('sync_sftp_fingerprint')}</label>
                <input
                  id="sync-sftp-key"
                  name="hostKey"
                  placeholder="SHA256:…"
                  maxLength={52}
                  defaultValue={remote?.kind === kind ? remote.hostKey : ''}
                  required
                />
              </>
            )}
            {kind !== 'off' && (
              <>
                <label htmlFor="sync-user">{t('sync_storage_user')}</label>
                <input
                  id="sync-user"
                  name="username"
                  autoComplete="username"
                  maxLength={256}
                  defaultValue={remote?.kind === kind ? remote.username : ''}
                  required={kind === 'sftp'}
                />
                <label htmlFor="sync-password">{t('sync_storage_password')}</label>
                <input
                  id="sync-password"
                  name="password"
                  type="password"
                  autoComplete="off"
                  maxLength={2048}
                  required={kind === 'sftp'}
                />
              </>
            )}
            <button className="secondary-button" disabled={busy || (kind === 'off' && !remote)}>
              {t('sync_save_storage')}
            </button>
          </form>
          {remote && (
            <>
              <button
                className="secondary-button"
                disabled={busy || status.remoteSyncing}
                onClick={() => void update({ action: 'sync_remote_now' })}
              >
                {t('sync_storage_now')}
              </button>
              <p id="sync-storage-progress-label" className="settings-feedback" role="status">
                {progress
                  ? remoteProgressLabel(progress)
                  : status.remoteError
                    ? t(safeError(status.remoteError).message)
                    : status.remoteSyncedRecords === null
                      ? t('sync_storage_pending')
                      : t('sync_storage_confirmed_0', { 0: status.remoteSyncedRecords })}
              </p>
              {progress && (
                <progress
                  className="sync-progress"
                  aria-labelledby="sync-storage-progress-label"
                  value={countedProgress?.processed}
                  max={countedProgress?.total}
                />
              )}
              {status.remoteLastSync && (
                <p className="settings-feedback">
                  {lastSync(status.remoteLastSync)}
                  {(progress || status.remoteError) && status.remoteSyncedRecords !== null
                    ? ` · ${t('sync_storage_confirmed_0', { 0: status.remoteSyncedRecords })}`
                    : ''}
                </p>
              )}
            </>
          )}
          {!!status.localOnlyRecords && (
            <p className="settings-feedback">
              {t('sync_existing_history_local_only_0', { 0: status.localOnlyRecords })}
            </p>
          )}
        </>
      )}
    </section>
  );
}
