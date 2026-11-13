import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const cli = fileURLToPath(new URL('../node_modules/@tauri-apps/cli/tauri.js', import.meta.url));
const platforms = {
  linux: { defaults: ['deb', 'appimage'], bundles: ['deb', 'appimage', 'rpm'] },
  win32: { defaults: ['nsis'], bundles: ['nsis', 'msi'] },
  darwin: { defaults: ['app', 'dmg'], bundles: ['app', 'dmg'] },
};

const help = `Usage: npm run package -- [options]

Build installers on the current operating system (release by default).
  --bundles <list>  Comma-separated formats; Linux: deb,appimage,rpm;
                    Windows: nsis,msi; macOS: app,dmg
  --target <triple> Rust target on the same OS (or universal-apple-darwin)
  --debug           Debug build with the isolated development app identifier
  --dry-run         Print the command without building
  --help            Show this help

Defaults: Linux deb,appimage; Windows nsis; macOS app,dmg.
Install dependencies with npm ci first. Cargo always uses --locked.
Outputs: target/[<target>/]<release|debug>/bundle/ (unless Cargo overrides it).
`;

export function packageOptions(argv, platform = process.platform, arch = process.arch) {
  const supported = platforms[platform];
  if (!supported) throw new Error(`Unsupported packaging host: ${platform}`);
  const options = { bundles: supported.defaults, debug: false, dryRun: false };
  const seen = new Set();
  for (let i = 0; i < argv.length; i += 1) {
    const flag = argv[i];
    if (seen.has(flag)) throw new Error(`Repeated option: ${flag}`);
    seen.add(flag);
    switch (flag) {
      case '--debug':
        options.debug = true;
        break;
      case '--dry-run':
        options.dryRun = true;
        break;
      case '--bundles':
      case '--target': {
        const value = argv[++i];
        if (!value || value.startsWith('-')) throw new Error(`Missing value for ${flag}`);
        if (flag === '--bundles') options.bundles = value.split(',');
        else options.target = value;
        break;
      }
      default:
        throw new Error(`Unknown option: ${flag}. Use --help for supported options.`);
    }
  }
  if (options.bundles.some((bundle) => !supported.bundles.includes(bundle))) {
    throw new Error(`Bundle formats on ${platform}: ${supported.bundles.join(',')}`);
  }
  options.bundles = [...new Set(options.bundles)];
  if (options.target) {
    const patterns = {
      linux: /^(x86_64|aarch64)-unknown-linux-gnu$/,
      win32: /^(x86_64|aarch64|i686)-pc-windows-msvc$/,
      darwin: /^(x86_64|aarch64|universal)-apple-darwin$/,
    };
    if (!patterns[platform].test(options.target)) {
      throw new Error(
        `Unsupported target ${options.target} on ${platform}; use a native desktop target.`,
      );
    }
    // linuxdeploy does not support packaging AppImages for a different CPU.
    const nativeCpu = { x64: 'x86_64', arm64: 'aarch64' }[arch];
    if (
      platform === 'linux' &&
      options.bundles.includes('appimage') &&
      !options.target.startsWith(`${nativeCpu}-`)
    ) {
      throw new Error('AppImage must be packaged on the matching CPU architecture.');
    }
  }
  return options;
}

export function buildArguments(options) {
  const args = ['build', '--ci'];
  if (options.debug) args.push('--debug', '--config', 'src-tauri/tauri.dev.conf.json');
  args.push('--config', JSON.stringify({ bundle: { active: true } }));
  args.push('--bundles', options.bundles.join(','));
  if (options.target) args.push('--target', options.target);
  args.push('--', '--locked');
  return args;
}

export function checkVersions(npmVersion, lockVersion, appVersion, rustVersion) {
  if (![lockVersion, appVersion, rustVersion].every((version) => version === npmVersion)) {
    throw new Error(
      `Version mismatch: package.json=${npmVersion}, package-lock.json=${lockVersion}, tauri.conf.json=${appVersion}, Cargo workspace=${rustVersion}. Update versions together.`,
    );
  }
}

function main() {
  if (process.argv.includes('--help')) {
    process.stdout.write(help);
    return 0;
  }
  const options = packageOptions(process.argv.slice(2));
  const args = buildArguments(options);
  if (options.dryRun) {
    // JSON preserves spaces and quotes on every host; this is a preview, not shell code.
    console.log(JSON.stringify([process.execPath, cli, ...args], null, 2));
    return 0;
  }
  if (!existsSync(cli)) throw new Error('Local Tauri CLI is missing. Run npm ci first.');
  const metadata = spawnSync(
    'cargo',
    ['metadata', '--no-deps', '--format-version', '1', '--locked'],
    {
      cwd: root,
      encoding: 'utf8',
    },
  );
  if (metadata.error) throw metadata.error;
  if (metadata.status !== 0) {
    process.stderr.write(metadata.stderr);
    return metadata.status ?? 1;
  }
  const json = (path) => JSON.parse(readFileSync(new URL(path, pathToFileURL(root)), 'utf8'));
  const rustPackage = JSON.parse(metadata.stdout).packages.find(
    (pkg) => pkg.name === 'uclip-desktop',
  );
  checkVersions(
    json('package.json').version,
    json('package-lock.json').packages[''].version,
    json('src-tauri/tauri.conf.json').version,
    rustPackage?.version,
  );
  console.log(
    `Packaging ${options.bundles.join(', ')} (${options.debug ? 'debug/dev' : 'release'})…`,
  );
  // Invoke the local JS entry point directly: no shell quoting or Windows npm.cmd required.
  const result = spawnSync(process.execPath, [cli, ...args], { cwd: root, stdio: 'inherit' });
  if (result.error) throw result.error;
  return result.status ?? 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    process.exitCode = main();
  } catch (error) {
    console.error(`Packaging failed: ${error.message}`);
    process.exitCode = 1;
  }
}
