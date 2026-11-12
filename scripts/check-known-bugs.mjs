// RUSTSEC-2024-0429: Tauri/GTK3 still uses glib 0.18.5. Prevent introducing callers
// of its unsound VariantStrIter API anywhere in the resolved Linux Rust graph.
// This is a conservative source-use guard, not a whole-program proof or a patch.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';

const result = spawnSync(
  'cargo',
  [
    'metadata',
    '--locked',
    '--format-version',
    '1',
    '--filter-platform',
    'x86_64-unknown-linux-gnu',
  ],
  { encoding: 'utf8', maxBuffer: 16 * 1024 * 1024 },
);
if (result.status !== 0) {
  process.stderr.write(result.stderr || String(result.error));
  process.exit(1);
}
const metadata = JSON.parse(result.stdout);
const nodes = new Map(metadata.resolve.nodes.map((node) => [node.id, node]));
const reachable = new Set();
function visit(id) {
  if (reachable.has(id)) return;
  reachable.add(id);
  for (const dependency of nodes.get(id)?.deps ?? []) visit(dependency.pkg);
}
for (const id of metadata.workspace_members) visit(id);
const packages = metadata.packages.filter((pkg) => reachable.has(pkg.id));
const affected = packages.filter(
  (pkg) => pkg.name === 'glib' && /^0\.(?:1[5-9])\./.test(pkg.version),
);
if (affected.length) {
  assert.equal(affected.length, 1);
  assert.equal(
    affected[0].version,
    '0.18.5',
    'Review the glib advisory again when this version changes.',
  );
  const hits = [];
  function scan(directory) {
    if (!existsSync(directory)) return;
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) scan(path);
      else if (
        entry.isFile() &&
        entry.name.endsWith('.rs') &&
        /\b(?:array_iter_str|VariantStrIter)\b/.test(readFileSync(path, 'utf8'))
      )
        hits.push(path);
    }
  }
  for (const pkg of packages) {
    // glib defines/reexports the affected API and tests it internally; all callers are prohibited.
    if (pkg.id !== affected[0].id) scan(join(dirname(pkg.manifest_path), 'src'));
  }
  assert.deepEqual(
    hits,
    [],
    'A caller/reference to the affected glib API was introduced. Review RUSTSEC-2024-0429 before using it.',
  );
  console.log(
    `Checked ${packages.length} resolved Linux packages: no source references to the affected glib iterator outside its defining crate.`,
  );
} else {
  console.log(
    'No affected glib 0.15–0.19 is resolved; reassess the advisory baseline and remove this guard when appropriate.',
  );
}
