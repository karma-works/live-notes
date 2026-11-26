// End-to-end: mock Live Notes + the TEST build in Zen/Firefox with a throwaway profile.
//
//   node test/e2e.mjs [--firefox /usr/bin/zen-browser] [--headful]
//
// Saves one article page and one YouTube video through the same saveTab() as the toolbar
// button (triggered via test-hook.js) and checks what the mock received.
import { spawn, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startMockServer } from './mock-server.mjs';

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), '..');
const arg = (name, def) => {
  const i = process.argv.indexOf('--' + name);
  return i > 0 ? process.argv[i + 1] : def;
};
const FIREFOX = arg('firefox', '/usr/bin/zen-browser');
const PORT = Number(process.env.LIVE_NOTES_TEST_PORT) || 47899;
const TOKEN = 'e2e-' + Math.random().toString(16).slice(2);
const ARTICLE = arg('article', 'https://en.wikipedia.org/wiki/Note-taking');
const VIDEO = arg('video', 'https://youtu.be/dQw4w9WgXcQ');
const CONSENT = [
  'ytd-consent-bump-v2-lightbox button[aria-label*="Reject" i]',
  'ytd-consent-bump-v2-lightbox button[aria-label*="ablehnen" i]',
  'form[action*="consent.youtube.com"] button[aria-label*="Reject" i]',
  'form[action*="consent.youtube.com"] button[aria-label*="ablehnen" i]',
];

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];
const check = (name, ok, detail = '') => {
  results.push({ name, ok });
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail ? '  — ' + detail : ''}`);
};

execFileSync(process.execPath, [path.join(root, 'scripts', 'build.mjs'), '--test'], { stdio: 'inherit', env: { ...process.env, LIVE_NOTES_TEST_PORT: String(PORT) } });
const mock = await startMockServer({ port: PORT, token: TOKEN });
console.log(`mock on :${PORT}`);

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'live-notes-e2e-profile-'));
const env = { ...process.env };
if (!process.argv.includes('--headful')) env.MOZ_HEADLESS = '1';
const webExt = path.join(root, 'node_modules', '.bin', 'web-ext');
const browser = spawn(
  webExt,
  [
    'run',
    '--source-dir', path.join(root, 'dist-test'),
    '--firefox', FIREFOX,
    '--firefox-profile', profile,
    '--profile-create-if-missing',
    '--keep-profile-changes',
    '--no-input',
    '--no-reload',
    '--start-url', 'about:blank',
    '--pref', 'intl.locale.requested=de',
    '--pref', 'intl.accept_languages=de-DE,de,en',
    '--pref', 'browser.shell.checkDefaultBrowser=false',
    '--pref', 'zen.welcome-screen.seen=true',
  ],
  { env, stdio: ['ignore', 'pipe', 'pipe'], detached: true },
);
let browserLog = '';
browser.stdout.on('data', (d) => (browserLog += d));
browser.stderr.on('data', (d) => (browserLog += d));

async function waitReport(pred, timeoutMs) {
  const t0 = Date.now();
  while (Date.now() - t0 < timeoutMs) {
    const r = mock.state.reports.find(pred);
    if (r) return r;
    await sleep(500);
  }
  return null;
}

async function shutdown(code) {
  try {
    process.kill(-browser.pid, 'SIGTERM');
  } catch {
    // already gone
  }
  await sleep(1500);
  try {
    process.kill(-browser.pid, 'SIGKILL');
  } catch {
    // already gone
  }
  await mock.close();
  fs.rmSync(profile, { recursive: true, force: true });
  fs.writeFileSync(path.join(root, 'dist-test', 'e2e-state.json'), JSON.stringify(mock.state, null, 2));
  process.exit(code);
}

try {
  mock.state.queue.push({ action: 'configure', port: PORT, token: TOKEN });
  const conf = await waitReport((r) => r && r.action === 'configure', 60000);
  check('extension started and test hook configured settings', !!conf, conf ? 'origins=' + JSON.stringify(conf.origins) : browserLog.slice(-2000));
  if (!conf) await shutdown(1);

  mock.state.queue.push({ action: 'ping-options' });
  const ping = await waitReport((r) => r && r.action === 'ping', 15000);
  check('GET /v1/ping with token', ping && ping.ok && ping.result.app === 'live-notes', JSON.stringify(ping && (ping.result || ping.code)));

  // ---- article ----
  mock.state.queue.push({ action: 'save', url: ARTICLE, wait_ms: 1500 });
  const a = await waitReport((r) => r && r.action === 'save' && r.url === ARTICLE, 90000);
  const ac = mock.state.clips.find((c) => c.payload.kind === 'page');
  check('article: saveTab finished with status done', a && a.result && a.result.status === 'done', JSON.stringify(a && a.result));
  if (ac) {
    const p = ac.payload;
    check('article: payload kind/url/title', p.kind === 'page' && p.url.startsWith('https://') && !!p.title, `title="${p.title}"`);
    check('article: Readability text', p.text.length > 2000 && p.text.includes('\n\n'), `text=${p.text.length} chars, html=${(p.html || '').length} chars, paragraphs=${p.text.split('\n\n').length}`);
    check('article: lang/site_name', !!p.lang, `lang=${p.lang} site_name=${p.site_name} byline=${p.byline}`);
  } else check('article: payload received', false);
  check('article: badge ✓ after done', a && a.badge === '✓', 'badge=' + (a && a.badge));

  // ---- YouTube ----
  mock.state.queue.push({ action: 'save', url: VIDEO, wait_ms: 4000, click: CONSENT });
  const y = await waitReport((r) => r && r.action === 'save' && r.url === VIDEO, 150000);
  console.log('youtube strategies:', JSON.stringify(y && y.extraction));
  const yc = mock.state.clips.find((c) => c.payload.kind === 'youtube');
  check('youtube: saveTab finished with status done', y && y.result && y.result.status === 'done', JSON.stringify(y && y.result) + ' finalUrl=' + (y && y.finalUrl));
  if (yc) {
    const p = yc.payload;
    const yy = p.youtube || {};
    const segs = yy.transcript ? yy.transcript.length : 0;
    check('youtube: video metadata', yy.video_id === 'dQw4w9WgXcQ' && !!yy.channel && yy.duration_s > 100, `title="${p.title}" channel=${yy.channel} duration_s=${yy.duration_s} description=${(yy.description || '').length} chars`);
    check('youtube: transcript', segs > 20, `source=${yy.transcript_source} lang=${yy.transcript_lang} segments=${segs} first=${JSON.stringify(yy.transcript && yy.transcript[0])}`);
    // browser UI is German, the video is English: the original English track must win
    check('youtube: original-language track despite German UI', yy.transcript_lang === 'en' && yy.transcript.some((s) => /strangers/i.test(s.text)), `lang=${yy.transcript_lang}`);
    check('youtube: payload uses the innertube strategy', yy.transcript_source === 'innertube', `source=${yy.transcript_source}`);
  } else check('youtube: payload received', false);

  // ---- YouTube after in-app (SPA) navigation: page globals belong to the previous video ----
  const SPA = VIDEO + '#spa';
  mock.state.queue.push({
    action: 'save',
    url: SPA,
    wait_ms: 4000,
    click: CONSENT,
    spa_video: 'jNQXAC9IVRw', // "Me at the zoo": only the legacy transcript panel, no get_panel
  });
  const s2 = await waitReport((r) => r && r.action === 'save' && r.url === SPA, 150000);
  const sc = mock.state.clips.filter((c) => c.payload.kind === 'youtube').at(-1);
  const spaId = s2 && s2.finalUrl && (s2.finalUrl.match(/[?&]v=([\w-]{11})/) || [])[1];
  console.log('spa strategies:', JSON.stringify(s2 && s2.extraction), 'stale ytInitialData video:', s2 && s2.ytInitialDataVideo);
  check('spa: navigated to another video in-app (globals stale)', spaId === 'jNQXAC9IVRw' && s2.ytInitialDataVideo === 'dQw4w9WgXcQ', `finalUrl=${s2 && s2.finalUrl} ytInitialData video=${s2 && s2.ytInitialDataVideo}`);
  if (sc && spaId) {
    const yy = sc.payload.youtube;
    check('spa: payload describes the current video, not the stale one', yy.video_id === spaId && sc !== yc && sc.payload.title !== (yc && yc.payload.title), `video_id=${yy.video_id} title="${sc.payload.title}" stale globals=${s2.ytInitialDataVideo}`);
    check('spa: transcript', !!yy.transcript && yy.transcript.length >= 2 && yy.transcript_lang === 'en', `source=${yy.transcript_source} lang=${yy.transcript_lang} segments=${yy.transcript ? yy.transcript.length : null}`);
  } else check('spa: payload received', false);

  // ---- each fallback strategy on its own (nothing is posted) ----
  for (const skip of [['innertube'], ['innertube', 'panel']]) {
    const url = VIDEO.replace('youtu.be/', 'www.youtube.com/watch?v=') + '&only=' + skip.join('-');
    mock.state.queue.push({ action: 'extract-youtube', url, skip });
    const r = await waitReport((x) => x && x.action === 'extract' && x.url === url, 90000);
    const name = skip.length === 1 ? 'panel' : 'timedtext';
    const detail = r ? `source=${r.source} lang=${r.lang} segments=${r.segments} attempts=${JSON.stringify(r.attempts)}` : 'no report';
    if (name === 'panel') check('strategy b (DOM transcript panel) alone yields segments', r && r.source === 'panel' && r.segments > 10, detail);
    else console.log(`INFO  strategy c (timedtext json3) alone: ${detail}`);
  }

  const origins = [...new Set(mock.state.requests.map((r) => String(r.origin)))];
  check('requests carried an extension Origin (or none) and passed the Origin check', origins.every((o) => o === 'null' || o.startsWith('moz-extension://')), 'origins seen: ' + origins.join(', '));
} catch (e) {
  console.error(e);
  check('no exception', false, String(e));
}

const failed = results.filter((r) => !r.ok).length;
console.log(`\n${results.length - failed}/${results.length} checks passed; full mock state in dist-test/e2e-state.json`);
await shutdown(failed ? 1 : 0);
