import test from 'node:test';
import assert from 'node:assert/strict';
import { readdir, readFile } from 'node:fs/promises';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('..', import.meta.url));
const SOURCE = /\.(?:ts|tsx|js|mjs|css|html|rs|json|toml)$/;
// Built from parts so this file does not match itself if the scan is ever widened.
const FORBIDDEN = [['pandacode', 'ai'].join('.'), ['feature', 'base'].join('')];

async function files(dir) {
  const entries = await readdir(dir, { withFileTypes: true });
  const nested = await Promise.all(entries.map(entry => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return files(path);
    return SOURCE.test(entry.name) ? [path] : [];
  }));
  return nested.flat();
}

test('no app source references the paid PandaCode service or the old support portal', async () => {
  const sources = [...await files(join(root, 'src')), ...await files(join(root, 'src-tauri', 'src'))];
  assert.ok(sources.length > 50, 'scanned the app sources');
  const offenders = [];
  for (const path of sources) {
    const text = (await readFile(path, 'utf8')).toLowerCase();
    for (const term of FORBIDDEN) if (text.includes(term)) offenders.push(`${relative(root, path)}: ${term}`);
  }
  assert.deepEqual(offenders, []);
});

test('updates come only from signed GitHub Releases, and nothing reads license keys', async () => {
  const config = JSON.parse(await readFile(join(root, 'src-tauri', 'tauri.conf.json'), 'utf8'));
  assert.deepEqual(config.plugins?.updater?.endpoints, ['https://github.com/aicodeking/pandacode/releases/latest/download/latest.json']);
  assert.match(config.plugins.updater.pubkey, /^[A-Za-z0-9+/=]{40,}$/, 'a public key verifies every update');
  // Update files are produced only by builds that hold the signing key (scripts/release/build.mjs).
  assert.equal(config.bundle.createUpdaterArtifacts, undefined);
  const build = await readFile(join(root, 'src-tauri', 'build.rs'), 'utf8');
  assert.ok(!/PUBLIC_KEY/.test(build));
});

test('the fork notice the PandaCode License requires is in the README and on the About screen', async () => {
  const notice = 'This is a fork of PandaCode';
  const upstream = 'https://github.com/aicodeking/pandacode';
  const readme = await readFile(join(root, 'README.md'), 'utf8');
  const top = readme.split('\n').slice(0, 12).join('\n');
  assert.ok(top.includes(notice), 'the README states the fork near its top');
  assert.ok(top.includes(upstream), 'the README links to PandaCode beside the statement');
  const about = await readFile(join(root, 'src', 'about-settings.ts'), 'utf8');
  assert.match(about, new RegExp(`FORK_NOTICE = '${notice}'`));
  assert.match(about, /upstream: 'https:\/\/github\.com\/aicodeking\/pandacode'/);
  assert.match(about, /settings-fork-notice[^\n]*\$\{FORK_NOTICE\}[^\n]*PROJECT_LINKS\.upstream/, 'the About group shows the statement with the link');
});
