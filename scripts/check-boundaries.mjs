import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');
const config = JSON.parse(read('src-tauri/tauri.conf.json'));
const capability = JSON.parse(read('src-tauri/capabilities/main.json'));
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
  commands.map((name) => `allow-${name.replaceAll('_', '-')}`).sort(),
);
assert.deepEqual(config.app.security.capabilities, ['main']);
assert.equal(config.app.withGlobalTauri, false);
assert(config.app.security.csp.includes("frame-src 'none'"));
assert(config.app.security.csp.includes("object-src 'none'"));
assert(!config.app.security.csp.includes('unsafe-eval'));
assert(!config.app.security.csp.includes('unsafe-inline'));
assert.equal(config.app.security.dangerousDisableAssetCspModification, undefined);
console.log('IPC manifest, local window grant and production CSP boundaries checked.');
