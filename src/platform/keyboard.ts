import type { KeyBindings, KeyChord } from '../generated/contracts';
const plain = (code: string): KeyChord => ({ code, primary: false, alt: false, shift: false });
export const DEFAULT_KEYS: KeyBindings = {
  search: { ...plain('KeyK'), primary: true },
  settings: { ...plain('Comma'), primary: true },
  copy: plain('Enter'),
  preview: plain('Space'),
  remove: plain('Delete'),
  up: plain('ArrowUp'),
  down: plain('ArrowDown'),
  dismiss: plain('Escape'),
};
export const keyLabels: Record<keyof KeyBindings, string> = {
  search: '聚焦搜索',
  settings: '设置',
  copy: '复制',
  preview: '查看完整内容',
  remove: '删除记录',
  up: '上一条',
  down: '下一条',
  dismiss: '返回 / 清空 / 隐藏',
};
export function matchesKey(
  event: Pick<
    KeyboardEvent,
    'code' | 'ctrlKey' | 'metaKey' | 'altKey' | 'shiftKey' | 'isComposing' | 'repeat'
  >,
  chord: KeyChord,
  mac: boolean,
  allowRepeat = false,
) {
  return (
    !event.isComposing &&
    (!event.repeat || allowRepeat) &&
    (event.code === 'NumpadEnter' ? 'Enter' : event.code) === chord.code &&
    (mac ? event.metaKey : event.ctrlKey) === chord.primary &&
    !(mac ? event.ctrlKey : event.metaKey) &&
    event.altKey === chord.alt &&
    event.shiftKey === chord.shift
  );
}
export function formatKey(chord: KeyChord, primary = 'Ctrl') {
  const names: Record<string, string> = {
    Comma: ',',
    Period: '.',
    Slash: '/',
    Space: 'Space',
    ArrowUp: '↑',
    ArrowDown: '↓',
    Escape: 'Esc',
  };
  return [
    ...(chord.primary ? [primary] : []),
    ...(chord.alt ? ['Alt'] : []),
    ...(chord.shift ? ['Shift'] : []),
    names[chord.code] ?? chord.code.replace(/^Key|^Digit/, ''),
  ].join(' + ');
}
export function bindingError(bindings: KeyBindings): string {
  const seen = new Set<string>();
  for (const action of Object.keys(keyLabels) as (keyof KeyBindings)[]) {
    const chord = bindings[action];
    if (
      ['search', 'settings', 'dismiss'].includes(action) &&
      !(action === 'dismiss' && chord.code === 'Escape') &&
      !chord.primary &&
      !/^F([1-9]|1[0-2])$/.test(chord.code)
    )
      return '聚焦搜索、设置和返回需要 Ctrl/Cmd 组合或功能键。';
    const key = JSON.stringify([chord.code, chord.primary, chord.alt, chord.shift]);
    if (seen.has(key)) return `「${keyLabels[action]}」与其他操作重复。`;
    seen.add(key);
    if (
      (chord.code === 'Escape' && action !== 'dismiss') ||
      (chord.primary &&
        [
          'KeyA',
          'KeyC',
          'KeyV',
          'KeyX',
          'KeyZ',
          'KeyY',
          'KeyQ',
          'KeyW',
          'KeyH',
          'Backspace',
          'Delete',
        ].includes(chord.code)) ||
      (chord.alt && chord.code === 'F4')
    )
      return '此组合用于系统文本编辑或窗口操作，请选择其他按键。';
    if (
      !chord.primary &&
      !chord.alt &&
      !['Enter', 'Space', 'Escape', 'Delete', 'Backspace', 'ArrowUp', 'ArrowDown'].includes(
        chord.code,
      ) &&
      !/^F([1-9]|1[0-2])$/.test(chord.code)
    )
      return '字母和数字需要搭配 Ctrl/Cmd 或 Alt，避免影响输入。';
    if (
      !/^(Key[A-Z]|Digit[0-9]|F([1-9]|1[0-2])|Enter|Space|Escape|Delete|Backspace|ArrowUp|ArrowDown|Comma|Period|Slash)$/.test(
        chord.code,
      )
    )
      return '此按键暂不支持。';
  }
  return '';
}
