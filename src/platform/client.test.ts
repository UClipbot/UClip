import { expect, test, vi } from 'vitest';
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks';
import { desktop, safeError } from './client';

test('unknown errors and server messages never expose arbitrary content', () => {
  expect(safeError(new Error('/private/path token=secret')).message).not.toContain('secret');
  expect(safeError({ code: 'storage', message: 'secret' }).message).not.toContain('secret');
  expect(safeError({ code: 'invented', message: 'secret' }).code).toBe('unavailable');
});

test('version mismatch fails instead of accepting an incompatible snapshot', async () => {
  vi.stubGlobal('window', {});
  mockIPC(() => ({ contractVersion: 999 }));
  try {
    await expect(desktop.bootstrap()).rejects.toMatchObject({ code: 'unavailable' });
  } finally {
    clearMocks();
    vi.unstubAllGlobals();
  }
});
