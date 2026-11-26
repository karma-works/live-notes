// Minimal build: copy src/ to dist/ and vendor Readability (Apache-2.0) from node_modules.
// `--test` additionally wires in test/test-hook.js (e2e only, never part of a release build)
// and writes to dist-test/.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), '..');
const test = process.argv.includes('--test');
const out = path.join(root, test ? 'dist-test' : 'dist');

fs.rmSync(out, { recursive: true, force: true });
fs.cpSync(path.join(root, 'src'), out, { recursive: true });

const rd = path.join(root, 'node_modules', '@mozilla', 'readability');
const vendor = path.join(out, 'vendor');
fs.mkdirSync(vendor, { recursive: true });
fs.copyFileSync(path.join(rd, 'Readability.js'), path.join(vendor, 'Readability.js'));
fs.copyFileSync(path.join(rd, 'LICENSE.md'), path.join(vendor, 'Readability.LICENSE.md'));
const version = JSON.parse(fs.readFileSync(path.join(rd, 'package.json'), 'utf8')).version;
fs.writeFileSync(path.join(vendor, 'README.md'), `@mozilla/readability ${version}, unmodified copy of Readability.js (Apache-2.0, see Readability.LICENSE.md).\n`);

if (test) {
  const manifestPath = path.join(out, 'manifest.json');
  const m = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
  const controlPort = String(Number(process.env.LIVE_NOTES_TEST_PORT) || 47899);
  const hook = fs.readFileSync(path.join(root, 'test', 'test-hook.js'), 'utf8').replace('__TEST_CONTROL_PORT__', controlPort);
  fs.writeFileSync(path.join(out, 'test-hook.js'), hook);
  m.background.scripts.push('test-hook.js');
  // Tabs opened by the test hook get no activeTab grant, so the test build may script any page.
  m.host_permissions.push('<all_urls>');
  m.name = 'Live Notes (TEST BUILD)';
  fs.writeFileSync(manifestPath, JSON.stringify(m, null, 2) + '\n');
}

console.log(`built ${path.relative(root, out)}/${test ? ' (test build with test-hook.js)' : ''}`);
