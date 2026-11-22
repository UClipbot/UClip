import { invoke } from '@tauri-apps/api/core';
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';

export const setGlobalShortcutRecording = (token: number, recording: boolean) =>
  invoke<void>('set_global_shortcut_recording', { token, recording });

export const onGlobalShortcutCapture = (capture: (token: number, binding: string | null) => void) =>
  getCurrentWebviewWindow().listen<[number, string | null]>(
    'global-shortcut-capture',
    ({ payload }) => capture(...payload),
  );

const keyNames = new Map(
  Object.entries({
    Backquote: '`',
    Backslash: '\\',
    BracketLeft: '[',
    BracketRight: ']',
    Comma: ',',
    Equal: '=',
    Minus: '-',
    Period: '.',
    Quote: "'",
    Semicolon: ';',
    Slash: '/',
    ArrowDown: '↓',
    ArrowLeft: '←',
    ArrowRight: '→',
    ArrowUp: '↑',
    Escape: 'Esc',
  }).map(([code, name]) => [code.toLowerCase(), name]),
);

function shortKey(code: string) {
  return code.replace(/^(Key(?=[A-Z]$)|Digit(?=[0-9]$))/, '');
}

export function globalShortcutKeyCode(event: Pick<KeyboardEvent, 'key' | 'code'>, os: string) {
  // X11 and Windows use layout-dependent letters. macOS registers physical key positions.
  return os !== 'macos' && /^[a-zA-Z]$/.test(event.key)
    ? `Key${event.key.toUpperCase()}`
    : event.code;
}

export function formatGlobalShortcut(binding: string, os: string) {
  const parts = binding.split('+').map((part) => part.trim());
  const key = parts.pop() ?? '';
  const modifiers = new Set(parts.map((part) => part.toLowerCase()));
  const has = (...names: string[]) => names.some((name) => modifiers.has(name));
  const primary = has('commandorcontrol', 'commandorctrl', 'cmdorcontrol', 'cmdorctrl');
  const labels = [
    ...(has('control', 'ctrl') || (primary && os !== 'macos') ? ['Ctrl'] : []),
    ...(has('alt', 'option') ? [os === 'macos' ? 'Option' : 'Alt'] : []),
    ...(has('shift') ? ['Shift'] : []),
    ...(has('super', 'command', 'cmd') || (primary && os === 'macos')
      ? [os === 'macos' ? 'Cmd' : os === 'windows' ? 'Win' : 'Super']
      : []),
  ];
  const name = keyNames.get(key.toLowerCase());
  return [...labels, name ?? shortKey(key)].join(' + ');
}

export function recordGlobalShortcut(event: KeyboardEvent, os: string) {
  if (event.isComposing || event.keyCode === 229)
    return { error: 'finish_composing_text_first' } as const;
  if (event.repeat || /^(Control|Shift|Alt|Meta)(Left|Right)$/.test(event.code)) return;
  if (event.getModifierState('AltGraph'))
    return { error: 'this_key_is_not_supported_yet' } as const;
  if (!event.ctrlKey && !event.altKey && !event.metaKey)
    return { error: 'global_shortcut_requires_modifier' } as const;
  // Use the key codes accepted by the native accelerator parser. Registration still validates
  // whether a key and its modifiers are available on the current platform.
  if (
    !keyNames.has(event.code.toLowerCase()) &&
    !/^(Key[A-Z]|Digit[0-9]|F([1-9]|1[0-9]|2[0-4])|Numpad([0-9]|Add|Decimal|Divide|Enter|Equal|Multiply|Subtract)|Backspace|Enter|Space|Tab|Delete|End|Home|Insert|PageDown|PageUp|PrintScreen|Pause|CapsLock|NumLock|ScrollLock|AudioVolume(Down|Up|Mute)|Media(Play|Pause|PlayPause|Stop|TrackNext|TrackPrevious))$/.test(
      event.code,
    )
  )
    return { error: 'this_key_is_not_supported_yet' } as const;
  const key = shortKey(globalShortcutKeyCode(event, os));
  return {
    binding: [
      ...(event.ctrlKey ? ['Control'] : []),
      ...(event.altKey ? ['Alt'] : []),
      ...(event.shiftKey ? ['Shift'] : []),
      ...(event.metaKey ? ['Super'] : []),
      key,
    ].join('+'),
  } as const;
}
