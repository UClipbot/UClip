import { t } from '../i18n';
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
  search: 'focus_search',
  settings: 'settings',
  copy: 'copy',
  preview: 'view_full_content',
  remove: 'delete_record',
  up: 'previous_record',
  down: 'next_record',
  dismiss: 'back_clear_hide',
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
      return t('search_settings_and_back_require_ctrl_cmd_or_a_function_key');
    const key = JSON.stringify([chord.code, chord.primary, chord.alt, chord.shift]);
    if (seen.has(key)) return t('0_conflicts_with_another_action', { 0: t(keyLabels[action]) });
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
      return t(
        'this_combination_is_used_for_system_text_editing_or_window_operations_choose_different_keys',
      );
    if (
      !chord.primary &&
      !chord.alt &&
      !['Enter', 'Space', 'Escape', 'Delete', 'Backspace', 'ArrowUp', 'ArrowDown'].includes(
        chord.code,
      ) &&
      !/^F([1-9]|1[0-2])$/.test(chord.code)
    )
      return t('letters_and_numbers_require_ctrl_cmd_or_alt_to_avoid_interfering_with_typing');
    if (
      !/^(Key[A-Z]|Digit[0-9]|F([1-9]|1[0-2])|Enter|Space|Escape|Delete|Backspace|ArrowUp|ArrowDown|Comma|Period|Slash)$/.test(
        chord.code,
      )
    )
      return t('this_key_is_not_supported_yet');
  }
  return '';
}
