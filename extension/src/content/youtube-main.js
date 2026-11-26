/*
 * Runs in the YouTube page's MAIN world (after lib/youtube.js), so innertube requests carry
 * the user's own cookies and headers. The value of the last expression (a Promise) is the
 * result that scripting.executeScript hands back to the background script.
 */
(async () => {
  'use strict';
  const yt = globalThis.__liveNotesYouTube;
  delete globalThis.__liveNotesYouTube;

  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const videoId = yt.videoIdFromUrl(location.href);
  if (!videoId) return { error: 'not_a_video' };

  // ---- Fresh page data (YouTube is an SPA: the globals can belong to an older video) ----
  const cfgNow = () => {
    const c = globalThis.ytcfg;
    if (!c) return {};
    if (c.data_) return c.data_;
    const get = (k) => (typeof c.get === 'function' ? c.get(k) : undefined);
    return {
      INNERTUBE_CONTEXT: get('INNERTUBE_CONTEXT'),
      INNERTUBE_API_KEY: get('INNERTUBE_API_KEY'),
      INNERTUBE_CLIENT_VERSION: get('INNERTUBE_CLIENT_VERSION'),
      INNERTUBE_CONTEXT_CLIENT_NAME: get('INNERTUBE_CONTEXT_CLIENT_NAME'),
      SESSION_INDEX: get('SESSION_INDEX'),
    };
  };
  const prVideo = (pr) => pr && pr.videoDetails && pr.videoDetails.videoId;
  const idVideo = (d) => {
    try {
      return d.currentVideoEndpoint.watchEndpoint.videoId;
    } catch {
      return null;
    }
  };

  let playerResponse = null;
  let initialData = null;
  const player = document.getElementById('movie_player');
  if (player && typeof player.getPlayerResponse === 'function') {
    try {
      const pr = player.getPlayerResponse();
      if (prVideo(pr) === videoId) playerResponse = pr;
    } catch {
      // ignore
    }
  }
  if (!playerResponse && prVideo(globalThis.ytInitialPlayerResponse) === videoId) {
    playerResponse = globalThis.ytInitialPlayerResponse;
  }
  if (idVideo(globalThis.ytInitialData) === videoId) initialData = globalThis.ytInitialData;
  if (!initialData) {
    const app = document.querySelector('ytd-app');
    const resp = app && app.data && app.data.response;
    if (idVideo(resp) === videoId) initialData = resp;
  }
  let ytcfg = cfgNow();
  let refetched = false;
  if (!initialData || !playerResponse || location.pathname.startsWith('/shorts/')) {
    // Stale or Shorts layout: load the regular watch page once (same origin, same session).
    try {
      const res = await fetch('/watch?v=' + encodeURIComponent(videoId), { credentials: 'include' });
      const fresh = yt.dataFromWatchHtml(await res.text());
      if (idVideo(fresh.initialData) === videoId) initialData = fresh.initialData;
      if (prVideo(fresh.playerResponse) === videoId) playerResponse = fresh.playerResponse;
      if (fresh.ytcfg && fresh.ytcfg.INNERTUBE_CONTEXT) ytcfg = Object.assign({}, ytcfg, fresh.ytcfg);
      refetched = true;
    } catch {
      // continue with what we have
    }
  }

  // ---- Strategy 2: the transcript panel in the DOM ----
  // The modern panel is sometimes rendered into an engagement panel without a target-id, so
  // look for segment elements in any engagement panel.
  const PANEL_SEL = 'ytd-engagement-panel-section-list-renderer';
  const SEGMENT_SEL = 'ytd-transcript-segment-renderer, transcript-segment-view-model';
  const visible = (el) => !!(el && (el.offsetWidth || el.offsetHeight || el.getClientRects().length));
  const panelRows = () =>
    Array.from(document.querySelectorAll(PANEL_SEL))
      .filter((p) => p.getAttribute('visibility') !== 'ENGAGEMENT_PANEL_VISIBILITY_HIDDEN')
      .flatMap((p) => Array.from(p.querySelectorAll(SEGMENT_SEL)))
      .map((el) => {
      const timeEl = el.querySelector('.segment-timestamp, [class*="Timestamp"], [class*="timestamp"]');
      const textEl = el.querySelector('.segment-text, yt-formatted-string:not(.segment-timestamp), [class*="SegmentText"], span[role="text"]');
      let time = timeEl ? timeEl.textContent : '';
      let text = textEl ? textEl.textContent : '';
      if (!time || !text) {
        const m = (el.textContent || '').trim().match(/^(\d+(?::\d+)+)\s+([\s\S]*)$/);
        if (m) [, time, text] = m;
      }
      return { time: (time || '').trim(), text: (text || '').trim() };
    });

  async function readPanel() {
    if (refetched && location.pathname.startsWith('/shorts/')) throw new Error('no panel on Shorts');
    let rows = panelRows();
    let opened = false;
    const before = new Set(Array.from(document.querySelectorAll(PANEL_SEL)).filter((p) => p.getAttribute('visibility') === 'ENGAGEMENT_PANEL_VISIBILITY_EXPANDED'));
    if (!rows.length) {
      // after SPA navigation the description can contain an old, hidden copy of the button
      const buttons = Array.from(document.querySelectorAll('ytd-video-description-transcript-section-renderer button, button[aria-label*="ranscript" i]'));
      const btn = buttons.filter(visible).pop() || buttons.pop();
      if (!btn) throw new Error('no transcript button');
      btn.click();
      opened = true;
      for (let i = 0; i < 40 && !rows.length; i++) {
        await sleep(250);
        rows = panelRows();
      }
      await sleep(300); // let the list finish rendering
      rows = panelRows();
    }
    const titleEl = document.querySelector(PANEL_SEL + ' #footer yt-dropdown-menu #label-text, ' + PANEL_SEL + ' #footer tp-yt-paper-button #label');
    const title = titleEl ? titleEl.textContent : '';
    if (opened) {
      // close what we opened, leave panels the user had open alone
      for (const p of document.querySelectorAll(PANEL_SEL)) {
        if (p.getAttribute('visibility') === 'ENGAGEMENT_PANEL_VISIBILITY_EXPANDED' && !before.has(p) && p.querySelector(SEGMENT_SEL)) {
          p.setAttribute('visibility', 'ENGAGEMENT_PANEL_VISIBILITY_HIDDEN');
        }
      }
    }
    if (!rows.length) throw new Error('panel did not render segments');
    return { rows, title };
  }

  const data = { videoId, initialData, playerResponse, ytcfg };
  // TEST ONLY: the e2e test sets this to check single strategies; normal pages never do.
  const skip = globalThis.__liveNotesTestSkipStrategies;
  delete globalThis.__liveNotesTestSkipStrategies;
  const result = await yt.fetchTranscript(data, {
    skip: Array.isArray(skip) ? skip : [],
    fetch: (u, o) => fetch(u, o),
    cookie: document.cookie,
    panel: readPanel,
  });
  const meta = yt.videoMeta(playerResponse, initialData);
  return {
    url: location.href,
    title: meta.title || document.title.replace(/ - YouTube$/, ''),
    lang: document.documentElement.lang || null,
    youtube: {
      video_id: videoId,
      channel: meta.channel,
      duration_s: meta.duration_s,
      description: meta.description,
      transcript_lang: result.transcript_lang,
      transcript_source: result.transcript_source,
      transcript: result.transcript,
    },
    attempts: result.attempts,
    refetched,
  };
})();
