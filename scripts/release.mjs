import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createReadStream, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const versionFiles = [
  'Cargo.toml',
  'Cargo.lock',
  'package.json',
  'package-lock.json',
  'src-tauri/tauri.conf.json',
];

function run(command, args, capture = false) {
  const result = spawnSync(command, args, {
    cwd: root,
    encoding: 'utf8',
    stdio: capture ? ['ignore', 'pipe', 'inherit'] : 'inherit',
  });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} ${args[0]} exited with ${result.status}`);
  return capture ? result.stdout.trim() : '';
}

const git = (...args) => run('git', args, true);
const read = (path) => readFileSync(join(root, path), 'utf8');
const json = (path) => JSON.parse(read(path));

function verifyVersions(expected) {
  const npmLock = json('package-lock.json');
  const versions = new Map([
    ['package.json', json('package.json').version],
    ['package-lock.json', npmLock.version],
    ['package-lock.json root package', npmLock.packages['']?.version],
    ['src-tauri/tauri.conf.json', json('src-tauri/tauri.conf.json').version],
  ]);
  const metadata = JSON.parse(
    run('cargo', ['metadata', '--locked', '--no-deps', '--format-version', '1'], true),
  );
  const lock = new Map(
    [
      ...read('Cargo.lock').matchAll(
        /^\[\[package\]\]\r?\nname = "([^"]+)"\r?\nversion = "([^"]+)"(?!\r?\nsource =)/gm,
      ),
    ].map(([, name, version]) => [name, version]),
  );
  for (const member of metadata.packages) {
    versions.set(member.manifest_path, member.version);
    versions.set(`Cargo.lock (${member.name})`, lock.get(member.name));
  }
  for (const [path, version] of versions) {
    if (version !== expected)
      throw new Error(`${path} has version ${version}; expected ${expected}.`);
  }
}

function bumpVersion(version, previous) {
  const manifest = read('Cargo.toml');
  const versionLine =
    /(^\[workspace\.package\][\t ]*\r?\n(?:[^\r\n\[]*\r?\n)*?version = ")[^"]+("$)/m;
  const next = manifest.replace(versionLine, (_, before, after) => `${before}${version}${after}`);
  if (manifest === next) throw new Error('Could not update Cargo workspace version.');
  const tauriPath = 'src-tauri/tauri.conf.json';
  const config = json(tauriPath);
  config.version = version;
  run('npm', ['version', version, '--no-git-tag-version', '--ignore-scripts']);
  writeFileSync(join(root, 'Cargo.toml'), next);
  writeFileSync(join(root, tauriPath), `${JSON.stringify(config, null, 2)}\n`);
  const previousLock = read('Cargo.lock');
  run('cargo', ['update', '--workspace']);
  verifyVersions(version);
  const lockedDependencies = (contents) =>
    contents.split('[[package]]').filter((entry) => /^source = /m.test(entry));
  if (
    JSON.stringify(lockedDependencies(previousLock)) !==
    JSON.stringify(lockedDependencies(read('Cargo.lock')))
  ) {
    throw new Error(
      `Updating ${previous} to ${version} also changed third-party Cargo.lock entries.`,
    );
  }
}

async function watchWorkflow(repo, workflow, branch, head) {
  let id;
  for (let attempt = 0; attempt < 60; attempt += 1) {
    id = run(
      'gh',
      [
        'api',
        '--method',
        'GET',
        `repos/${repo}/actions/workflows/${workflow}/runs`,
        '-f',
        `branch=${branch}`,
        '-f',
        'event=push',
        '-f',
        `head_sha=${head}`,
        '-f',
        'per_page=1',
        '--jq',
        '.workflow_runs[0].id // empty',
      ],
      true,
    );
    if (id) break;
    await sleep(5000);
  }
  if (!id) throw new Error(`No ${workflow} run appeared for ${branch} at ${head}.`);
  console.log(`Watching ${workflow} (${branch}): https://github.com/${repo}/actions/runs/${id}`);
  run('gh', ['run', 'watch', id, '--repo', repo, '--interval', '15', '--exit-status']);
}

async function verifyRelease(repo, tag, version) {
  const release = JSON.parse(
    run(
      'gh',
      ['release', 'view', tag, '--repo', repo, '--json', 'url,isDraft,isPrerelease,assets'],
      true,
    ),
  );
  const installers = [
    `UClip_${version}_amd64.deb`,
    `UClip_${version}_amd64.AppImage`,
    `UClip_${version}_x64-setup.exe`,
    `UClip_${version}_aarch64.dmg`,
    `UClip_${version}_x64.dmg`,
  ];
  const checksum = 'SHA256SUMS.txt';
  const names = new Set(release.assets.map((asset) => asset.name));
  if (
    release.isDraft ||
    !release.isPrerelease ||
    names.size !== installers.length + 1 ||
    ![...installers, checksum].every((name) => names.has(name))
  ) {
    throw new Error(
      `Published preview ${tag} is missing expected assets or has the wrong visibility.`,
    );
  }

  const directory = mkdtempSync(join(tmpdir(), 'uclip-release-'));
  try {
    run('gh', ['release', 'download', tag, '--repo', repo, '--dir', directory]);
    const rows = readFileSync(join(directory, checksum), 'utf8').trim().split(/\r?\n/);
    const hashes = new Map(
      rows.map((line) => {
        const match = /^([a-f0-9]{64}) [ *](.+)$/.exec(line);
        if (!match) throw new Error(`Invalid row in ${checksum}.`);
        return [match[2], match[1]];
      }),
    );
    if (rows.length !== installers.length || hashes.size !== installers.length) {
      throw new Error(`${checksum} must list each installer once.`);
    }
    for (const name of installers) {
      const hash = createHash('sha256');
      for await (const chunk of createReadStream(join(directory, name))) hash.update(chunk);
      if (hash.digest('hex') !== hashes.get(name)) throw new Error(`Checksum mismatch: ${name}`);
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
  console.log(`Verified ${installers.length} installers: ${release.url}`);
}

async function release(version) {
  if (git('branch', '--show-current') !== 'main' || git('status', '--porcelain')) {
    throw new Error('Start from a clean main branch. Commit the changes to include first.');
  }
  const origin = git('remote', 'get-url', 'origin');
  const repo =
    /^(?:https:\/\/github\.com\/|git@github\.com:|ssh:\/\/git@github\.com\/)([^/]+\/[^/]+?)(?:\.git)?\/?$/.exec(
      origin,
    )?.[1];
  if (!repo) throw new Error('The origin remote must point to a GitHub repository.');
  run('gh', ['repo', 'view', repo, '--json', 'nameWithOwner'], true);
  run('git', ['fetch', 'origin', 'main']);
  const tag = `v${version}`;
  const packageBranch = `package/${tag}`;
  const packageRef = `refs/heads/${packageBranch}`;
  const tagRef = `refs/tags/${tag}`;
  const remote = new Map(
    git('ls-remote', 'origin', packageRef, tagRef, `${tagRef}^{}`)
      .split('\n')
      .filter(Boolean)
      .map((line) => line.split('\t').reverse()),
  );
  const remoteTag = remote.get(`${tagRef}^{}`) ?? remote.get(tagRef);
  let head = git('rev-parse', 'HEAD');
  const previous = json('package.json').version;
  verifyVersions(previous);
  const localTag = git('tag', '--list', tag) ? git('rev-parse', `${tagRef}^{commit}`) : undefined;
  for (const ref of [remote.get(packageRef), remoteTag, localTag]) {
    if (ref && (ref !== head || previous !== version)) {
      throw new Error(`${tag} is already reserved by a different release commit.`);
    }
  }
  if (!remote.has(packageRef)) run('git', ['merge-base', '--is-ancestor', 'origin/main', 'HEAD']);
  if (previous !== version) {
    if (version.localeCompare(previous, 'en', { numeric: true }) <= 0) {
      throw new Error(`${version} must be newer than ${previous}.`);
    }
    bumpVersion(version, previous);
    run('npm', ['run', 'check']);
    run('git', ['diff', '--check']);
    run('git', ['add', '--', ...versionFiles]);
    run('git', ['commit', '-m', `Release UClip ${version}`]);
    head = git('rev-parse', 'HEAD');
  } else if (!remote.has(packageRef)) {
    run('npm', ['run', 'check']);
  }
  if (!remote.has(packageRef)) {
    run('git', ['push', '--atomic', 'origin', 'HEAD:refs/heads/main', `HEAD:${packageRef}`]);
  }
  if (!remoteTag) {
    await watchWorkflow(repo, 'check.yml', 'main', head);
    await watchWorkflow(repo, 'package.yml', packageBranch, head);
    if (!localTag) run('git', ['tag', '-a', tag, '-m', `UClip ${tag}`]);
    run('git', ['push', 'origin', tagRef]);
  }
  await watchWorkflow(repo, 'package.yml', tag, head);
  await verifyRelease(repo, tag, version);
}

const [version, extra] = process.argv.slice(2);
try {
  if (!version || extra || !/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version)) {
    throw new Error('Usage: npm run release -- <major.minor.patch>');
  }
  await release(version);
} catch (error) {
  console.error(`Release failed: ${error.message}`);
  process.exitCode = 1;
}
