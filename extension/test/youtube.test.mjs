import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const yt = createRequire(import.meta.url)('../src/lib/youtube.js');
const fx = (name) => fs.readFileSync(path.join(path.dirname(fileURLToPath(import.meta.url)), 'fixtures', name), 'utf8');

const watch = yt.dataFromWatchHtml(fx('watch-dQw4w9WgXcQ.html'));

test('videoIdFromUrl: watch, shorts, youtu.be, embed, others', () => {
  assert.equal(yt.videoIdFromUrl('https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=10s'), 'dQw4w9WgXcQ');
  assert.equal(yt.videoIdFromUrl('https://m.youtube.com/watch?v=dQw4w9WgXcQ'), 'dQw4w9WgXcQ');
  assert.equal(yt.videoIdFromUrl('https://www.youtube.com/shorts/aqz-KE-bpKQ'), 'aqz-KE-bpKQ');
  assert.equal(yt.videoIdFromUrl('https://youtu.be/dQw4w9WgXcQ?si=x'), 'dQw4w9WgXcQ');
  assert.equal(yt.videoIdFromUrl('https://www.youtube.com/embed/dQw4w9WgXcQ'), 'dQw4w9WgXcQ');
  assert.equal(yt.videoIdFromUrl('https://www.youtube.com/@RickAstleyYT'), null);
  assert.equal(yt.videoIdFromUrl('https://example.com/watch?v=dQw4w9WgXcQ'), null);
  assert.equal(yt.videoIdFromUrl('not a url'), null);
});

test('watch page: ytInitialData, player response and ytcfg are extracted', () => {
  assert.ok(watch.initialData && watch.playerResponse);
  assert.equal(watch.playerResponse.videoDetails.videoId, 'dQw4w9WgXcQ');
  assert.ok(watch.ytcfg.INNERTUBE_CONTEXT.client.clientVersion);
  assert.equal(watch.ytcfg.CLIENT_CANARY_STATE, 'none', 'multiple ytcfg.set calls are merged');
});

test('readJsonObject handles braces inside strings', () => {
  const html = 'var x = {"a":"}{\\"","b":{"c":1}};';
  assert.deepEqual(yt.extractAssignedJson(html, 'x'), { a: '}{"', b: { c: 1 } });
});

test('transcript endpoints are found in ytInitialData', () => {
  assert.deepEqual(yt.findTranscriptPanel(watch.initialData), { panelId: 'PAmodern_transcript_view', params: 'qgkPCgtkUXc0dzlXZ1hjURgC' });
  assert.match(yt.findTranscriptParams(watch.initialData), /^CgtkUXc0dzlXZ1hjUR/);
});

test('videoMeta', () => {
  const m = yt.videoMeta(watch.playerResponse, watch.initialData);
  assert.equal(m.video_id, 'dQw4w9WgXcQ');
  assert.equal(m.channel, 'Rick Astley');
  assert.equal(m.duration_s, 213);
  assert.ok(m.description.length > 100);
  assert.match(m.title, /Never Gonna Give You Up/);
});

test('chooseCaptionTrack prefers the manual track in the spoken language', () => {
  const tracks = yt.captionTracks(watch.playerResponse);
  assert.equal(yt.chooseCaptionTrack(tracks).languageCode, 'en');
  assert.notEqual(yt.chooseCaptionTrack(tracks).kind, 'asr');
  const asrOnly = [{ languageCode: 'de', kind: 'asr' }, { languageCode: 'fr' }];
  assert.equal(yt.chooseCaptionTrack(asrOnly).languageCode, 'de', 'auto-generated original beats a foreign manual track');
  assert.equal(yt.chooseCaptionTrack([{ languageCode: 'ja' }]).languageCode, 'ja');
  assert.equal(yt.chooseCaptionTrack([]), null);
});

test('chooseCaptionTrack without asr track: captionsInitialState reveals a foreign-language original', () => {
  // "Me at the zoo" (English audio, manual en + de tracks) as served to a German and an English UI
  const pr = (order, state) => ({
    captions: { playerCaptionsTracklistRenderer: { captionTracks: order.map((l) => ({ languageCode: l })), audioTracks: [{ defaultCaptionTrackIndex: 0, captionsInitialState: state }] } },
  });
  const cfg = (hl) => ({ INNERTUBE_CONTEXT: { client: { hl } } });
  const de = pr(['de', 'en'], 'CAPTIONS_INITIAL_STATE_ON_RECOMMENDED');
  assert.equal(yt.chooseCaptionTrack(yt.captionTracks(de), yt.trackHints(de, cfg('de'))).languageCode, 'en');
  const en = pr(['en', 'de'], 'CAPTIONS_INITIAL_STATE_OFF_RECOMMENDED');
  assert.equal(yt.chooseCaptionTrack(yt.captionTracks(en), yt.trackHints(en, cfg('en'))).languageCode, 'en');
});

test('get_panel (modern transcript panel, recorded live) is parsed with ms timings', () => {
  const segs = yt.parsePanelTranscript(JSON.parse(fx('get_panel-dQw4w9WgXcQ.json')), 213);
  assert.ok(segs.length >= 20, 'segments: ' + segs.length);
  assert.deepEqual(Object.keys(segs[0]), ['start_ms', 'dur_ms', 'text']);
  assert.equal(segs[0].start_ms, 1000);
  for (let i = 1; i < segs.length; i++) {
    assert.ok(segs[i].start_ms >= segs[i - 1].start_ms);
    assert.equal(segs[i - 1].dur_ms, segs[i].start_ms - segs[i - 1].start_ms);
  }
  assert.equal(segs.at(-1).start_ms + segs.at(-1).dur_ms, 213000, 'last segment runs to the end');
  assert.ok(segs.some((s) => /strangers/i.test(s.text)), 'English text');
  assert.ok(segs.every((s) => s.text && !/\s{2}/.test(s.text)));
});

test('legacy get_transcript response is parsed; section headers and blanks are skipped', () => {
  const json = JSON.parse(fx('get_transcript-legacy.json'));
  assert.deepEqual(yt.parseInnertubeTranscript(json), [
    { start_ms: 1360, dur_ms: 1680, text: '[♪♪♪]' },
    { start_ms: 18640, dur_ms: 3240, text: "♪ We're no strangers to love ♪" },
    { start_ms: 22640, dur_ms: 4320, text: '♪ You know the rules and so do I ♪' },
  ]);
  const menu = yt.parseTranscriptLanguageMenu(json);
  assert.equal(menu.length, 3);
  assert.deepEqual(menu.find((m) => m.selected), { title: 'German (Germany)', selected: true, params: 'PARAMS_DE' });
});

test('json3 timedtext is parsed', () => {
  assert.deepEqual(yt.parseJson3(JSON.parse(fx('timedtext.json3'))), [
    { start_ms: 1360, dur_ms: 1680, text: '[♪♪♪]' },
    { start_ms: 18640, dur_ms: 3240, text: "♪ We're no strangers to love ♪" },
    { start_ms: 22640, dur_ms: 4320, text: '♪ You know the rules and so do I ♪' },
  ]);
});

test('panel DOM rows become segments', () => {
  assert.equal(yt.parseTimestamp('1:02:03'), 3723000);
  assert.equal(yt.parseTimestamp('0:18'), 18000);
  assert.equal(yt.parseTimestamp('x'), null);
  assert.deepEqual(
    yt.segmentsFromPanelRows([
      { time: '0:18', text: ' We are\n no strangers ' },
      { time: '', text: 'ignored' },
      { time: '0:27', text: 'to love' },
    ]),
    [
      { start_ms: 18000, dur_ms: 9000, text: 'We are no strangers' },
      { start_ms: 27000, dur_ms: 0, text: 'to love' },
    ],
  );
});

// --- fetchTranscript orchestration with a fake network -----------------------------------

function fakeFetch(routes, log) {
  return async (url, opts = {}) => {
    const body = opts.body ? JSON.parse(opts.body) : null;
    log.push({ url, body, headers: opts.headers });
    for (const [re, handler] of routes) {
      if (re.test(url)) {
        const [status, text] = handler(body);
        return { ok: status < 300, status, text: async () => text };
      }
    }
    return { ok: false, status: 404, text: async () => '' };
  };
}

test('fetchTranscript: get_panel succeeds, hl is set to the chosen track language', async () => {
  const log = [];
  const panel = fx('get_panel-dQw4w9WgXcQ.json');
  const r = await yt.fetchTranscript(watch, { fetch: fakeFetch([[/get_panel/, () => [200, panel]]], log), cookie: '' });
  assert.equal(r.transcript_source, 'innertube');
  assert.equal(r.transcript_lang, 'en');
  assert.ok(r.transcript.length >= 20);
  assert.equal(log.length, 1);
  assert.equal(log[0].body.panelId, 'PAmodern_transcript_view');
  assert.equal(log[0].body.context.client.hl, 'en');
  assert.equal(log[0].headers['X-Youtube-Client-Name'], '1');
  assert.equal(watch.ytcfg.INNERTUBE_CONTEXT.client.hl !== undefined, true, 'original context untouched');
});

test('fetchTranscript: falls back get_panel → get_transcript → panel → timedtext → null', async () => {
  const fail = () => [400, '{"error":{"status":"FAILED_PRECONDITION"}}'];
  const legacy = fx('get_transcript-legacy.json');
  let log = [];
  let r = await yt.fetchTranscript(watch, { fetch: fakeFetch([[/get_panel/, fail], [/get_transcript/, () => [200, legacy]]], log) });
  assert.equal(r.transcript_source, 'innertube');
  assert.equal(r.transcript.length, 3);
  // the menu's selected track was German; the English manual track is preferred → re-request
  assert.deepEqual(log.map((l) => l.body && l.body.params).slice(1), [yt.findTranscriptParams(watch.initialData), 'PARAMS_EN']);
  assert.equal(r.transcript_lang, 'en');

  log = [];
  r = await yt.fetchTranscript(watch, {
    fetch: fakeFetch([[/youtubei/, fail]], log),
    panel: async () => ({ rows: [{ time: '0:18', text: 'a' }, { time: '0:27', text: 'b' }], title: '' }),
  });
  assert.equal(r.transcript_source, 'panel');
  assert.equal(r.transcript.at(-1).dur_ms, 213000 - 27000);

  r = await yt.fetchTranscript(watch, { fetch: fakeFetch([[/youtubei/, fail], [/timedtext/, () => [200, fx('timedtext.json3')]]], (log = [])) });
  assert.equal(r.transcript_source, 'timedtext');
  assert.match(log.at(-1).url, /fmt=json3$/);
  assert.equal(r.transcript.length, 3);

  r = await yt.fetchTranscript(watch, { fetch: fakeFetch([[/youtubei/, fail], [/timedtext/, () => [200, '']]], []) });
  assert.equal(r.transcript, null);
  assert.equal(r.transcript_source, null);
  assert.deepEqual(r.attempts.map((a) => a.strategy), ['innertube', 'panel', 'timedtext']);
  assert.match(r.attempts[2].error, /PO token/);
});

test('innertubeHeaders adds SAPISIDHASH for signed-in sessions', async () => {
  const h = await yt.innertubeHeaders(watch.ytcfg, { cookie: 'YSC=1; SAPISID=abc/def; x=y' });
  assert.match(h.Authorization, /^SAPISIDHASH \d+_[0-9a-f]{40}$/);
  assert.equal(h['X-Origin'], 'https://www.youtube.com');
  const anon = await yt.innertubeHeaders(watch.ytcfg, { cookie: 'YSC=1' });
  assert.equal(anon.Authorization, undefined);
});

test('buildPanelParams reproduces the params youtube.com sends to get_panel', () => {
  // value observed in Zen for youtube.com's own get_panel request (Nov 2026)
  assert.equal(yt.buildPanelParams('jNQXAC9IVRw'), 'qgkPCgtqTlFYQUM5SVZSdxgB');
  assert.equal(Buffer.from(yt.buildPanelParams('dQw4w9WgXcQ'), 'base64').subarray(5, 16).toString(), 'dQw4w9WgXcQ');
});

test('fetchTranscript builds get_panel params when ytInitialData is stale', async () => {
  const log = [];
  const stale = { ...watch, videoId: 'jNQXAC9IVRw', initialData: null };
  const r = await yt.fetchTranscript(stale, { fetch: fakeFetch([[/get_panel/, () => [200, fx('get_panel-dQw4w9WgXcQ.json')]]], log) });
  assert.equal(log[0].body.params, 'qgkPCgtqTlFYQUM5SVZSdxgB');
  assert.equal(r.transcript_source, 'innertube');
  assert.equal(r.attempts[0].endpoint, 'get_panel (built params)');
});
