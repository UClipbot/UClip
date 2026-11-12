import { invoke, isTauri } from '@tauri-apps/api/core';
import type {
  AppError,
  BootstrapSnapshot,
  ErrorCode,
  SettingsSnapshot,
  UpdateSettings,
} from '../generated/contracts';

const codes: ReadonlySet<ErrorCode> = new Set([
  'storage',
  'unsupported_schema',
  'foreign_database',
  'conflict',
  'invalid_input',
  'busy',
  'unavailable',
]);

export function safeError(error: unknown): AppError {
  if (
    typeof error === 'object' &&
    error !== null &&
    'code' in error &&
    codes.has(error.code as ErrorCode)
  ) {
    // Never display unknown renderer/plugin errors or stringify arbitrary objects.
    const messages: Record<ErrorCode, string> = {
      storage: '本地保存失败，请检查磁盘空间和权限。',
      unsupported_schema: '请使用较新版本打开这些数据。',
      foreign_database: '目标文件不属于此应用。',
      conflict: '设置已在其他位置变更，请刷新后重试。',
      invalid_input: '请求参数无效。',
      busy: '本地服务繁忙，请稍后重试。',
      unavailable: '本地服务不可用，请重启桌面应用。',
    };
    const code = error.code as ErrorCode;
    return { code, message: messages[code] };
  }
  return { code: 'unavailable', message: '无法连接桌面服务。请通过 Tauri 启动应用。' };
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) throw safeError(null);
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    throw safeError(error);
  }
}

export const desktop = {
  async bootstrap(): Promise<BootstrapSnapshot> {
    const snapshot = await call<BootstrapSnapshot>('bootstrap');
    if (snapshot.contractVersion !== 1) {
      throw {
        code: 'unavailable',
        message: '界面与桌面服务版本不匹配，请重启或重新安装。',
      } satisfies AppError;
    }
    return snapshot;
  },
  settings: () => call<SettingsSnapshot>('get_settings'),
  updateSettings: (request: UpdateSettings) =>
    call<SettingsSnapshot>('update_settings', { request }),
};
