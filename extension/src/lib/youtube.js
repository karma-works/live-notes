/*
 * YouTube transcript logic for Live Notes.
 *
 * Plain script (no modules) so it can be injected into the page's main world with
 * scripting.executeScript and also be required from Node for tests. Everything that touches
 * the network or the DOM is passed in via `env`, the rest is pure.
 *
 * Strategy order (see docs/clip-api.md, transcript_source):
 *   1. "innertube": POST youtubei/v1/get_transcript with params from ytInitialData
 *   2. "panel":     open the "Show transcript" panel and read the rendered segments
 *   3. "timedtext": captionTracks[].baseUrl + &fmt=json3 (often empty because of PO tokens)
 */
(function (root) {
  'use strict';

  const ORIGIN = 'https://www.youtube.com';

  // ---------------------------------------------------------------------------------------
  // URL helpers

  /** Video id of a watch/shorts/youtu.be/embed/live URL, or null. */
  function videoIdFromUrl(url) {
    let u;
    try {
      u = new URL(url);
    } catch {
      return null;
    }
    const host = u.hostname.replace(/^www\.|^m\./, '');
    const ok = (id) => (id && /^[A-Za-z0-9_-]{11}$/.test(id) ? id : null);
    if (host === 'youtu.be') return ok(u.pathname.slice(1).split('/')[0]);
    if (host !== 'youtube.com' && host !== 'music.youtube.com') return null;
    if (u.pathname === '/watch') return ok(u.searchParams.get('v'));
    const m = u.pathname.match(/^\/(shorts|embed|live)\/([^/?#]+)/);
    return m ? ok(m[2]) : null;
  }

  function isYouTubeVideoUrl(url) {
    return videoIdFromUrl(url) !== null;
  }

  // ---------------------------------------------------------------------------------------
  // Extracting embedded JSON from watch-page HTML

  /** Parse the JSON object literal that starts at html[start] (must be "{"). */
  function readJsonObject(html, start) {
    let depth = 0;
    let inStr = false;
    let esc = false;
    for (let i = start; i < html.length; i++) {
      const c = html[i];
      if (inStr) {
        if (esc) esc = false;
        else if (c === '\\') esc = true;
        else if (c === '"') inStr = false;
        continue;
      }
      if (c === '"') inStr = true;
      else if (c === '{') depth++;
      else if (c === '}') {
        depth--;
        if (depth === 0) return JSON.parse(html.slice(start, i + 1));
      }
    }
    throw new Error('unterminated JSON object');
  }

  /** Value of `var name = {...};` / `window["name"] = {...}` in the HTML, or null. */
  function extractAssignedJson(html, name) {
    const patterns = [
      new RegExp('(?:var\\s+|window\\[["\']|window\\.)' + name + '(?:["\']\\])?\\s*=\\s*\\{'),
      new RegExp(name + '\\s*=\\s*\\{'),
    ];
    for (const re of patterns) {
      const m = re.exec(html);
      if (!m) continue;
      try {
        return readJsonObject(html, m.index + m[0].length - 1);
      } catch {
        // try next pattern
      }
    }
    return null;
  }

  /** Merge all `ytcfg.set({...})` calls of the HTML. */
  function extractYtcfg(html) {
    const cfg = {};
    const re = /ytcfg\.set\(\s*\{/g;
    let m;
    while ((m = re.exec(html))) {
      try {
        Object.assign(cfg, readJsonObject(html, m.index + m[0].length - 1));
      } catch {
        // ignore broken chunk
      }
    }
    return cfg;
  }

  // ---------------------------------------------------------------------------------------
  // Walking innertube JSON

  /** Depth-first search: call fn(key, value) for every property; collect non-undefined. */
  function collect(obj, key, out = [], seen = new Set()) {
    if (!obj || typeof obj !== 'object' || seen.has(obj)) return out;
    seen.add(obj);
    if (Array.isArray(obj)) {
      for (const v of obj) collect(v, key, out, seen);
      return out;
    }
    for (const k of Object.keys(obj)) {
      const v = obj[k];
      if (k === key) out.push(v);
      if (v && typeof v === 'object') collect(v, key, out, seen);
    }
    return out;
  }

  function textOf(t) {
    if (t == null) return '';
    if (typeof t === 'string') return t;
    if (typeof t.simpleText === 'string') return t.simpleText;
    if (Array.isArray(t.runs)) return t.runs.map((r) => r.text || '').join('');
    if (typeof t.content === 'string') return t.content; // newer view models
    return '';
  }

  /** getTranscriptEndpoint.params from ytInitialData, preferring the transcript panel's. */
  function findTranscriptParams(initialData) {
    if (!initialData) return null;
    const eps = collect(initialData, 'getTranscriptEndpoint').filter((e) => e && e.params);
    return eps.length ? eps[0].params : null;
  }

  function cleanText(s) {
    return String(s).replace(/\s+/g, ' ').trim();
  }

  /** Parse a get_transcript response into [{start_ms, dur_ms, text}]. */
  function parseInnertubeTranscript(json) {
    const segs = [];
    for (const r of collect(json, 'transcriptSegmentRenderer')) {
      const start = Number(r.startMs);
      const end = Number(r.endMs);
      const text = cleanText(textOf(r.snippet));
      if (!Number.isFinite(start) || !text) continue;
      segs.push({ start_ms: start, dur_ms: Number.isFinite(end) ? Math.max(0, end - start) : 0, text });
    }
    segs.sort((a, b) => a.start_ms - b.start_ms);
    return segs;
  }

  /** Language menu of a get_transcript response: [{title, selected, params}]. */
  function parseTranscriptLanguageMenu(json) {
    const out = [];
    for (const menu of collect(json, 'sortFilterSubMenuRenderer')) {
      for (const item of menu.subMenuItems || []) {
        const cont = item.continuation && item.continuation.reloadContinuationData;
        out.push({ title: item.title || '', selected: !!item.selected, params: cont ? cont.continuation : null });
      }
      if (out.length) break;
    }
    return out;
  }

  /** Parse a timedtext json3 document into segments. */
  function parseJson3(json) {
    const segs = [];
    for (const ev of (json && json.events) || []) {
      if (!ev.segs) continue;
      const text = cleanText(ev.segs.map((s) => s.utf8 || '').join(''));
      if (!text) continue;
      segs.push({ start_ms: Number(ev.tStartMs) || 0, dur_ms: Number(ev.dDurationMs) || 0, text });
    }
    return segs;
  }

  /** "1:02:03" → ms */
  function parseTimestamp(s) {
    if (!/^\d+(:\d+)*$/.test(String(s).trim())) return null;
    const parts = String(s).trim().split(':').map(Number);
    if (!parts.length || parts.some((n) => !Number.isFinite(n))) return null;
    return parts.reduce((acc, n) => acc * 60 + n, 0) * 1000;
  }

  /** [{time:"0:01", text}] from the panel → segments (durations from the following start). */
  function segmentsFromPanelRows(rows) {
    const segs = [];
    for (const r of rows) {
      const start = parseTimestamp(r.time);
      const text = cleanText(r.text || '');
      if (start == null || !text) continue;
      segs.push({ start_ms: start, dur_ms: 0, text });
    }
    for (let i = 0; i < segs.length; i++) {
      const next = segs[i + 1];
      segs[i].dur_ms = next ? Math.max(0, next.start_ms - segs[i].start_ms) : 0;
    }
    return segs;
  }

  // ---------------------------------------------------------------------------------------
  // Player response: metadata and caption tracks

  function captionTracks(playerResponse) {
    const r = playerResponse && playerResponse.captions && playerResponse.captions.playerCaptionsTracklistRenderer;
    return (r && r.captionTracks) || [];
  }

  function trackName(t) {
    return textOf(t.name);
  }

  const baseLang = (code) => String(code || '').toLowerCase().split(/[-_]/)[0];

  /**
   * Pick the track that represents the video best: a manual track in the spoken language,
   * then the auto-generated track (spoken language), then any manual track, then anything.
   * Auto-translated tracks never appear in captionTracks, so they are never chosen.
   */
  function chooseCaptionTrack(tracks, hints = {}) {
    if (!tracks || !tracks.length) return null;
    const asr = tracks.find((t) => t.kind === 'asr');
    const manual = tracks.filter((t) => t.kind !== 'asr');
    const spoken = baseLang(hints.originalLang || (asr && asr.languageCode));
    if (spoken) {
      const m = manual.find((t) => baseLang(t.languageCode) === spoken);
      if (m) return m;
      if (asr) return asr;
    }
    // Without an auto-generated track nothing names the spoken language, and the track order
    // follows the viewer's UI language. YouTube turns captions on by default
    // (CAPTIONS_INITIAL_STATE_ON_RECOMMENDED) when the audio is NOT in the UI language, so then
    // a track in another language is the better guess for the original.
    const ui = baseLang(hints.uiLang);
    if (ui && hints.captionsState === 'CAPTIONS_INITIAL_STATE_ON_RECOMMENDED') {
      const other = manual.find((t) => baseLang(t.languageCode) !== ui);
      if (other) return other;
    }
    const def = hints.defaultIndex != null ? tracks[hints.defaultIndex] : null;
    if (def && def.kind !== 'asr') return def;
    return manual[0] || asr || tracks[0];
  }

  /** Hints for chooseCaptionTrack from the player response and the page's UI language. */
  function trackHints(playerResponse, ytcfg) {
    const r = playerResponse && playerResponse.captions && playerResponse.captions.playerCaptionsTracklistRenderer;
    const audio = r && r.audioTracks && r.audioTracks[0];
    const ctx = ytcfg && ytcfg.INNERTUBE_CONTEXT;
    return {
      originalLang: null,
      uiLang: (ctx && ctx.client && ctx.client.hl) || (ytcfg && ytcfg.HL) || null,
      captionsState: (audio && audio.captionsInitialState) || null,
      defaultIndex: audio && audio.defaultCaptionTrackIndex != null ? audio.defaultCaptionTrackIndex : null,
    };
  }

  /** Map a transcript-menu/panel title ("English (auto-generated)") to a caption track. */
  function trackForTitle(tracks, title) {
    const t = cleanText(title).toLowerCase();
    if (!t) return null;
    return tracks.find((x) => cleanText(trackName(x)).toLowerCase() === t) || null;
  }

  function videoMeta(playerResponse, initialData) {
    const d = (playerResponse && playerResponse.videoDetails) || {};
    const micro = (playerResponse && playerResponse.microformat && playerResponse.microformat.playerMicroformatRenderer) || {};
    let channel = d.author || textOf(micro.ownerChannelName) || null;
    if (!channel && initialData) {
      const owner = collect(initialData, 'videoOwnerRenderer')[0];
      if (owner) channel = textOf(owner.title) || null;
    }
    const duration = Number(d.lengthSeconds || micro.lengthSeconds);
    return {
      video_id: d.videoId || null,
      title: d.title || textOf(micro.title) || null,
      channel,
      duration_s: Number.isFinite(duration) && duration > 0 ? duration : null,
      description: d.shortDescription || textOf(micro.description) || null,
    };
  }

  // ---------------------------------------------------------------------------------------
  // Network: innertube + timedtext

  async function sha1Hex(str, env) {
    const subtle = (env && env.subtle) || (root.crypto && root.crypto.subtle);
    if (!subtle) return null;
    const buf = await subtle.digest('SHA-1', new TextEncoder().encode(str));
    return Array.from(new Uint8Array(buf), (b) => b.toString(16).padStart(2, '0')).join('');
  }

  function cookieValue(cookieString, name) {
    const m = new RegExp('(?:^|;\\s*)' + name.replace(/[-[\]{}()*+?.,\\^$|#\s]/g, '\\$&') + '=([^;]*)').exec(cookieString || '');
    return m ? m[1] : null;
  }

  /** Headers that YouTube's web client sends with innertube requests. */
  async function innertubeHeaders(cfg, env) {
    const ctx = cfg.INNERTUBE_CONTEXT || {};
    const client = ctx.client || {};
    const h = {
      'Content-Type': 'application/json',
      'X-Youtube-Client-Name': String(cfg.INNERTUBE_CONTEXT_CLIENT_NAME || 1),
      'X-Youtube-Client-Version': String(cfg.INNERTUBE_CLIENT_VERSION || client.clientVersion || ''),
    };
    if (client.visitorData) h['X-Goog-Visitor-Id'] = client.visitorData;
    // Signed-in session: same SAPISIDHASH authorization the page itself uses.
    const sapisid = cookieValue(env.cookie, 'SAPISID') || cookieValue(env.cookie, '__Secure-3PAPISID');
    if (sapisid) {
      const ts = Math.floor(Date.now() / 1000);
      const hash = await sha1Hex(ts + ' ' + sapisid + ' ' + ORIGIN, env);
      if (hash) {
        h.Authorization = 'SAPISIDHASH ' + ts + '_' + hash;
        h['X-Origin'] = ORIGIN;
        h['X-Goog-AuthUser'] = String(cfg.SESSION_INDEX || 0);
      }
    }
    return h;
  }

  async function postInnertube(cfg, endpoint, body, env) {
    const key = cfg.INNERTUBE_API_KEY ? 'key=' + encodeURIComponent(cfg.INNERTUBE_API_KEY) + '&' : '';
    const url = (env.origin || ORIGIN) + '/youtubei/v1/' + endpoint + '?' + key + 'prettyPrint=false';
    const res = await env.fetch(url, {
      method: 'POST',
      headers: await innertubeHeaders(cfg, env),
      body: JSON.stringify(body),
      credentials: 'include',
    });
    const text = await res.text();
    if (!res.ok) {
      let msg = endpoint + ' HTTP ' + res.status;
      try {
        msg += ' ' + JSON.parse(text).error.status;
      } catch {
        // keep status only
      }
      throw new Error(msg);
    }
    return JSON.parse(text);
  }

  /** Innertube context with the UI language set to `lang`, which selects the transcript track. */
  function contextFor(cfg, lang) {
    const ctx = JSON.parse(JSON.stringify(cfg.INNERTUBE_CONTEXT));
    if (lang && ctx.client) ctx.client.hl = lang;
    return ctx;
  }

  /**
   * The current ("modern") transcript panel: ytInitialData has a showEngagementPanelEndpoint
   * for panel PAmodern_transcript_view whose globalConfiguration.params go to get_panel.
   */
  function findTranscriptPanel(initialData) {
    for (const ep of collect(initialData, 'showEngagementPanelEndpoint')) {
      const tag = ep && ep.identifier && ep.identifier.tag;
      const params = ep && ep.globalConfiguration && ep.globalConfiguration.params;
      if (tag && /transcript/i.test(tag) && params) return { panelId: tag, params };
    }
    return null;
  }

  /**
   * get_panel params for the transcript panel when ytInitialData does not carry them (stale
   * after SPA navigation, or an experiment arm without the endpoint). Same bytes YouTube's own
   * client sends: field 1189 { 1: videoId, 3: 1 }.
   */
  function buildPanelParams(videoId) {
    const id = String(videoId);
    const inner = [0x0a, id.length].concat(Array.from(id, (c) => c.charCodeAt(0)), [0x18, 0x01]);
    const bytes = [0xaa, 0x09, inner.length].concat(inner);
    return btoa(String.fromCharCode.apply(null, bytes));
  }

  /** Parse a get_panel response (macroMarkersPanelItemViewModel list) into segments. */
  function parsePanelTranscript(json, durationS) {
    const items = collect(json, 'macroMarkersPanelItemViewModel');
    const times = collect(json, 'timedSyncDataList').reduce((a, l) => a.concat(l || []), []).map((x) => Number(x && x.videoTimeMs));
    const useTimes = times.length === items.length && times.every(Number.isFinite);
    const segs = [];
    items.forEach((it, i) => {
      const tl = it && it.item && it.item.timelineItemViewModel;
      if (!tl) return;
      const text = cleanText(
        (tl.contentItems || [])
          .map((c) => (c.transcriptSegmentViewModel ? textOf(c.transcriptSegmentViewModel) : ''))
          .join(' '),
      );
      if (!text) return;
      let start = useTimes ? times[i] : null;
      if (start == null) {
        const we = collect(it.onTap, 'watchEndpoint')[0];
        if (we && Number.isFinite(Number(we.startTimeSeconds))) start = Number(we.startTimeSeconds) * 1000;
      }
      if (start == null) start = parseTimestamp(tl.timestamp);
      if (start == null) return;
      segs.push({ start_ms: start, dur_ms: 0, text });
    });
    segs.sort((x, y) => x.start_ms - y.start_ms);
    fillDurations(segs, durationS);
    return segs;
  }

  /** dur_ms = gap to the next segment (last one: until the end of the video, if known). */
  function fillDurations(segs, durationS) {
    for (let i = 0; i < segs.length; i++) {
      const next = segs[i + 1];
      const end = next ? next.start_ms : Number.isFinite(durationS) ? durationS * 1000 : segs[i].start_ms;
      segs[i].dur_ms = Math.max(0, end - segs[i].start_ms);
    }
    return segs;
  }

  /** Strategy 1. Returns {transcript, lang} or throws. */
  async function viaInnertube(data, env) {
    if (!data.ytcfg || !data.ytcfg.INNERTUBE_CONTEXT) throw new Error('no INNERTUBE_CONTEXT');
    const tracks = captionTracks(data.playerResponse);
    const want = chooseCaptionTrack(tracks, trackHints(data.playerResponse, data.ytcfg));
    const wantLang = want ? baseLang(want.languageCode) : null;
    const durationS = videoMeta(data.playerResponse).duration_s;
    const errors = [];

    // 1a. get_panel (what youtube.com itself uses since 2025/26)
    const videoId = data.videoId || videoMeta(data.playerResponse).video_id;
    const panel =
      findTranscriptPanel(data.initialData) || (videoId ? { panelId: 'PAmodern_transcript_view', params: buildPanelParams(videoId), built: true } : null);
    if (panel) {
      try {
        const json = await postInnertube(data.ytcfg, 'get_panel', { context: contextFor(data.ytcfg, wantLang), panelId: panel.panelId, params: panel.params }, env);
        const transcript = parsePanelTranscript(json, durationS);
        if (transcript.length) return { transcript, lang: want ? want.languageCode : null, endpoint: panel.built ? 'get_panel (built params)' : 'get_panel' };
        errors.push('get_panel: no segments');
      } catch (e) {
        errors.push(String(e.message || e));
      }
    }

    // 1b. legacy get_transcript
    const params = findTranscriptParams(data.initialData);
    if (!params) throw new Error(errors.concat('no getTranscriptEndpoint').join('; '));
    try {
      let json = await postInnertube(data.ytcfg, 'get_transcript', { context: contextFor(data.ytcfg, wantLang), params }, env);
      const menu = parseTranscriptLanguageMenu(json);
      let selected = menu.find((m) => m.selected);
      if (want && menu.length) {
        const item = menu.find((m) => trackForTitle([want], m.title));
        if (item && !item.selected && item.params) {
          try {
            json = await postInnertube(data.ytcfg, 'get_transcript', { context: contextFor(data.ytcfg, wantLang), params: item.params }, env);
            selected = item;
          } catch {
            // keep the default language
          }
        }
      }
      const transcript = parseInnertubeTranscript(json);
      if (!transcript.length) throw new Error('get_transcript: no segments');
      const track = (selected && trackForTitle(tracks, selected.title)) || want;
      return { transcript, lang: track ? track.languageCode : null, endpoint: 'get_transcript' };
    } catch (e) {
      errors.push(String(e.message || e));
      throw new Error(errors.join('; '));
    }
  }

  /** Strategy 3. */
  async function viaTimedtext(data, env) {
    const tracks = captionTracks(data.playerResponse);
    const track = chooseCaptionTrack(tracks, trackHints(data.playerResponse, data.ytcfg));
    if (!track || !track.baseUrl) throw new Error('no captionTracks');
    const url = track.baseUrl.replace(/&fmt=[^&]*/, '') + '&fmt=json3';
    const res = await env.fetch(url, { credentials: 'include' });
    if (!res.ok) throw new Error('HTTP ' + res.status);
    const body = await res.text();
    if (!body.trim()) throw new Error('empty timedtext response (PO token required)');
    const transcript = parseJson3(JSON.parse(body));
    if (!transcript.length) throw new Error('timedtext without segments');
    return { transcript, lang: track.languageCode || null };
  }

  /**
   * Run all strategies. env: {fetch, cookie?, subtle?, panel?: async () => {rows, title}}.
   * data: {videoId, initialData, playerResponse, ytcfg}.
   */
  async function fetchTranscript(data, env) {
    const attempts = [];
    const strategies = [
      ['innertube', () => viaInnertube(data, env)],
      [
        'panel',
        async () => {
          if (!env.panel) throw new Error('no DOM');
          const { rows, title } = await env.panel();
          const transcript = fillDurations(segmentsFromPanelRows(rows || []), videoMeta(data.playerResponse).duration_s);
          if (!transcript.length) throw new Error('panel had no segments');
          // The panel shows the track YouTube picked for the UI language.
          const tracks = captionTracks(data.playerResponse);
          const ui = baseLang(data.ytcfg && data.ytcfg.INNERTUBE_CONTEXT && data.ytcfg.INNERTUBE_CONTEXT.client && data.ytcfg.INNERTUBE_CONTEXT.client.hl);
          const track =
            trackForTitle(tracks, title || '') ||
            tracks.find((x) => x.kind !== 'asr' && baseLang(x.languageCode) === ui) ||
            chooseCaptionTrack(tracks, trackHints(data.playerResponse, data.ytcfg));
          return { transcript, lang: track ? track.languageCode : null };
        },
      ],
      ['timedtext', () => viaTimedtext(data, env)],
    ];
    for (const [name, run] of strategies) {
      if (env.skip && env.skip.includes(name)) continue;
      try {
        const r = await run();
        attempts.push({ strategy: name, ok: true, segments: r.transcript.length, endpoint: r.endpoint });
        return { transcript: r.transcript, transcript_lang: r.lang, transcript_source: name, attempts };
      } catch (e) {
        attempts.push({ strategy: name, ok: false, error: String((e && e.message) || e) });
      }
    }
    const tracks = captionTracks(data.playerResponse);
    const t = chooseCaptionTrack(tracks, trackHints(data.playerResponse, data.ytcfg));
    return { transcript: null, transcript_lang: t ? t.languageCode : null, transcript_source: null, attempts };
  }

  /** Watch-page data for a video id from fetched HTML. */
  function dataFromWatchHtml(html) {
    return {
      initialData: extractAssignedJson(html, 'ytInitialData'),
      playerResponse: extractAssignedJson(html, 'ytInitialPlayerResponse'),
      ytcfg: extractYtcfg(html),
    };
  }

  const api = {
    videoIdFromUrl,
    isYouTubeVideoUrl,
    readJsonObject,
    extractAssignedJson,
    extractYtcfg,
    findTranscriptParams,
    findTranscriptPanel,
    buildPanelParams,
    parsePanelTranscript,
    fillDurations,
    contextFor,
    parseInnertubeTranscript,
    parseTranscriptLanguageMenu,
    parseJson3,
    parseTimestamp,
    segmentsFromPanelRows,
    captionTracks,
    chooseCaptionTrack,
    trackHints,
    trackForTitle,
    videoMeta,
    innertubeHeaders,
    fetchTranscript,
    dataFromWatchHtml,
    textOf,
  };

  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  else root.__liveNotesYouTube = api;
})(typeof globalThis !== 'undefined' ? globalThis : this);
