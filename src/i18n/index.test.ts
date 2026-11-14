import { afterEach, expect, test } from 'vitest';
import {
  formatHistoryDate,
  formatHistoryTime,
  getLocale,
  resolveLanguage,
  setLocale,
  subscribeLocale,
  translate,
} from './index';
import { english, chinese } from './messages';
import { safeError } from '../platform/client';

afterEach(() => setLocale('en'));

test('explicit preferences win; system Chinese variants resolve and other languages fall back', () => {
  expect(resolveLanguage('en', ['zh-CN'])).toBe('en');
  expect(resolveLanguage('zh-CN', ['en-US'])).toBe('zh-CN');
  for (const language of ['zh-CN', 'zh-TW', 'zh-Hans-CN', 'ZH_hk'])
    expect(resolveLanguage('system', [language])).toBe('zh-CN');
  for (const languages of [[], ['fr-FR'], ['en-US', 'zh-CN']])
    expect(resolveLanguage('system', languages)).toBe('en');
});

test('messages preserve placeholders and interpolate user values only once', () => {
  for (const [key, value] of Object.entries(chinese)) {
    const source = english[key as keyof typeof english];
    expect(value.trim()).not.toBe('');
    expect(source).not.toMatch(/[\u3400-\u9fff]/u);
    expect([...source.matchAll(/\{\w+\}/g)].map((m) => m[0]).sort()).toEqual(
      [...value.matchAll(/\{\w+\}/g)].map((m) => m[0]).sort(),
    );
  }
  expect(translate('en', 'sample_moved_to_0', { 0: '<script>{0}</script>' })).toBe(
    'Sample moved to “<script>{0}</script>”',
  );
  expect(translate('zh-CN', 'sample_moved_to_0', { 0: '\u5de5\u4f5c' })).toBe(
    '\u793a\u4f8b\u5df2\u79fb\u5165「\u5de5\u4f5c」',
  );
  expect(translate('en', '0_records', { 0: 1 })).toBe('1 record');
  expect(translate('en', '0_records', { 0: 2 })).toBe('2 records');
  for (const content of ['custom user content', 'constructor', 'toString', '__proto__'])
    expect(translate('en', content)).toBe(content);
});

test('locale subscriptions update all mounted consumers and can unsubscribe', () => {
  let calls = 0;
  const stop = subscribeLocale(() => {
    calls++;
  });
  setLocale('zh-CN');
  expect(getLocale()).toBe('zh-CN');
  setLocale('zh-CN');
  expect(calls).toBe(1);
  stop();
  setLocale('en');
  expect(calls).toBe(1);
});

test('dates use local calendar boundaries and older history shows an actual date', () => {
  const now = new Date(2026, 0, 1, 0, 30);
  const yesterday = new Date(2025, 11, 31, 23, 30).getTime();
  const older = new Date(2025, 11, 30, 12).getTime();
  setLocale('en');
  expect(formatHistoryDate(now.getTime(), now)).toBe('Today');
  expect(formatHistoryDate(yesterday, now)).toBe('Yesterday');
  expect(formatHistoryDate(older, now)).toBe('Dec 30, 2025');
  expect(formatHistoryTime(older)).toBe(
    new Intl.DateTimeFormat('en', { hour: '2-digit', minute: '2-digit' }).format(older),
  );
  setLocale('zh-CN');
  expect(formatHistoryDate(yesterday, now)).toBe('\u6628\u5929');
  expect(formatHistoryDate(older, now)).toContain('2025');
});

test('safe errors translate by known code without showing server content', () => {
  const error = safeError({ code: 'storage', message: 'secret /private/path' });
  expect(translate('en', error.message)).toBe(
    'Could not save locally. Check disk space and permissions.',
  );
  expect(translate('zh-CN', error.message)).toContain('\u672c\u5730\u4fdd\u5b58\u5931\u8d25');
});
