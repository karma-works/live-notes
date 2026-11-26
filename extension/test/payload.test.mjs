import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const P = require('../src/lib/payload.js');
const api = require('../src/lib/api.js');
const { startMockServer } = await import('./mock-server.mjs');

const tab = { url: 'https://example.org/a', title: 'Tab title' };

test('page payload: fields per docs/clip-api.md, empty optionals omitted', () => {
  const p = P.buildPagePayload(
    { url: 'https://example.org/a', title: 'Artikel', site_name: 'Example', byline: ' ', lang: 'de', text: 'Absatz 1\n\nAbsatz 2', html: '<p>x</p>', selection: null },
    tab,
  );
  assert.deepEqual(p, { kind: 'page', url: 'https://example.org/a', title: 'Artikel', site_name: 'Example', lang: 'de', text: 'Absatz 1\n\nAbsatz 2', html: '<p>x</p>' });
});

test('page payload: context-menu selection wins over page selection; title falls back to tab', () => {
  const p = P.buildPagePayload({ url: tab.url, title: '', text: 't', selection: 'page sel' }, tab, ' menu sel ');
  assert.equal(p.selection, 'menu sel');
  assert.equal(p.title, 'Tab title');
  assert.equal(P.buildPagePayload({ url: tab.url, text: 't', selection: 'page sel' }, tab).selection, 'page sel');
});

test('youtube payload', () => {
  const transcript = [{ start_ms: 0, dur_ms: 1000, text: 'Hallo' }];
  const p = P.buildYouTubePayload(
    {
      url: 'https://www.youtube.com/watch?v=dQw4w9WgXcQ',
      title: 'Video',
      lang: 'de-DE',
      youtube: { video_id: 'dQw4w9WgXcQ', channel: 'Ch', duration_s: 212.6, description: 'D', transcript_lang: 'en', transcript_source: 'innertube', transcript },
    },
    tab,
  );
  assert.deepEqual(p, {
    kind: 'youtube',
    url: 'https://www.youtube.com/watch?v=dQw4w9WgXcQ',
    title: 'Video',
    site_name: 'YouTube',
    lang: 'de-DE',
    text: '',
    youtube: { video_id: 'dQw4w9WgXcQ', channel: 'Ch', duration_s: 213, description: 'D', transcript_lang: 'en', transcript_source: 'innertube', transcript },
  });
});

test('youtube payload without transcript sends transcript: null and no source', () => {
  const p = P.buildYouTubePayload({ url: 'https://youtu.be/x', title: 'V', youtube: { video_id: 'x', transcript: [], transcript_source: 'panel' } }, tab);
  assert.equal(p.youtube.transcript, null);
  assert.equal('transcript_source' in p.youtube, false);
  assert.equal(JSON.parse(JSON.stringify(p)).youtube.transcript, null);
});

test('fitSize keeps bodies under the limit: html dropped first, then text cut', () => {
  const big = { kind: 'page', url: 'u', title: 't', text: 'ä'.repeat(600), html: 'x'.repeat(600) };
  const a = P.fitSize(big, 1500);
  assert.equal(a.html, undefined);
  assert.equal(a.text.length, 600);
  const b = P.fitSize({ ...big, text: 'ä'.repeat(2000) }, 1500);
  assert.ok(new TextEncoder().encode(JSON.stringify(b)).length <= 1500);
  assert.ok(b.text.length > 100);
  const segs = Array.from({ length: 200 }, (_, i) => ({ start_ms: i, dur_ms: 1, text: 'word '.repeat(5) }));
  const y = P.fitSize({ kind: 'youtube', url: 'u', title: 't', text: '', youtube: { video_id: 'x', transcript: segs } }, 4000);
  assert.ok(new TextEncoder().encode(JSON.stringify(y)).length <= 4000);
  assert.ok(y.youtube.transcript.length > 10 && y.youtube.transcript[0].start_ms === 0);
});

test('fallback payload for unscriptable pages', () => {
  assert.deepEqual(P.buildFallbackPayload(tab, 'sel'), { kind: 'page', url: tab.url, title: 'Tab title', text: '', selection: 'sel' });
});

test('api client against the mock: ping, clip + job, 401, 403, connection refused', async () => {
  const m = await startMockServer({ port: 0, token: 'tok', jobDelayMs: 50 });
  const port = m.server.address().port;
  try {
    const ext = (url, o = {}) => fetch(url, { ...o, headers: { ...o.headers, Origin: 'moz-extension://abc' } });
    assert.deepEqual(await api.ping({ port, token: 'tok' }, ext), { app: 'live-notes', version: '0.0.0-mock', api: 1 });
    const { job_id } = await api.clip({ port, token: 'tok' }, P.buildPagePayload({ url: 'https://e.org', title: 'T', text: 'x' }, {}), ext);
    assert.equal((await api.job({ port, token: 'tok' }, job_id, ext)).status, 'pending');
    await new Promise((r) => setTimeout(r, 100));
    assert.deepEqual(await api.job({ port, token: 'tok' }, job_id, ext), { status: 'done', stage: 'done', title: 'T', path: '/tmp/mock-notes/Web/T.md' });
    await assert.rejects(api.job({ port, token: 'tok' }, 'nope', ext), { code: 'not_found' });
    await assert.rejects(api.ping({ port, token: 'wrong' }, ext), { code: 'unauthorized', status: 401 });
    await assert.rejects(api.ping({ port, token: '' }, ext), { code: 'unauthorized' });
    const web = (url, o = {}) => fetch(url, { ...o, headers: { ...o.headers, Origin: 'https://evil.example' } });
    await assert.rejects(api.ping({ port, token: 'tok' }, web), { code: 'forbidden', status: 403 });
    await assert.rejects(api.ping({ port: 99999, token: 'tok' }), { code: 'bad_settings' });
  } finally {
    await m.close();
  }
  await assert.rejects(api.ping({ port, token: 'tok' }), { code: 'not_running' });
});
