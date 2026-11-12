import { spawnSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';

const result = spawnSync(
  'cargo',
  ['run', '--locked', '--quiet', '-p', 'uclip-core', '--example', 'export_contracts'],
  { encoding: 'utf8' },
);
if (result.status !== 0) {
  process.stderr.write(result.stderr || String(result.error));
  process.exit(1);
}
const target = new URL('../src/generated/contracts.ts', import.meta.url);
if (process.argv.includes('--check')) {
  let current = '';
  try {
    current = readFileSync(target, 'utf8');
  } catch {
    /* Missing generated file is drift too. */
  }
  if (current !== result.stdout) {
    process.stderr.write(
      'IPC types changed. Run npm run contracts:generate and review the result.\n',
    );
    process.exit(1);
  }
} else {
  writeFileSync(target, result.stdout);
}
