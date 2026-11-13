import { describe, expect, it } from 'vitest';
import { bindingError, DEFAULT_KEYS, matchesKey } from './keyboard';
const event = {
  code: 'KeyK',
  ctrlKey: true,
  metaKey: false,
  altKey: false,
  shiftKey: false,
  isComposing: false,
  repeat: false,
};
describe('window shortcut boundaries', () => {
  it('uses the platform primary key and refuses IME and repeated copy actions', () => {
    expect(matchesKey(event, DEFAULT_KEYS.search, false)).toBe(true);
    expect(matchesKey(event, DEFAULT_KEYS.search, true)).toBe(false);
    expect(matchesKey({ ...event, ctrlKey: false, metaKey: true }, DEFAULT_KEYS.search, true)).toBe(
      true,
    );
    expect(matchesKey({ ...event, isComposing: true }, DEFAULT_KEYS.search, false)).toBe(false);
    expect(
      matchesKey({ ...event, code: 'NumpadEnter', ctrlKey: false }, DEFAULT_KEYS.copy, false),
    ).toBe(true);
    expect(
      matchesKey(
        { ...event, code: 'Enter', ctrlKey: false, repeat: true },
        DEFAULT_KEYS.copy,
        false,
      ),
    ).toBe(false);
    expect(
      matchesKey(
        { ...event, code: 'ArrowDown', ctrlKey: false, repeat: true },
        DEFAULT_KEYS.down,
        false,
        true,
      ),
    ).toBe(true);
  });
  it('rejects duplicate assignments, system editing keys and plain typing for global actions', () => {
    expect(bindingError(DEFAULT_KEYS)).toBe('');
    expect(bindingError({ ...DEFAULT_KEYS, preview: DEFAULT_KEYS.copy })).toContain('重复');
    expect(
      bindingError({ ...DEFAULT_KEYS, copy: { ...DEFAULT_KEYS.search, code: 'KeyC' } }),
    ).toContain('系统文本编辑');
    expect(
      bindingError({ ...DEFAULT_KEYS, search: { ...DEFAULT_KEYS.copy, code: 'Space' } }),
    ).toContain('Ctrl/Cmd');
  });
});
