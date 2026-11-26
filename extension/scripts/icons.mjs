// Regenerates src/icons/icon-*.png from src/icons/icon.svg (needs rsvg-convert from librsvg).
// The PNGs are committed, so this only has to run after changing the SVG.
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const dir = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', 'src', 'icons');
for (const size of [16, 32, 48, 96, 128]) {
  execFileSync('rsvg-convert', ['-w', String(size), '-h', String(size), '-o', path.join(dir, `icon-${size}.png`), path.join(dir, 'icon.svg')]);
  console.log(`icon-${size}.png`);
}
