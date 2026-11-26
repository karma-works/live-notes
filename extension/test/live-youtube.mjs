// Live check of the YouTube transcript strategies against youtube.com (network!).
//
//   node test/live-youtube.mjs [videoId] [--save-fixtures]
//
// Uses src/lib/youtube.js exactly as the extension does, but from Node: fetches the watch
// page once (anonymous, consent declined via SOCS cookie), then tries each strategy on its own.
// The DOM panel strategy needs a browser and is covered by test/e2e.mjs.
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const yt = createRequire(import.meta.url)('../src/lib/youtube.js');
const videoId = process.argv.slice(2).find((a) => !a.startsWith('--')) || 'dQw4w9WgXcQ';
const saveFixtures = process.argv.includes('--save-fixtures');
const UA = 'Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0';

const jar = new Map([['SOCS', 'CAI']]); // "reject all" consent choice
const cookieHeader = () => [...jar].map(([k, v]) => `${k}=${v}`).join('; ');
async function nodeFetch(url, opts = {}) {
  const res = await fetch(url, { ...opts, headers: { 'User-Agent': UA, 'Accept-Language': 'en-US,en;q=0.8', Origin: 'https://www.youtube.com', Cookie: cookieHeader(), ...(opts.headers || {}) } });
  for (const c of res.headers.getSetCookie()) {
    const [kv] = c.split(';');
    const i = kv.indexOf('=');
    jar.set(kv.slice(0, i), kv.slice(i + 1));
  }
  return res;
}
const env = { fetch: nodeFetch, cookie: '' };

const html = await (await nodeFetch(`https://www.youtube.com/watch?v=${videoId}`)).text();
const data = { videoId, ...yt.dataFromWatchHtml(html) };
const meta = yt.videoMeta(data.playerResponse, data.initialData);
const tracks = yt.captionTracks(data.playerResponse);
console.log(`video ${videoId}: "${meta.title}" by ${meta.channel}, ${meta.duration_s}s`);
console.log(`  ytInitialData: ${!!data.initialData}, ytInitialPlayerResponse: ${!!data.playerResponse}, INNERTUBE_CONTEXT: ${!!data.ytcfg.INNERTUBE_CONTEXT}`);
console.log(`  captionTracks: ${tracks.map((t) => t.languageCode + (t.kind ? '/' + t.kind : '')).join(' ') || '-'}; chosen: ${(yt.chooseCaptionTrack(tracks) || {}).languageCode}`);
console.log(`  modern panel: ${JSON.stringify(yt.findTranscriptPanel(data.initialData))}; legacy params: ${!!yt.findTranscriptParams(data.initialData)}`);

const summary = (r) => (r.transcript ? `OK ${r.transcript_source} lang=${r.transcript_lang} segments=${r.transcript.length} first=${JSON.stringify(r.transcript[0])}` : 'no transcript') + `\n    attempts: ${JSON.stringify(r.attempts)}`;

// Full chain as the extension runs it (without DOM panel).
const full = await yt.fetchTranscript(data, env);
console.log('fetchTranscript (all strategies):\n  ' + summary(full));

// Legacy get_transcript alone: hide the modern panel endpoint.
const legacyOnly = { ...data, initialData: JSON.parse(JSON.stringify(data.initialData, (k, v) => (k === 'showEngagementPanelEndpoint' ? undefined : v))) };
const legacy = await yt.fetchTranscript(legacyOnly, env);
console.log('legacy get_transcript only (+timedtext fallback):\n  ' + summary(legacy));

if (saveFixtures) {
  const panel = yt.findTranscriptPanel(data.initialData);
  const ctx = yt.contextFor(data.ytcfg, 'en');
  const res = await nodeFetch('https://www.youtube.com/youtubei/v1/get_panel?prettyPrint=false', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-Youtube-Client-Name': '1', 'X-Youtube-Client-Version': ctx.client.clientVersion },
    body: JSON.stringify({ context: ctx, ...panel }),
  });
  const j = await res.json();
  delete j.responseContext; // contains the anonymous visitor id
  fs.writeFileSync(path.join(here, 'fixtures', `get_panel-${videoId}.json`), JSON.stringify(j));
  console.log('saved fixture get_panel-' + videoId + '.json');
}

process.exit(full.transcript ? 0 : 1);
