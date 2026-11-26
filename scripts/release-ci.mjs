import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createReadStream, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));

function read(command, args) {
  return execFileSync(command, args, { cwd: root, encoding: 'utf8', timeout: 30000 }).trim();
}

export function isReleaseVersion(version) {
  return typeof version === 'string' && /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version);
}

export function githubOrigin() {
  const origin = read('git', ['remote', 'get-url', 'origin']);
  const repo =
    /^(?:https:\/\/github\.com\/|git@github\.com:|ssh:\/\/git@github\.com\/)([^/]+\/[^/]+?)(?:\.git)?\/?$/.exec(
      origin,
    )?.[1];
  if (!repo) throw new Error('The origin remote must point to a GitHub repository.');
  return repo;
}

export function packageCommitSubject(version) {
  return `Package UClip ${version}`;
}

export function findPackageTarget(version) {
  const subject = packageCommitSubject(version);
  const pattern = `^${subject.replaceAll('.', '\\.')}$`;
  const entries = read('git', [
    'log',
    '--first-parent',
    '--extended-regexp',
    `--grep=${pattern}`,
    '--format=%H%x09%s',
    'origin/main',
  ]);
  const commit = entries.split('\n').find((entry) => entry.endsWith(`\t${subject}`));
  if (commit) return { ref: 'main', head: commit.split('\t')[0] };

  // Installers built before main pushes could trigger packaging use a dedicated branch.
  const ref = `package/v${version}`;
  const remote = read('git', ['ls-remote', '--heads', 'origin', `refs/heads/${ref}`]);
  const head = remote.split('\t')[0];
  return head ? { ref, head } : undefined;
}

export async function watchWorkflow(
  repo,
  ref,
  head,
  { once = false, workflow = 'package.yml' } = {},
) {
  const registrationDeadline = Date.now() + 5 * 60_000;
  const label = `${workflow} (${ref}, ${head.slice(0, 7)})`;
  let previous;
  let lookupFailures = 0;
  while (true) {
    let output;
    try {
      output = read('gh', [
        'api',
        '--method',
        'GET',
        `repos/${repo}/actions/workflows/${workflow}/runs`,
        '-f',
        `branch=${ref}`,
        '-f',
        'event=push',
        '-f',
        `head_sha=${head}`,
        '-f',
        'per_page=1',
        '--jq',
        '(.workflow_runs[0] // empty) | {id,status,conclusion,url:.html_url}',
      ]);
    } catch (error) {
      lookupFailures += 1;
      if (once || lookupFailures >= 3) throw error;
      await sleep(5000);
      continue;
    }
    lookupFailures = 0;
    const run = output ? JSON.parse(output) : null;
    const state = run ? `${run.conclusion || run.status} ${run.url}` : 'waiting for a run';
    if (previous !== state) console.log(`${label}: ${state}`);
    previous = state;
    if (run?.status === 'completed') {
      if (run.conclusion !== 'success')
        throw new Error(`${label} ended with ${run.conclusion || 'no conclusion'}. See ${run.url}`);
      return run;
    }
    if (once) return;
    if (!run && Date.now() >= registrationDeadline)
      throw new Error(`No matching workflow run appeared for ${label}.`);
    await sleep(run ? 20000 : 5000);
  }
}

export async function verifyRelease(repo, tag, version) {
  const release = JSON.parse(
    read('gh', [
      'release',
      'view',
      tag,
      '--repo',
      repo,
      '--json',
      'url,isDraft,isPrerelease,assets',
    ]),
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
    execFileSync('gh', ['release', 'download', tag, '--repo', repo, '--dir', directory], {
      cwd: root,
      stdio: 'inherit',
    });
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
  return release.url;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const [version, flag, extra] = process.argv.slice(2);
    if (!isReleaseVersion(version) || (flag !== undefined && flag !== '--once') || extra) {
      throw new Error('Usage: npm run release:watch -- <major.minor.patch> [--once]');
    }
    const repo = githubOrigin();
    read('git', ['fetch', '--quiet', 'origin', 'main']);
    const target = findPackageTarget(version);
    if (!target) throw new Error(`No packaging commit found for UClip ${version}.`);
    await watchWorkflow(repo, target.ref, target.head, { once: flag === '--once' });
  } catch (error) {
    console.error(`Packaging check failed: ${error.message}`);
    process.exitCode = 1;
  }
}
