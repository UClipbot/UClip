import { invoke } from '@tauri-apps/api/core';
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
import type {
  CopyResult,
  ErrorCode,
  QuickPasteSettings,
  QuickPasteSnapshot,
  UpdateQuickPastePreferences,
} from '../generated/contracts';

export const readQuickPaste = () => invoke<QuickPasteSnapshot | null>('quick_paste_snapshot');
export const selectQuickPaste = (token: string, slot: number) =>
  invoke<CopyResult>('select_quick_paste', { token, slot });
export const expandQuickPaste = (token: string) => invoke('expand_quick_paste', { token });
export const cancelQuickPaste = (token: string) => invoke('cancel_quick_paste', { token });
export const onQuickPasteChanged = (refresh: () => void) =>
  getCurrentWebviewWindow().listen('quick-paste-changed', refresh);
export const onQuickPasteUnavailable = (report: (error: { code: ErrorCode }) => void) =>
  getCurrentWebviewWindow().listen<ErrorCode>('quick-paste-unavailable', ({ payload }) =>
    report({ code: payload }),
  );
export const getQuickPasteSettings = () => invoke<QuickPasteSettings>('quick_paste_settings');
export const updateQuickPasteSettings = (request: UpdateQuickPastePreferences) =>
  invoke<QuickPasteSettings>('update_quick_paste_settings', { request });
