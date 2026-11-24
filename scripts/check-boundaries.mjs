import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');
const config = JSON.parse(read('src-tauri/tauri.conf.json'));
const capability = JSON.parse(read('src-tauri/capabilities/main.json'));
const settingsCapability = JSON.parse(read('src-tauri/capabilities/settings.json'));
const previewCapability = JSON.parse(read('src-tauri/capabilities/hover-preview.json'));
const quickCapability = JSON.parse(read('src-tauri/capabilities/quick-paste.json'));
const auxiliaryResize = 'resize_auxiliary_corner';
const settingsResize = ['settings_resize_available', 'start_settings_resize'];
const quickSessionCommands = [
  'quick_paste_snapshot',
  'select_quick_paste',
  'expand_quick_paste',
  'cancel_quick_paste',
];
const registered = read('src-tauri/src/lib.rs').match(/generate_handler!\[([\s\S]*?)\]/)?.[1];
const manifested = read('src-tauri/build.rs').match(/\.commands\(&\[([\s\S]*?)\]\)/)?.[1];
assert(registered && manifested, 'Command registration must remain inspectable.');
const commands = registered
  .split(',')
  .map((part) => part.trim().split('::').at(-1))
  .filter(Boolean)
  .sort();
const aclCommands = [...manifested.matchAll(/"([a-z_]+)"/g)].map((match) => match[1]).sort();
assert.deepEqual(
  commands,
  aclCommands,
  'Every registered application command must enter the ACL manifest.',
);
assert.deepEqual(capability.windows, ['main']);
assert.equal(capability.local, true);
assert.equal(capability.remote, undefined);
assert.deepEqual(
  capability.permissions.toSorted(),
  [
    ...commands
      .filter(
        (name) =>
          ![
            'get_hover_preview',
            'hover_preview_presence',
            'set_global_shortcut_recording',
            'set_hardware_acceleration',
            'update_sync',
            'sync_pairing_code',
            auxiliaryResize,
            ...settingsResize,
            ...quickSessionCommands,
          ].includes(name),
      )
      .map((name) => `allow-${name.replaceAll('_', '-')}`),
    'core:event:allow-listen',
    'core:event:allow-unlisten',
  ].sort(),
);
assert.deepEqual(config.app.security.capabilities, [
  'main',
  'settings',
  'hover-preview',
  'quick-paste',
]);
assert.deepEqual(settingsCapability.windows, ['settings']);
assert.equal(settingsCapability.local, true);
assert.equal(settingsCapability.remote, undefined);
assert.deepEqual(
  settingsCapability.permissions.toSorted(),
  [
    'core:event:allow-listen',
    'core:event:allow-unlisten',
    'allow-bootstrap',
    'allow-resize-auxiliary-corner',
    ...settingsResize.map((name) => `allow-${name.replaceAll('_', '-')}`),
    'allow-update-settings',
    'allow-sync-status',
    'allow-update-sync',
    'allow-sync-pairing-code',
    'allow-clipboard-status',
    'allow-update-capture-policy',
    'allow-desktop-status',
    'allow-set-hardware-acceleration',
    'allow-update-desktop-preferences',
    'allow-set-global-shortcut-recording',
    'allow-quick-paste-settings',
    'allow-update-quick-paste-settings',
    'allow-update-key-bindings',
    'allow-set-autostart',
    'allow-close-main-window',
  ].toSorted(),
);
assert.deepEqual(previewCapability.windows, ['hover-preview']);
assert.equal(previewCapability.local, true);
assert.equal(previewCapability.remote, undefined);
assert.deepEqual(
  previewCapability.permissions.toSorted(),
  [
    'core:event:allow-listen',
    'core:event:allow-unlisten',
    'allow-bootstrap',
    'allow-clip-detail',
    'allow-get-hover-preview',
    'allow-hover-preview-presence',
    'allow-hide-hover-preview',
  ].toSorted(),
);
assert.deepEqual(quickCapability.windows, ['quick-paste']);
assert.equal(quickCapability.local, true);
assert.equal(quickCapability.remote, undefined);
assert.deepEqual(
  quickCapability.permissions.toSorted(),
  [
    'core:event:allow-listen',
    'core:event:allow-unlisten',
    'allow-bootstrap',
    'allow-resize-auxiliary-corner',
    ...quickSessionCommands.map((name) => `allow-${name.replaceAll('_', '-')}`),
  ].toSorted(),
);
assert.equal(config.app.withGlobalTauri, false);
assert(config.app.security.csp.includes("frame-src 'none'"));
assert(config.app.security.csp.includes("object-src 'none'"));
assert(!config.app.security.csp.includes('unsafe-eval'));
assert(!config.app.security.csp.includes('unsafe-inline'));
assert.equal(config.app.security.dangerousDisableAssetCspModification, undefined);
console.log('IPC manifest, local window grant and production CSP boundaries checked.');
