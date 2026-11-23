import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { buildArguments, checkVersions, cliTargets, packageOptions } from './package.mjs';

test('native hosts select supported installer defaults and lock Cargo dependencies', () => {
  for (const [platform, bundles] of [
    ['linux', ['deb', 'appimage']],
    ['win32', ['nsis']],
    ['darwin', ['app', 'dmg']],
  ]) {
    const options = packageOptions([], platform);
    assert.deepEqual(options.bundles, bundles);
    const args = buildArguments(options);
    assert.deepEqual(args.slice(-2), ['--', '--locked']);
    const config = JSON.parse(args[args.indexOf('--config') + 1]);
    assert.deepEqual(config.bundle, {
      active: true,
      externalBin: ['../target/uclip-package/uclip'],
    });
    assert.ok(!args.includes('--debug'));
    assert.ok(!args.includes('src-tauri/tauri.dev.conf.json'));
  }
});

test('debug installers isolate their identifier, and same-OS targets reach Tauri', () => {
  const args = buildArguments(
    packageOptions(['--debug', '--bundles', 'dmg', '--target', 'universal-apple-darwin'], 'darwin'),
  );
  assert.ok(args.includes('--debug'));
  assert.ok(args.includes('src-tauri/tauri.dev.conf.json'));
  assert.equal(args[args.indexOf('--target') + 1], 'universal-apple-darwin');
  assert.deepEqual(cliTargets('universal-apple-darwin'), [
    'x86_64-apple-darwin',
    'aarch64-apple-darwin',
  ]);
  assert.deepEqual(cliTargets('x86_64-pc-windows-msvc'), ['x86_64-pc-windows-msvc']);
});

test('Windows installers embed WebView2 and statically link the MSVC runtime', () => {
  const config = JSON.parse(
    readFileSync(new URL('../src-tauri/tauri.conf.json', import.meta.url), 'utf8'),
  );
  assert.equal(config.bundle.windows.webviewInstallMode.type, 'offlineInstaller');
  assert.equal(config.build.windows.staticVCRuntime, true);
});

test('invalid input fails before compiling or silently changing the requested format', () => {
  for (const argv of [
    ['--bundles', 'nsis'],
    ['--bundles', 'deb,'],
    ['--bundles', 'all'],
    ['--bundles'],
    ['--target', '--debug'],
    ['--target', 'x86_64-pc-windows-msvc'],
    ['--config', 'src-tauri/tauri.dev.conf.json'],
    ['--no-bundle'],
    ['--debug', '--debug'],
  ]) {
    assert.throws(() => packageOptions(argv, 'linux'));
  }
  assert.throws(() => packageOptions([], 'freebsd'));
  assert.deepEqual(packageOptions(['--bundles', 'msi,nsis,msi'], 'win32').bundles, ['msi', 'nsis']);
});

test('mismatched application versions stop packaging', () => {
  checkVersions('0.1.0', '0.1.0', '0.1.0', '0.1.0');
  for (let i = 0; i < 4; i += 1) {
    const versions = ['0.1.0', '0.1.0', '0.1.0', '0.1.0'];
    versions[i] = '0.2.0';
    assert.throws(() => checkVersions(...versions), /Version mismatch/);
  }
});

test('AppImage rejects cross-CPU packaging while deb allows a configured cross toolchain', () => {
  assert.throws(
    () => packageOptions(['--target', 'aarch64-unknown-linux-gnu'], 'linux', 'x64'),
    /matching CPU/,
  );
  assert.doesNotThrow(() =>
    packageOptions(['--target', 'aarch64-unknown-linux-gnu'], 'linux', 'arm64'),
  );
  assert.doesNotThrow(() =>
    packageOptions(['--bundles', 'deb', '--target', 'aarch64-unknown-linux-gnu'], 'linux', 'x64'),
  );
});

test('CLI dry-run works outside the repository without starting a build', () => {
  const result = spawnSync(
    process.execPath,
    [
      fileURLToPath(new URL('./package.mjs', import.meta.url)),
      '--bundles',
      process.platform === 'win32' ? 'nsis' : process.platform === 'darwin' ? 'dmg' : 'deb',
      '--dry-run',
    ],
    { cwd: tmpdir(), encoding: 'utf8' },
  );
  assert.equal(result.status, 0, result.stderr);
  const command = JSON.parse(result.stdout);
  assert.equal(command[0], process.execPath);
  assert.deepEqual(command.slice(-2), ['--', '--locked']);
});

test('CLI invalid options return nonzero with an actionable error', () => {
  const result = spawnSync(
    process.execPath,
    [fileURLToPath(new URL('./package.mjs', import.meta.url)), '--no-bundle'],
    { encoding: 'utf8' },
  );
  assert.equal(result.status, 1);
  assert.match(result.stderr, /Unknown option: --no-bundle/);
});
