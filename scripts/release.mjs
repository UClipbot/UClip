import { spawnSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  findPackageTarget,
  githubOrigin,
  isReleaseVersion,
  packageCommitSubject,
  verifyRelease,
  watchWorkflow,
} from './release-ci.mjs';

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

function preparePackageCommit(version) {
  run('git', ['merge-base', '--is-ancestor', 'origin/main', 'HEAD']);
  const previous = json('package.json').version;
  const subject = packageCommitSubject(version);
  verifyVersions(previous);
  if (previous !== version) {
    if (version.localeCompare(previous, 'en', { numeric: true }) <= 0) {
      throw new Error(`${version} must be newer than ${previous}.`);
    }
    bumpVersion(version, previous);
  } else if (git('log', '-1', '--format=%s') !== subject) {
    throw new Error(`No packaging commit for ${version}. Choose a newer version.`);
  }
  run('npm', ['run', 'check']);
  if (previous !== version) {
    run('git', ['diff', '--check']);
    run('git', ['add', '--', ...versionFiles]);
    run('git', ['commit', '-m', subject]);
  }
  return { ref: 'main', head: git('rev-parse', 'HEAD') };
}

async function release(version, packageOnly) {
  if (git('branch', '--show-current') !== 'main' || git('status', '--porcelain')) {
    throw new Error('Start from a clean main branch. Commit the changes to include first.');
  }
  const repo = githubOrigin();
  run('gh', ['repo', 'view', repo, '--json', 'nameWithOwner'], true);
  run('git', ['fetch', 'origin', 'main']);
  const tag = `v${version}`;
  const tagRef = `refs/tags/${tag}`;
  const publishBranch = `publish/${tag}`;
  const publishRef = `refs/heads/${publishBranch}`;
  const remote = new Map(
    git('ls-remote', 'origin', tagRef, `${tagRef}^{}`, publishRef)
      .split('\n')
      .filter(Boolean)
      .map((line) => line.split('\t').reverse()),
  );
  const remoteTag = remote.get(`${tagRef}^{}`) ?? remote.get(tagRef);
  const remoteRequest = remote.get(publishRef);
  const target = findPackageTarget(version);
  const localTag = git('tag', '--list', tag) ? git('rev-parse', `${tagRef}^{commit}`) : undefined;
  for (const ref of [remoteTag, remoteRequest, localTag]) {
    if (ref && ref !== target?.head) {
      throw new Error(`${tag} is already reserved by a different release commit.`);
    }
  }

  if (target) {
    run('git', ['merge-base', '--is-ancestor', target.head, 'origin/main']);
    const actual = JSON.parse(git('show', `${target.head}:package.json`)).version;
    if (actual !== version)
      throw new Error(`Packaging commit has version ${actual}; expected ${version}.`);
  }
  const { ref, head } = target ?? preparePackageCommit(version);
  const requestRelease =
    !packageOnly && ref === 'main' && !remoteRequest && !remoteTag && !localTag;
  const updates = [];
  if (!target) updates.push(`${head}:refs/heads/main`);
  if (requestRelease) updates.push(`${head}:${publishRef}`);
  if (updates.length) {
    const lease = requestRelease ? [`--force-with-lease=${publishRef}:`] : [];
    run('git', ['push', '--atomic', ...lease, 'origin', ...updates]);
  }

  if (packageOnly) {
    await watchWorkflow(repo, ref, head);
    if (!remoteRequest && !remoteTag) {
      console.log(`Installers verified. To publish them, run npm run release -- ${version}.`);
    }
    return;
  }

  if (requestRelease || remoteRequest) {
    console.log(`GitHub Actions will publish ${tag} from ${head} after packaging passes.`);
    await watchWorkflow(repo, publishBranch, head);
    await verifyRelease(repo, tag, version);
    return;
  }

  // Older package branches and existing local tags use the tag-triggered release.
  if (!remoteTag) {
    if (ref !== 'main') await watchWorkflow(repo, 'main', head, { workflow: 'check.yml' });
    await watchWorkflow(repo, ref, head);
    if (!localTag) run('git', ['tag', '-a', '-m', `UClip ${tag}`, tag, head]);
    const tagObject = git('rev-parse', tagRef);
    if (git('rev-parse', `${tagObject}^{commit}`) !== head) {
      throw new Error(`${tag} no longer points to the verified commit ${head}.`);
    }
    run('git', ['push', 'origin', `${tagObject}:${tagRef}`]);
  }
  await watchWorkflow(repo, tag, head);
  await verifyRelease(repo, tag, version);
}

const [version, flag, extra] = process.argv.slice(2);
try {
  if (!isReleaseVersion(version) || (flag !== undefined && flag !== '--package-only') || extra) {
    throw new Error('Usage: npm run release -- <major.minor.patch> [--package-only]');
  }
  await release(version, flag === '--package-only');
} catch (error) {
  console.error(`Release failed: ${error.message}`);
  process.exitCode = 1;
}
