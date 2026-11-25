import { execFileSync } from 'node:child_process';
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

export async function watchWorkflow(repo, workflow, ref, head, { once = false } = {}) {
  const registrationDeadline = Date.now() + 5 * 60_000;
  const label = `${workflow} (${ref}, ${head.slice(0, 7)})`;
  let previous;
  while (true) {
    const output = read('gh', [
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
    const run = output ? JSON.parse(output) : null;
    const state = run ? `${run.conclusion || run.status} ${run.url}` : 'waiting for a run';
    if (previous !== state) console.log(`${label}: ${state}`);
    previous = state;
    if (run?.status === 'completed') {
      if (run.conclusion !== 'success')
        throw new Error(`${label} ended with ${run.conclusion || 'no conclusion'}.`);
      return;
    }
    if (once) return;
    if (!run && Date.now() >= registrationDeadline)
      throw new Error(`No matching workflow run appeared for ${label}.`);
    await sleep(run ? 20000 : 5000);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const [version, flag, extra] = process.argv.slice(2);
    if (!isReleaseVersion(version) || (flag !== undefined && flag !== '--once') || extra) {
      throw new Error('Usage: npm run release:watch -- <major.minor.patch> [--once]');
    }
    const repo = githubOrigin();
    const branch = `package/v${version}`;
    const remote = read('git', ['ls-remote', '--heads', 'origin', `refs/heads/${branch}`]);
    const head = remote.split('\t')[0];
    if (!head) throw new Error(`Remote branch ${branch} does not exist.`);
    await watchWorkflow(repo, 'package.yml', branch, head, { once: flag === '--once' });
  } catch (error) {
    console.error(`Packaging check failed: ${error.message}`);
    process.exitCode = 1;
  }
}
