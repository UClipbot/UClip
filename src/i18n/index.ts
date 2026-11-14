import type { Language } from '../generated/contracts';
import { english, chinese } from './messages';

export type Locale = Exclude<Language, 'system'>;
export type Values = Readonly<Record<string, string | number>>;

export function resolveLanguage(preference: Language, languages: readonly string[]): Locale {
  if (preference !== 'system') return preference;
  // The primary OS/browser language decides. All Chinese variants use Simplified Chinese.
  return /^zh(?:[-_]|$)/i.test(languages[0] ?? '') ? 'zh-CN' : 'en';
}

let locale: Locale =
  typeof navigator === 'undefined' ? 'en' : resolveLanguage('system', navigator.languages);
const listeners = new Set<() => void>();
export const getLocale = () => locale;
export function setLocale(value: Locale) {
  if (locale === value) return;
  locale = value;
  for (const listener of listeners) listener();
}
export function subscribeLocale(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function translate(language: Locale, key: string, values: Values = {}): string {
  let message = Object.prototype.hasOwnProperty.call(english, key)
    ? (language === 'en' ? english : chinese)[key as keyof typeof english]
    : key;
  if (language === 'en' && values[0] === 1) {
    if (key === '0_records') message = '{0} record';
    if (key === '0_items') message = '{0} item';
  }
  // Interpolate once: braces in clipboard content or names stay literal.
  return message.replace(/\{(\w+)\}/g, (placeholder, name: string) =>
    Object.prototype.hasOwnProperty.call(values, name) ? String(values[name]) : placeholder,
  );
}
export const t = (key: string, values?: Values) => translate(locale, key, values);

export function formatHistoryDate(timestamp: number, now = new Date()): string {
  const date = new Date(timestamp);
  if (date.toDateString() === now.toDateString()) return t('today');
  const yesterday = new Date(now);
  yesterday.setDate(yesterday.getDate() - 1);
  if (date.toDateString() === yesterday.toDateString()) return t('yesterday');
  return new Intl.DateTimeFormat(locale, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
  }).format(date);
}
export function formatHistoryTime(timestamp: number): string {
  return new Intl.DateTimeFormat(locale, { hour: '2-digit', minute: '2-digit' }).format(timestamp);
}
